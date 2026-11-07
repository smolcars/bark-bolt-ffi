use std::sync::Arc;
use tokio::sync::RwLock;

use bark::onchain::OnchainWalletTrait;
use log::info;

use crate::config::Config;
use crate::core::onchain::OnchainWallet as CoreOnchainWallet;
use crate::error::BarkError;
use crate::ffi::custom_onchain_wallet::{CallbackWalletAdapter, CustomOnchainWalletCallbacks};
use crate::ffi::db::open_sqlite_db;
use crate::ffi::runtime::run_async;
use crate::types::OnchainBalance;
use crate::Network;

/// BoltFFI-facing onchain wallet. Supports two backends:
/// - BDK (real onchain wallet via `bark::onchain::OnchainWallet`)
/// - Callback (foreign-language implementation)
pub struct OnchainWallet {
    inner: OnchainWalletInner,
}

enum OnchainWalletInner {
    Bdk(Arc<CoreOnchainWallet>),
    Callback(Arc<RwLock<CallbackWalletAdapter>>),
}

/// A cancelled foreign wait must not lose the only way to release reserved inputs.
struct PreparedHandoff {
    transaction: Option<crate::types::PreparedTransaction>,
    core: Arc<CoreOnchainWallet>,
}

impl Drop for PreparedHandoff {
    fn drop(&mut self) {
        if let Some(transaction) = self.transaction.take() {
            let core = self.core.clone();
            crate::ffi::runtime::TOKIO_RT.spawn(async move {
                let _ = core.discard_prepared(transaction.id).await;
            });
        }
    }
}

#[boltffi::export]
impl OnchainWallet {
    /// BDK-backed wallet. Opens the shared sqlite cache.
    pub async fn default(
        network: Network,
        mnemonic: String,
        config: Config,
        datadir: String,
    ) -> Result<Self, BarkError> {
        run_async(async move {
            let db = open_sqlite_db(&datadir)?;
            let registration =
                crate::backup::register_live_database(&crate::ffi::db::sqlite_path(&datadir))?;
            let core =
                CoreOnchainWallet::default(network, mnemonic, config, db, registration).await?;
            Ok(Self {
                inner: OnchainWalletInner::Bdk(Arc::new(core)),
            })
        })
        .await
    }

    /// Open an initialized wallet, validating network and mnemonic before any writes.
    pub async fn open_existing(
        network: Network,
        mnemonic: String,
        config: Config,
        datadir: String,
    ) -> Result<Self, BarkError> {
        run_async(async move {
            crate::ffi::db::validate_existing_wallet(&datadir, network, &mnemonic)?;
            Self::default(network, mnemonic, config, datadir).await
        })
        .await
    }

    /// Callback-backed wallet for foreign-language implementations.
    pub fn custom(callbacks: Arc<dyn CustomOnchainWalletCallbacks>) -> Result<Self, BarkError> {
        info!("[ONCHAIN] Creating callback-based onchain wallet");
        let adapter = CallbackWalletAdapter::new(callbacks);
        Ok(Self {
            inner: OnchainWalletInner::Callback(Arc::new(RwLock::new(adapter))),
        })
    }

