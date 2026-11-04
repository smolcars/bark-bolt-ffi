//! Live Ark mailbox processing for wallets opened without Bark's daemon.
//!
//! Runs Bark's own `subscribe_process_mailbox_messages` on the SDK runtime, so
//! arkoor receives, Lightning receive notices and delegated round completions
//! are applied as the server delivers them instead of on the next `sync()`.
//! Each message goes through the same Bark handler `sync()` uses, which
//! serializes the arkoor dedup and only moves the checkpoint forward, so the
//! processor can run next to `sync()` and payment calls.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{info, warn};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::core::wallet::Wallet as CoreWallet;
use crate::ffi::runtime::TOKIO_RT;

/// First wait before resubscribing after Bark's subscription gives up.
const RESTART_DELAY: Duration = Duration::from_secs(1);
/// Longest wait between resubscribes while the server stays unreachable. A
/// subscription that lasted at least this long starts over at `RESTART_DELAY`.
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);
/// How long a cancelled subscription may take to return. Bark checks for
/// cancellation between messages, so a message in flight finishes; it doesn't
/// while connecting. After this the subscription is dropped, as if the app had
/// been killed, and Bark resumes from the unadvanced checkpoint next time.
const STOP_GRACE: Duration = Duration::from_secs(5);

struct Task {
    shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

#[derive(Default)]
struct State {
    task: Option<Task>,
    /// Bark's daemon processes the mailbox itself; while it runs the processor
    /// stays off so the wallet has one live consumer.
    daemon: bool,
    #[cfg(test)]
    spawned: usize,
}

/// Owns the processor task of one native wallet.
///
/// Every lifecycle change, daemon start and stop included, holds `state` until
/// it has finished. A start therefore never overlaps a stop that is still
/// waiting for its task, and `daemon` always matches Bark's daemon.
pub(crate) struct MailboxProcessor {
    state: Mutex<State>,
    /// Cancelled when the wallet is dropped; each task's token is a child.
    closed: CancellationToken,
}

impl MailboxProcessor {
    pub(crate) fn new(daemon: bool) -> Self {
        Self {
            state: Mutex::new(State {
                daemon,
                ..State::default()
            }),
            closed: CancellationToken::new(),
        }
    }

    /// Start the processor unless it already runs or the daemon does.
    pub(crate) async fn start(&self, core: &Arc<CoreWallet>) {
        let core = core.clone();
        self.start_with(move |shutdown| {
            let core = core.clone();
            async move {
                core.inner()
                    .subscribe_process_mailbox_messages(None, shutdown)
                    .await
            }
        })
        .await
    }

    async fn start_with<F, Fut>(&self, subscribe: F)
    where
        F: FnMut(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let mut state = self.state.lock().await;
        if state.daemon {
            info!("[MAILBOX] Daemon already processes the mailbox; processor not started");
            return;
        }
        if state.task.as_ref().is_some_and(|t| !t.handle.is_finished()) {
            return;
        }
        let shutdown = self.closed.child_token();
        let handle = TOKIO_RT.spawn(run(subscribe, shutdown.clone()));
        state.task = Some(Task { shutdown, handle });
        #[cfg(test)]
        {
            state.spawned += 1;
        }
    }

    /// Stop the processor and wait until its task has ended. No-op when the
    /// processor is not running.
    pub(crate) async fn stop(&self) {
        stop_task(&mut *self.state.lock().await).await;
    }

    /// Stop the processor, then start Bark's daemon with `start_daemon`. The
    /// processor stays off until [`Self::stop_daemon`].
    pub(crate) async fn hand_to_daemon(
        &self,
        start_daemon: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        if state.task.is_some() {
            info!("[MAILBOX] Stopping processor; the daemon takes over the mailbox");
        }
        stop_task(&mut state).await;
        start_daemon()?;
        state.daemon = true;
        Ok(())
    }

