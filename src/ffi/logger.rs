//! Foreign-callback bridge into the `log` crate.
//!
//! Foreign code (Swift/Kotlin) implements [`BarkLogger`]
//! and registers the implementation via [`set_logger`]. All `log::info!` /
//! `warn!` / etc. calls in this crate (and in `bark`/`ark`) are then forwarded
//! to the foreign callback.
//!
//! The bridge is installed into `log` once per process, but the logger behind
//! it can be replaced ([`set_logger`] again) or removed ([`clear_logger`]).

use std::sync::{Arc, PoisonError, RwLock};

use log::LevelFilter;
use once_cell::sync::OnceCell;

use crate::error::BarkError;
use crate::types::LogLevel;

/// Foreign-implemented sink for log records.
///
/// IMPORTANT: implementations must not call back into bark APIs that
/// themselves emit log records, or the foreign runtime may stack-overflow
/// or deadlock.
#[boltffi::export]
pub trait BarkLogger: Send + Sync {
    fn log(&self, level: LogLevel, target: String, message: String);
}

struct Bridge;

static BRIDGE: Bridge = Bridge;

/// Whether the first `set_logger` call managed to install [`BRIDGE`] as the
/// `log` crate's logger. `log` accepts a single logger per process, so later
/// calls reuse this outcome.
static INSTALLED: OnceCell<bool> = OnceCell::new();

/// The logger records are forwarded to. `None` until `set_logger` and after
/// `clear_logger`.
static SINK: RwLock<Option<Arc<dyn BarkLogger>>> = RwLock::new(None);

/// HTTP and gRPC transport crates log every request and pooled connection at
/// Debug and Trace. Only their Info and above reach the foreign logger.
const TRANSPORT_CRATES: &[&str] = &[
    "h2",
    "hyper",
    "hyper_util",
    "mio",
    "reqwest",
    "rustls",
    "tokio_rustls",
    "tonic",
    "tower",
    "tower_http",
    "want",
];

fn forwards(level: log::Level, target: &str, max_level: LevelFilter) -> bool {
    let krate = target.split("::").next().unwrap_or_default();
    level <= max_level && (level <= log::Level::Info || !TRANSPORT_CRATES.contains(&krate))
}

impl log::Log for Bridge {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        forwards(metadata.level(), metadata.target(), log::max_level())
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Foreign callbacks may replace or clear the logger; release the lock first.
        let sink = SINK.read().unwrap_or_else(PoisonError::into_inner).clone();
        let Some(sink) = sink else {
            return;
        };
        sink.log(
            record.level().into(),
            record.target().to_string(),
            format!("{}", record.args()),
        );
    }

    fn flush(&self) {}
}

/// Swap the current logger for `sink`.
fn replace_sink(sink: Option<Arc<dyn BarkLogger>>) {
    let previous = std::mem::replace(
        &mut *SINK.write().unwrap_or_else(PoisonError::into_inner),
        sink,
    );
    drop(previous);
}

/// Send bark's log records at or above `max_level` to `logger`.
///
/// Calling this again replaces the current logger and level. The replaced
/// logger is released. `clear_logger` turns logging off.
///
/// Errors if another logger for Rust's `log` crate was installed in this
/// process before the first call.
#[boltffi::export]
pub fn set_logger(logger: Arc<dyn BarkLogger>, max_level: LogLevel) -> Result<(), BarkError> {
    if !*INSTALLED.get_or_init(|| log::set_logger(&BRIDGE).is_ok()) {
        return Err(BarkError::from("could not set logger"));
    }
    // Swap the logger before raising the level, so records the new level lets
    // through never reach the old logger.
    replace_sink(Some(logger));
    log::set_max_level(max_level.into());
    Ok(())
}

/// Turn logging off and detach the current logger, and release its callback.
///
#[boltffi::export]
pub fn clear_logger() {
    log::set_max_level(LevelFilter::Off);
    replace_sink(None);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Keeps the messages logged from this module, ignoring records other
    /// tests emit while the process-wide logger is set.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl BarkLogger for Recorder {
        fn log(&self, _level: LogLevel, target: String, message: String) {
            if target == module_path!() {
                self.0.lock().unwrap().push(message);
            }
        }
    }

    impl Recorder {
        fn messages(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    // One test: the `log` logger and max level are process-wide.
    #[test]
    fn set_logger_replaces_and_clear_logger_detaches() {
        let first = Arc::new(Recorder::default());
        let second = Arc::new(Recorder::default());

        set_logger(first.clone(), LogLevel::Info).unwrap();
        log::info!("to first");
        log::debug!("below first's level");

        set_logger(second.clone(), LogLevel::Warn).unwrap();
        log::info!("below second's level");
        log::warn!("to second");

        clear_logger();
        log::error!("after clear");

        assert_eq!(first.messages(), ["to first"]);
        assert_eq!(second.messages(), ["to second"]);
        assert_eq!(Arc::strong_count(&first), 1);
        assert_eq!(Arc::strong_count(&second), 1);
    }

    #[test]
    fn transport_crates_forward_only_info_and_above() {
        use log::Level::{Debug, Info, Trace, Warn};
        let max = LevelFilter::Trace;

        assert!(!forwards(Debug, "hyper_util::client::legacy::pool", max));
        assert!(!forwards(Trace, "h2", max));
        assert!(forwards(Info, "h2::proto", max));
        assert!(forwards(Warn, "tonic::transport", max));
        assert!(forwards(Debug, "bark::wallet", max));
        assert!(forwards(Trace, "hyperion", max));
        assert!(!forwards(Info, "h2", LevelFilter::Warn));
    }
}