    /// Query the chain source directly; network failures remain errors.
    pub async fn transaction_status(
        &self,
        txid: String,
    ) -> Result<crate::types::TransactionStatus, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.transaction_status(txid).await }).await
            }
            OnchainWalletInner::Callback(_) => Err(BarkError::typed(
                crate::BarkErrorCode::UnsupportedOperation,
                "transactionStatus requires the BDK chain source",
            )),
        }
    }

    pub async fn sync(&self) -> Result<u64, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.sync().await }).await
            }
            OnchainWalletInner::Callback(_) => {
                info!("[ONCHAIN] Callback wallets manage their own sync");
                Ok(0)
            }
        }
    }

    /// Discover the wallet's pre-existing on-chain history. Run once after
    /// restoring a wallet from a mnemonic: `sync` only covers addresses this
    /// wallet instance has already revealed, so it never finds transactions
    /// made by a previous incarnation. Gap-limited full scan on esplora
    /// (`birthday_height` is ignored there), block scan from `birthday_height`
    /// on bitcoind. Returns the total balance in sats afterwards.
    pub async fn initial_scan(&self, birthday_height: Option<u32>) -> Result<u64, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.initial_scan(birthday_height).await }).await
            }
            OnchainWalletInner::Callback(_) => {
                Err("initial_scan() not supported for callback wallets".into())
            }
        }
    }

    pub async fn balance(&self) -> Result<OnchainBalance, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.balance().await }).await
            }
            OnchainWalletInner::Callback(adapter) => {
                let adapter = adapter.clone();
                run_async(async move {
                    let a = adapter.read().await;
                    let amount = a.balance().await;
                    Ok(OnchainBalance {
                        confirmed_sats: amount.to_sat(),
                        pending_sats: 0,
                        total_sats: amount.to_sat(),
                    })
                })
                .await
            }
        }
    }

    pub async fn new_address(&self) -> Result<String, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.new_address().await }).await
            }
            OnchainWalletInner::Callback(_) => {
                Err("new_address() not supported for callback wallets".into())
            }
        }
    }

    /// The lowest revealed address no wallet transaction pays; reveals and saves a new
    /// one only when all are used. Repeated calls return the same address without writing.
    ///
    /// "Used" is as of the last sync, unconfirmed payments included. The wallet sends its
    /// change to this same address, so a send can use it up.
    pub async fn next_unused_address(&self) -> Result<String, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.next_unused_address().await }).await
            }
            OnchainWalletInner::Callback(_) => {
                Err("next_unused_address() not supported for callback wallets".into())
            }
        }
    }

    pub async fn send(
        &self,
        address: String,
        amount_sats: u64,
        fee_rate_sat_per_vb: u64,
    ) -> Result<String, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.send(address, amount_sats, fee_rate_sat_per_vb).await })
                    .await
            }
            OnchainWalletInner::Callback(_) => {
                Err("send() not supported for callback wallets".into())
            }
        }
    }

    /// Build and sign, returning the exact fee and reserving inputs across handles/daemon sync.
    /// The id lasts until discard or all onchain/daemon handles are dropped; it is not backed up.
    pub async fn prepare_send(
        &self,
        address: String,
        amount: crate::types::OnchainSendAmount,
        fee_rate_sat_per_vb: u64,
        confirmed_only: bool,
    ) -> Result<crate::types::PreparedTransaction, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                let mut handoff = run_async(async move {
                    let transaction = core
                        .prepare_send(address, amount, fee_rate_sat_per_vb, confirmed_only)
                        .await?;
                    Ok::<_, BarkError>(PreparedHandoff {
                        transaction: Some(transaction),
                        core,
                    })
                })
                .await?;
                Ok(handoff
                    .transaction
                    .take()
                    .expect("prepared transaction handoff"))
            }
            OnchainWalletInner::Callback(_) => Err(BarkError::typed(
                crate::BarkErrorCode::UnsupportedOperation,
                "prepareSend requires the BDK onchain wallet",
            )),
        }
    }

    /// Broadcast exactly the prepared bytes. Failed submission remains persisted for sync retry.
    pub async fn broadcast_prepared(
        &self,
        id: String,
    ) -> Result<crate::types::BroadcastResult, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.broadcast_prepared(id).await }).await
            }
            OnchainWalletInner::Callback(_) => Err(BarkError::typed(
                crate::BarkErrorCode::UnsupportedOperation,
                "broadcastPrepared requires the BDK onchain wallet",
            )),
        }
    }

    /// Release an unbroadcast preparation. This cannot cancel a transaction already recorded.
    pub async fn discard_prepared(&self, id: String) -> Result<(), BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.discard_prepared(id).await }).await
            }
            OnchainWalletInner::Callback(_) => Err(BarkError::typed(
                crate::BarkErrorCode::UnsupportedOperation,
                "discardPrepared requires the BDK onchain wallet",
            )),
        }
    }

    /// Whether this wallet owns the scriptPubkey. Valid addresses from any network
    /// are accepted for this read-only query, matching Nitro.
    pub async fn is_mine(&self, address: String) -> Result<bool, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.is_mine(address).await }).await
            }
            OnchainWalletInner::Callback(adapter) => {
                let adapter = adapter.clone();
                run_async(async move {
                    let address: bitcoin::Address<bitcoin::address::NetworkUnchecked> =
                        address.parse().map_err(anyhow::Error::from)?;
                    Ok(adapter
                        .read()
                        .await
                        .is_mine(&address.assume_checked().script_pubkey())
                        .await?)
                })
                .await
            }
        }
    }

    /// Drain all BDK-selected funds. Fee rate is sat/vB; nil/null uses regular.
    pub async fn drain(
        &self,
        address: String,
        #[boltffi::default(None)] fee_rate_sat_per_vb: Option<u64>,
    ) -> Result<String, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.drain(address, fee_rate_sat_per_vb).await }).await
            }
            OnchainWalletInner::Callback(_) => {
                Err("drain() not supported for callback wallets".into())
            }
        }
    }

    /// Mark a wallet-known transaction as evicted from the mempool, so its
    /// inputs return to coin selection immediately. Only for a tx that has
    /// definitively been superseded on-chain (e.g. an RBF-replaced exit CPFP);
    /// evicting a still-in-flight tx invites a self-inflicted double-spend.
    pub async fn evict_tx(&self, txid: String) -> Result<(), BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.evict_tx(txid).await }).await
            }
            OnchainWalletInner::Callback(adapter) => {
                let adapter = adapter.clone();
                run_async(async move {
                    let txid = txid
                        .parse::<bitcoin::Txid>()
                        .map_err(|e| BarkError::from(format!("invalid txid: {}", e)))?;
                    let mut a = adapter.write().await;
                    a.evict_tx(txid).await.map_err(BarkError::from)
                })
                .await
            }
        }
    }

    /// Current chain tip height from the wallet's chain source.
    pub async fn tip_height(&self) -> Result<u32, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.tip_height().await }).await
            }
            OnchainWalletInner::Callback(_) => {
                Err("tip_height() not supported for callback wallets".into())
            }
        }
    }

    /// Cached network fee-rate estimates from the wallet's chain source.
    pub async fn fee_rates(&self) -> Result<crate::types::FeeRates, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.fee_rates().await }).await
            }
            OnchainWalletInner::Callback(_) => {
                Err("fee_rates() not supported for callback wallets".into())
            }
        }
    }

    /// Every wallet transaction with fee, balance change, confirmation and
    /// CPFP flag. Requires a prior `sync` to be meaningful.
    pub async fn transactions(&self) -> Result<Vec<crate::types::WalletTransaction>, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.transactions().await }).await
            }
            OnchainWalletInner::Callback(_) => {
                Err("transactions() not supported for callback wallets".into())
            }
        }
    }

    /// The wallet's unspent outputs. Requires a prior `sync` to be meaningful.
    pub async fn utxos(&self) -> Result<Vec<crate::types::OnchainUtxo>, BarkError> {
        match &self.inner {
            OnchainWalletInner::Bdk(core) => {
                let core = core.clone();
                run_async(async move { core.utxos().await }).await
            }
            OnchainWalletInner::Callback(_) => {
                Err("utxos() not supported for callback wallets".into())
            }
        }
    }
}