    /// Stop Bark's daemon with `stop_daemon`; the processor may start again.
    pub(crate) async fn stop_daemon<T>(&self, stop_daemon: impl Future<Output = T>) -> T {
        let mut state = self.state.lock().await;
        let stopped = stop_daemon.await;
        // Bark drops its daemon handle even when waiting for it fails.
        state.daemon = false;
        stopped
    }

    /// Stop the processor without waiting, for the wallet's `Drop`. Its task
    /// releases the wallet within `STOP_GRACE`.
    pub(crate) fn close(&self) {
        self.closed.cancel();
    }
}

async fn stop_task(state: &mut State) {
    if let Some(task) = state.task.take() {
        task.shutdown.cancel();
        if let Err(error) = task.handle.await {
            warn!("[MAILBOX] Processor task failed: {error}");
        }
    }
}

/// Resubscribe until cancelled. `subscribe` owns the wallet reference, so it
/// is released when this returns.
async fn run<F, Fut>(mut subscribe: F, shutdown: CancellationToken)
where
    F: FnMut(CancellationToken) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    info!("[MAILBOX] Processor started");
    let mut delay = RESTART_DELAY;
    loop {
        let started = Instant::now();
        // Boxed: Bark's subscription future is large.
        let mut subscription = Box::pin(subscribe(shutdown.clone()));
        let result = tokio::select! {
            result = &mut subscription => result,
            _ = shutdown.cancelled() => {
                if tokio::time::timeout(STOP_GRACE, subscription).await.is_err() {
                    warn!("[MAILBOX] Subscription still busy after {STOP_GRACE:?}; dropping it");
                }
                break;
            }
        };
        if shutdown.is_cancelled() {
            break;
        }
        match result {
            Ok(()) => warn!("[MAILBOX] Mailbox stream ended; resubscribing"),
            Err(error) => warn!("[MAILBOX] Mailbox stream failed; resubscribing: {error:#}"),
        }
        if started.elapsed() >= MAX_RESTART_DELAY {
            delay = RESTART_DELAY;
        }
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.cancelled() => break,
        }
        delay = (delay * 2).min(MAX_RESTART_DELAY);
    }
    info!("[MAILBOX] Processor stopped");
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::{Arc, Weak};
    use std::time::{Duration, Instant};

    use tokio_util::sync::CancellationToken;

    use super::{MailboxProcessor, STOP_GRACE};
    use crate::ffi::runtime::TOKIO_RT;
    use crate::ffi::wallet::{Wallet, WalletOpenArgs};
    use crate::types::{Network, WalletNotification};
    use crate::Config;

    /// A stopping task still owns the lifecycle: a concurrent stop returns
    /// only once it has ended, and a concurrent start can't run next to it.
    #[test]
    fn stop_holds_the_lifecycle_until_the_task_ends() {
        TOKIO_RT.block_on(async {
            let processor = MailboxProcessor::new(false);
            let ended = Arc::new(AtomicBool::new(false));
            let flag = ended.clone();
            // Takes 300 ms to return once cancelled, like a message in flight.
            processor
                .start_with(move |shutdown: CancellationToken| {
                    let flag = flag.clone();
                    async move {
                        shutdown.cancelled().await;
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        flag.store(true, SeqCst);
                        Ok(())
                    }
                })
                .await;
            let overlapped = Arc::new(AtomicBool::new(false));
            let later = || tokio::time::sleep(Duration::from_millis(50));
            let (_, second_stop_saw_end, _) = tokio::join!(
                processor.stop(),
                async {
                    later().await;
                    processor.stop().await;
                    ended.load(SeqCst)
                },
                async {
                    later().await;
                    let (ended, overlapped) = (ended.clone(), overlapped.clone());
                    processor
                        .start_with(move |shutdown: CancellationToken| {
                            overlapped.fetch_or(!ended.load(SeqCst), SeqCst);
                            async move {
                                shutdown.cancelled().await;
                                Ok(())
                            }
                        })
                        .await;
                },
            );
            assert!(second_stop_saw_end, "a concurrent stop returned early");
            assert_eq!(processor.state.lock().await.spawned, 2);
            processor.stop().await;
            assert!(
                !overlapped.load(SeqCst),
                "a start ran next to the stopping task"
            );
        });
    }

    /// `run_daemon` and a concurrent `stop_daemon` apply in order, so the
    /// daemon flag matches the daemon afterwards.
    #[test]
    fn daemon_start_and_stop_are_serialized() {
        TOKIO_RT.block_on(async {
            let processor = MailboxProcessor::new(false);
            let ended = Arc::new(AtomicBool::new(false));
            let flag = ended.clone();
            processor
                .start_with(move |shutdown: CancellationToken| {
                    let flag = flag.clone();
                    async move {
                        shutdown.cancelled().await;
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        flag.store(true, SeqCst);
                        Ok(())
                    }
                })
                .await;
            let daemon = Arc::new(AtomicBool::new(false));
            let (started, _) = tokio::join!(
                processor.hand_to_daemon(|| {
                    assert!(ended.load(SeqCst), "daemon started next to the processor");
                    daemon.store(true, SeqCst);
                    Ok(())
                }),
                async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    processor
                        .stop_daemon(async {
                            assert!(daemon.load(SeqCst), "daemon stopped before it started");
                            daemon.store(false, SeqCst);
                        })
                        .await
                },
            );
            started.unwrap();
            assert!(!processor.state.lock().await.daemon);
            processor
                .start_with(|shutdown: CancellationToken| async move {
                    shutdown.cancelled().await;
                    Ok(())
                })
                .await;
            assert_eq!(processor.state.lock().await.spawned, 2);
            processor.stop().await;
        });
    }

    /// Bark doesn't check its token while connecting: a subscription that
    /// never returns is abandoned after `STOP_GRACE` on stop and on close,
    /// releasing what it holds.
    #[test]
    fn stop_and_close_abandon_a_stuck_subscription() {
        TOKIO_RT.block_on(async {
            let processor = MailboxProcessor::new(false);
            for close in [false, true] {
                let probe = Arc::new(());
                let held = Arc::downgrade(&probe);
                processor
                    .start_with(move |_shutdown: CancellationToken| {
                        let probe = probe.clone();
                        async move {
                            let _probe = probe;
                            std::future::pending::<anyhow::Result<()>>().await
                        }
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                let stopping = Instant::now();
                if close {
                    processor.close();
                } else {
                    processor.stop().await;
                }
                while held.strong_count() > 0 {
                    assert!(stopping.elapsed() < STOP_GRACE + Duration::from_secs(2));
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        });
    }

    fn spawned(wallet: &Wallet) -> usize {
        wallet.mailbox.state.try_lock().unwrap().spawned
    }

    fn running(wallet: &Wallet) -> bool {
        let state = wallet.mailbox.state.try_lock().unwrap();
        state.task.as_ref().is_some_and(|t| !t.handle.is_finished())
    }

    fn args(datadir: &std::path::Path) -> WalletOpenArgs {
        WalletOpenArgs {
            run_daemon: false,
            datadir: datadir.to_string_lossy().into(),
            create_if_not_exists: true,
            create_without_server: true,
            skip_recovery: true,
        }
    }

    /// Wait until every strong reference to the wallet core is gone, i.e. the
    /// processor task released it after the native wallet was dropped.
    async fn released(core: Weak<crate::core::wallet::Wallet>) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while core.strong_count() > 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("processor kept the wallet alive after drop");
    }

    /// Serve just enough Esplora for a RegTest wallet to be created and
    /// opened offline: the genesis block and a tip at height 0.
    fn esplora_stub() -> String {
        use std::io::{BufRead, BufReader, Write};
        const GENESIS: &str = "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206";
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = String::new();
                if BufReader::new(&stream).read_line(&mut request).is_err() {
                    continue;
                }
                let path = request.split_whitespace().nth(1).unwrap_or_default();
                let (status, body) = match path {
                    "/block-height/0" | "/blocks/tip/hash" => ("200 OK", GENESIS),
                    "/blocks/tip/height" => ("200 OK", "0"),
                    "/fee-estimates" => ("200 OK", "{}"),
                    _ => ("404 Not Found", ""),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        address
    }

    /// Lifecycle against an unreachable Ark server, so it runs without
    /// RegTest: the processor keeps resubscribing, and start/stop/daemon/drop
    /// still behave.
    #[test]
    fn lifecycle_without_server() {
        let esplora = esplora_stub();
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let config: Config = serde_json::from_value(serde_json::json!({
                "server_address": "http://127.0.0.1:9",
                "esplora_address": esplora,
            }))
            .unwrap();
            let wallet = Wallet::open(
                Network::Regtest,
                crate::generate_mnemonic().unwrap(),
                config,
                args(directory.path()),
            )
            .await
            .unwrap();

            // Start twice: one task.
            wallet.start_mailbox_processor().await.unwrap();
            wallet.start_mailbox_processor().await.unwrap();
            assert_eq!(spawned(&wallet), 1);
            // Failed subscriptions are retried, not fatal.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            assert!(running(&wallet));

            // Stop waits for the task; a second stop is a no-op; restart spawns anew.
            tokio::time::timeout(Duration::from_secs(5), wallet.stop_mailbox_processor())
                .await
                .expect("stop hung")
                .unwrap();
            assert!(!running(&wallet));
            wallet.stop_mailbox_processor().await.unwrap();
            wallet.start_mailbox_processor().await.unwrap();
            assert_eq!(spawned(&wallet), 2);
            assert!(running(&wallet));

            // The daemon takes the mailbox over and the processor stays off
            // until the daemon stops.
            wallet.run_daemon().await.unwrap();
            assert!(!running(&wallet));
            wallet.start_mailbox_processor().await.unwrap();
            assert_eq!(spawned(&wallet), 2);
            wallet.stop_daemon_wait().await.unwrap();
            wallet.start_mailbox_processor().await.unwrap();
            assert_eq!(spawned(&wallet), 3);

            // Dropping the wallet while the processor runs stops it and
            // releases the wallet.
            let core = std::sync::Arc::downgrade(&wallet.core);
            drop(wallet);
            released(core).await;
        });
    }

    /// An arkoor payment reaches `notifications()` without any `sync()`, the
    /// processor resumes from the checkpoint after a restart, and running it
    /// next to `sync()` records each receive once.
    #[test]
    fn live_arkoor_receive_without_sync() {
        if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
            return;
        }
        fn regtest(args: &[&str]) -> String {
            let output = std::process::Command::new("bash")
                .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/ark-dev.sh"))
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "RegTest helper failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }
        /// Seconds until the next arkoor receive notification, or `None`
        /// when none arrives within `limit`.
        async fn arkoor_receive(
            holder: &crate::NotificationHolder,
            limit: Duration,
        ) -> Option<(u64, Duration)> {
            let started = std::time::Instant::now();
            tokio::time::timeout(limit, async {
                loop {
                    let notification = holder.next_notification().await.unwrap();
                    if let Some(WalletNotification::MovementCreated { movement }) = notification {
                        if movement.subsystem_name == "bark.arkoor"
                            && movement.subsystem_kind == "receive"
                        {
                            return (movement.effective_balance_sats as u64, started.elapsed());
                        }
                    }
                }
            })
            .await
            .ok()
        }

        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let config: Config = serde_json::from_value(serde_json::json!({
                "server_address": crate::ffi::regtest_url("ARK_PORT", "3635"),
                "bitcoind_address": crate::ffi::regtest_url("BITCOIN_PORT", "19443"),
                "bitcoind_user": "second", "bitcoind_pass": "ark",
            }))
            .unwrap();
            let mnemonic = crate::generate_mnemonic().unwrap();
            let onchain = crate::OnchainWallet::default(
                Network::Regtest,
                mnemonic.clone(),
                config.clone(),
                directory.path().to_string_lossy().into(),
            )
            .await
            .unwrap();
            let sender = Wallet::open_with_onchain(
                Network::Regtest,
                mnemonic,
                config.clone(),
                args(directory.path()),
                &onchain,
            )
            .await
            .unwrap();
            let recipient = Wallet::open(
                Network::Regtest,
                crate::generate_mnemonic().unwrap(),
                config,
                args(&directory.path().join("recipient")),
            )
            .await
            .unwrap();
            regtest(&["send-to", &onchain.new_address().await.unwrap(), "0.005"]);
            onchain.sync().await.unwrap();
            sender.board_amount(200_000).await.unwrap();
            regtest(&["generate", "6"]);
            sender.sync().await.unwrap();

            // The recipient never calls sync() until the concurrency check.
            let holder = recipient.notifications();
            recipient.start_mailbox_processor().await.unwrap();
            recipient.start_mailbox_processor().await.unwrap();
            assert_eq!(spawned(&recipient), 1);
            let address = recipient.new_address().await.unwrap();

            let sent = std::time::Instant::now();
            sender
                .send_arkoor_payment(address.clone(), 10_000)
                .await
                .unwrap();
            let send_took = sent.elapsed();
            let (sats, after_send) = arkoor_receive(&holder, Duration::from_secs(5))
                .await
                .expect("no arkoor receive notification within 5s of the send");
            assert_eq!(sats, 10_000);
            assert_eq!(recipient.balance().await.unwrap().spendable_sats, 10_000);
            println!(
                "mailbox processor: receive notified {after_send:?} after the send returned \
                 ({:?} after it started), without sync",
                send_took + after_send
            );

            // Stopped: the next payment waits in the mailbox.
            recipient.stop_mailbox_processor().await.unwrap();
            assert!(!running(&recipient));
            sender
                .send_arkoor_payment(address.clone(), 7_000)
                .await
                .unwrap();
            assert!(
                arkoor_receive(&holder, Duration::from_secs(3))
                    .await
                    .is_none(),
                "a stopped processor still received"
            );
            assert_eq!(recipient.balance().await.unwrap().spendable_sats, 10_000);

            // Restarted: it resumes from the stored checkpoint.
            recipient.start_mailbox_processor().await.unwrap();
            assert_eq!(spawned(&recipient), 2);
            let (sats, restart) = arkoor_receive(&holder, Duration::from_secs(5))
                .await
                .expect("restart did not pick up the waiting payment");
            assert_eq!(sats, 7_000);
            println!("mailbox processor: restart picked up the waiting payment in {restart:?}");

            // Next to sync(): each payment is recorded once.
            for amount in [3_000, 4_000] {
                let send = sender.send_arkoor_payment(address.clone(), amount);
                let (sent, synced) = tokio::join!(send, recipient.sync());
                sent.unwrap();
                synced.unwrap();
            }
            recipient.sync().await.unwrap();
            let receives = recipient
                .history()
                .await
                .unwrap()
                .into_iter()
                .filter(|m| m.subsystem_name == "bark.arkoor" && m.subsystem_kind == "receive")
                .map(|m| m.effective_balance_sats)
                .collect::<Vec<_>>();
            assert_eq!(receives.len(), 4, "{receives:?}");
            assert_eq!(receives.iter().sum::<i64>(), 24_000);
            assert_eq!(recipient.balance().await.unwrap().spendable_sats, 24_000);

            // Dropping the wallet while it runs releases it.
            assert!(running(&recipient));
            let core = std::sync::Arc::downgrade(&recipient.core);
            drop(holder);
            drop(recipient);
            released(core).await;
        });
    }
}
