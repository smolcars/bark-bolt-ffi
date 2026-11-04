pub mod custom_onchain_wallet;
pub mod db;
pub mod logger;
pub(crate) mod mailbox;
pub mod onchain;
pub mod runtime;
pub mod wallet;

pub use crate::core::notification::NotificationHolder;
pub use crate::core::state_changes::{StateChangeEvent, StateChangeSubscription};
pub use custom_onchain_wallet::CustomOnchainWalletCallbacks;
pub use logger::{clear_logger, set_logger, BarkLogger};
pub use onchain::OnchainWallet;
pub use wallet::Wallet;

/// Live-test RegTest endpoint; `ARK_PORT`/`BITCOIN_PORT` select an isolated stack.
#[cfg(test)]
pub(crate) fn regtest_url(port_var: &str, default: &str) -> String {
    format!(
        "http://127.0.0.1:{}",
        std::env::var(port_var).unwrap_or(default.into())
    )
}