impl OnchainWallet {
    pub(crate) fn inner_dyn(&self) -> Arc<RwLock<dyn OnchainWalletTrait>> {
        match &self.inner {
            OnchainWalletInner::Bdk(v) => v.inner(),
            OnchainWalletInner::Callback(v) => v.clone(),
        }
    }
}

#[cfg(test)]
mod prepared_cancellation_tests {
    use super::*;
    use crate::types::OnchainSendAmount;

    #[test]
    fn cancelled_foreign_wait_discards_unclaimed_preparation() {
        if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
            return;
        }
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let config: Config = serde_json::from_value(serde_json::json!({
                "server_address":crate::ffi::regtest_url("ARK_PORT", "3635"), "bitcoind_address":crate::ffi::regtest_url("BITCOIN_PORT", "19443"),
                "bitcoind_user":"second", "bitcoind_pass":"ark",
            })).unwrap();
            let onchain = OnchainWallet::default(Network::Regtest, crate::generate_mnemonic().unwrap(), config, directory.path().to_str().unwrap().into()).await.unwrap();
            let address = onchain.new_address().await.unwrap();
            let funding = std::process::Command::new("bash").args(["scripts/ark-dev.sh", "send-to", &address, "0.001"]).output().unwrap();
            assert!(funding.status.success(), "{}", String::from_utf8_lossy(&funding.stderr));
            onchain.sync().await.unwrap();
            let OnchainWalletInner::Bdk(core) = &onchain.inner else { unreachable!() };
            let core = core.clone();
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
            let waiter = tokio::spawn(run_async(async move {
                let transaction = core.prepare_send(address, OnchainSendAmount::DrainAll, 10, true).await.unwrap();
                ready_tx.send(()).unwrap();
                finish_rx.await.unwrap();
                PreparedHandoff { transaction: Some(transaction), core }
            }));
            ready_rx.await.unwrap();
            waiter.abort();
            let _ = waiter.await;
            finish_tx.send(()).unwrap();
            let destination = onchain.new_address().await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    match onchain.prepare_send(destination.clone(), OnchainSendAmount::DrainAll, 10, true).await {
                        Ok(prepared) => { onchain.discard_prepared(prepared.id).await.unwrap(); break; }
                        Err(error) => {
                            assert_eq!(error.code, crate::BarkErrorCode::InsufficientFunds);
                            tokio::task::yield_now().await;
                        }
                    }
                }
            }).await.expect("cancelled preparation retained its inputs");
        });
    }
}
