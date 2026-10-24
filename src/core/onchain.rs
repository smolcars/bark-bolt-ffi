use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Weak};
use tokio::sync::RwLock;

use bark::chain::{ChainSource, ChainSourceSpec};
use bark::onchain::{OnchainWallet as BarkOnchainWallet, OnchainWalletTrait};
use bark::persist::BarkPersister;
use bip39::Mnemonic;

use anyhow::Context;
use log::info;

use crate::config::Config;
use crate::core::parse_address;
use crate::core::prepared_onchain::WalletState;
use crate::error::{BarkError, BarkErrorCode};
use crate::types::{self, OnchainBalance};
use crate::Network;

/// BDK-based onchain Bitcoin wallet for boarding and exits.
///
/// Core implementation for the BDK backend. Callback-backed wallets are layered
/// on top of the core inner `bark::Wallet`.
pub struct OnchainWallet {
    wallet: Arc<RwLock<WalletState>>,
    chain: Arc<ChainSource>,
    /// For [`Self::send`]; neither the bdk wallet nor the trait exposes one.
    network: bitcoin::Network,
}

type LiveWallets = HashMap<PathBuf, Weak<RwLock<WalletState>>>;
static LIVE_WALLETS: LazyLock<tokio::sync::Mutex<LiveWallets>> =
    LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

impl OnchainWallet {
    pub async fn default(
        network: Network,
        mnemonic: String,
        config: Config,
        db: Arc<dyn BarkPersister>,
        registration: Arc<PathBuf>,
    ) -> Result<Self, BarkError> {
        let mnemonic = Mnemonic::parse(mnemonic.trim()).context("invalid mnemonic")?;

        let seed = mnemonic.to_seed("");
        let network = network.into();

        info!(
            "[ONCHAIN] Creating onchain wallet for {:?} network",
            network
        );

        let identity = bitcoin::bip32::Xpub::from_priv(
            &bitcoin::secp256k1::Secp256k1::new(),
            &bitcoin::bip32::Xpriv::new_master(network, &seed).context("invalid wallet seed")?,
        );
        let mut live = LIVE_WALLETS.lock().await;
        live.retain(|_, wallet| wallet.strong_count() > 0);
        let wallet = if let Some(wallet) = live.get(registration.as_ref()).and_then(Weak::upgrade) {
            let existing = wallet.read().await;
            if existing.identity != identity || existing.wallet.network() != network {
                return Err(
                    "onchain wallet is already open with a different network or mnemonic".into(),
                );
            }
            drop(existing);
            wallet
        } else {
            let onchain = BarkOnchainWallet::load_or_create(network, seed, db.clone()).await?;
            let wallet = Arc::new(RwLock::new(WalletState {
                wallet: onchain,
                prepared: HashMap::new(),
                identity,
                db,
                board_databases: Vec::new(),
                operation_gate: Arc::new(tokio::sync::Mutex::new(())),
                _registration: registration.clone(),
            }));
            live.insert(registration.as_ref().clone(), Arc::downgrade(&wallet));
            wallet
        };
        drop(live);

        let chain_spec = if let Some(url) = config.bitcoind_address.as_ref() {
            use bark_bitcoin_ext::rpc::Auth;
            let auth = if let Some(cookie) = config.bitcoind_cookiefile {
                Auth::CookieFile(std::path::PathBuf::from(cookie))
            } else if let (Some(user), Some(pass)) = (config.bitcoind_user, config.bitcoind_pass) {
                Auth::UserPass(user, pass)
            } else {
                Auth::None
            };
            info!("[ONCHAIN] Using Bitcoin Core RPC at {}", url);
            // ZMQ block notifications are configurable upstream via
            // `Config::bitcoind_zmq_address`; the FFI config does not expose it
            // yet, so the chain tip is polled instead.
            ChainSourceSpec::Bitcoind {
                url: url.clone(),
                auth,
                zmq: None,
            }
        } else if let Some(url) = config.esplora_address {
            info!("[ONCHAIN] Using Esplora at {}", url);
            ChainSourceSpec::Esplora { url }
        } else {
            return Err(anyhow::anyhow!(
                "Config must specify either esplora_address or bitcoind_address"
            )
            .into());
        };

        let chain = Arc::new(
            ChainSource::new(chain_spec, network, None)
                .await
                .context("Failed to create chain source")?,
        );

        Ok(Self {
            wallet,
            chain,
            network,
        })
    }

    /// Sign and reserve inputs without inserting a transaction into BDK or SQLite.
    pub async fn prepare_send(
        &self,
        address: String,
        amount: types::OnchainSendAmount,
        fee_rate_sat_per_vb: u64,
        confirmed_only: bool,
    ) -> Result<types::PreparedTransaction, BarkError> {
        let address = parse_address(&address, self.network)
            .map_err(|error| BarkError::typed(BarkErrorCode::InvalidAddress, error.message))?;
        let fee_rate = super::prepared_onchain::fee_rate(fee_rate_sat_per_vb)?;
        let gate = self.wallet.read().await.operation_gate.clone();
        let _operation = gate.lock().await;
        let mut state = self.wallet.write().await;
        let unavailable = state.pending_board_inputs().await?;
        state.prepare(address, amount, fee_rate, confirmed_only, &unavailable)
    }

    /// Persist and submit the exact signed transaction; never rebuild or select coins.
    pub async fn broadcast_prepared(
        &self,
        id: String,
    ) -> Result<types::BroadcastResult, BarkError> {
        use types::BroadcastOutcome;
        let gate = self.wallet.read().await.operation_gate.clone();
        let _operation = gate.lock().await;
        let mut state = self.wallet.write().await;
        let prepared = state.prepared.get(&id).ok_or_else(|| {
            BarkError::typed(
                BarkErrorCode::PreparedTransactionNotFound,
                "unknown prepared transaction id",
            )
        })?;
        if prepared.board.is_some() {
            return Err(BarkError::typed(
                BarkErrorCode::PreparedTransactionNotFound,
                "use submitPreparedBoard for a board preparation",
            ));
        }
        let transaction = prepared.transaction.clone();
        let was_recorded = prepared.recorded;
        let prepared_confirmed_only = prepared.confirmed_only;
        let txid = transaction.compute_txid().to_string();
        let reserved_elsewhere = transaction
            .input
            .iter()
            .any(|input| state.wallet.is_outpoint_locked(input.previous_output) && was_recorded);
        let pending_board_inputs = state.pending_board_inputs().await?;
        if reserved_elsewhere
            || transaction
                .input
                .iter()
                .any(|input| pending_board_inputs.contains(&input.previous_output))
        {
            return Err(BarkError::typed(
                BarkErrorCode::PreparedInputsUnavailable,
                "prepared inputs are now reserved by another transaction",
            ));
        }
        let is_canonical = state.wallet.get_tx(transaction.compute_txid()).is_some();
        if !is_canonical {
            let available: std::collections::HashSet<_> = state
                .wallet
                .list_unspent()
                .into_iter()
                .map(|output| output.outpoint)
                .collect();
            if transaction
                .input
                .iter()
                .any(|input| !available.contains(&input.previous_output))
            {
                return Err(BarkError::typed(
                    BarkErrorCode::PreparedInputsUnavailable,
                    "prepared inputs are no longer unspent; discard and prepare again",
                ));
            }
            // Only the first submission: an evicted, already-recorded tx must stay rebroadcastable.
            if prepared_confirmed_only && !was_recorded {
                for input in &transaction.input {
                    let status = self.chain.tx_status(input.previous_output.txid).await?;
                    if !matches!(status, bark_bitcoin_ext::TxStatus::Confirmed(_)) {
                        return Err(BarkError::typed(
                            BarkErrorCode::PreparedInputsUnavailable,
                            "prepared input parent is no longer confirmed; discard and prepare again",
                        ));
                    }
                }
            }
            if let Err(error) = state.record_prepared(&transaction).await {
                return Ok(types::BroadcastResult {
                    txid,
                    broadcast: BroadcastOutcome::NotAttempted {
                        reason: format!("failed to persist transaction: {error:#}"),
                    },
                });
            }
            state
                .prepared
                .get_mut(&id)
                .expect("prepared id held under lock")
                .recorded = true;
            state.release_inputs(&transaction);
        }
        let broadcast = match self.chain.broadcast_tx(&transaction).await {
            Ok(()) => BroadcastOutcome::Accepted,
            Err(error) => BroadcastOutcome::Failed {
                reason: format!("{error:#}"),
            },
        };
        Ok(types::BroadcastResult { txid, broadcast })
    }

    /// Before broadcast this releases inputs. After recording it only forgets the id.
    pub async fn discard_prepared(&self, id: String) -> Result<(), BarkError> {
        let gate = self.wallet.read().await.operation_gate.clone();
        let _operation = gate.lock().await;
        let mut state = self.wallet.write().await;
        if state
            .prepared
            .get(&id)
            .is_some_and(|entry| entry.board.is_some())
        {
            return Err(BarkError::typed(
                BarkErrorCode::PreparedTransactionNotFound,
                "use discardPreparedBoard for a board preparation",
            ));
        }
        let prepared = state.prepared.remove(&id).ok_or_else(|| {
            BarkError::typed(
                BarkErrorCode::PreparedTransactionNotFound,
                "unknown prepared transaction id",
            )
        })?;
        if !prepared.recorded {
            state.release_inputs(&prepared.transaction);
        }
        Ok(())
    }

    pub async fn transaction_status(
        &self,
        txid: String,
    ) -> Result<types::TransactionStatus, BarkError> {
        Ok(self
            .chain
            .tx_status(txid.parse().context("invalid transaction id")?)
            .await?
            .into())
    }

    pub async fn sync(&self) -> Result<u64, BarkError> {
        let mut w = self.wallet.write().await;
        w.sync(&self.chain).await.context("Sync failed")?;
        let balance = w.wallet.balance();
        Ok(balance.total().to_sat())
    }

    /// Discover the wallet's pre-existing on-chain history.
    ///
    /// `sync` only checks addresses this wallet instance has already revealed,
    /// so a wallet restored from a mnemonic never finds transactions made by
    /// its previous incarnation. This runs bark's initial scan instead: a
    /// gap-limited full scan on esplora (`birthday_height` is ignored there),
    /// or a block scan from `birthday_height` on bitcoind. Run it once after
    /// restoring; returns the total balance in sats afterwards.
    pub async fn initial_scan(&self, birthday_height: Option<u32>) -> Result<u64, BarkError> {
        let mut w = self.wallet.write().await;
        let balance = w
            .initial_wallet_scan(&self.chain, birthday_height.map(Into::into))
            .await
            .context("Initial wallet scan failed")?;
        Ok(balance.to_sat())
    }

    pub async fn balance(&self) -> Result<OnchainBalance, BarkError> {
        let w = self.wallet.write().await;
        Ok(w.wallet.balance().into())
    }

    pub async fn new_address(&self) -> Result<String, BarkError> {
        let mut w = self.wallet.write().await;
        let addr = w.address().await.context("Failed to generate address")?;
        Ok(addr.to_string())
    }

    pub async fn send(
        &self,
        address: String,
        amount_sats: u64,
        fee_rate_sat_per_vb: u64,
    ) -> Result<String, BarkError> {
        let addr = parse_address(&address, self.network)?;

        let amount = bitcoin::Amount::from_sat(amount_sats);

        let fee_rate =
            bitcoin::FeeRate::from_sat_per_vb(fee_rate_sat_per_vb).context("Invalid fee rate")?;

        let mut w = self.wallet.write().await;
        let txid = w
            .send(&self.chain, addr, amount, fee_rate)
            .await
            .context("Send failed")?;

        Ok(txid.to_string())
    }

    pub async fn is_mine(&self, address: String) -> Result<bool, BarkError> {
        // Ownership is a script query, regardless of the address's network (Nitro).
        let address: bitcoin::Address<bitcoin::address::NetworkUnchecked> =
            address.parse().context("invalid address")?;
        Ok(self
            .wallet
            .read()
            .await
            .is_mine(&address.assume_checked().script_pubkey())
            .await?)
    }

    /// True BDK drain; the optional rate is sat/vB, otherwise the chain's regular rate.
    pub async fn drain(
        &self,
        address: String,
        fee_rate_sat_per_vb: Option<u64>,
    ) -> Result<String, BarkError> {
        let address = parse_address(&address, self.network)?;
        let fee_rate = match fee_rate_sat_per_vb {
            Some(rate) => {
                bitcoin::FeeRate::from_sat_per_vb(rate).context("invalid fee rate in sat/vB")?
            }
            None => self.chain.fee_rates().await.regular,
        };
        let txid = self
            .wallet
            .write()
            .await
            .drain(&self.chain, address, fee_rate)
            .await
            .context("drain failed")?;
        Ok(txid.to_string())
    }

    /// Mark a wallet-known transaction as evicted from the mempool, so its
    /// inputs return to coin selection immediately instead of waiting for the
    /// sync eviction grace period. Only call this on a tx that has definitively
    /// been superseded on-chain (e.g. an exit CPFP that was RBF-replaced);
    /// evicting a still-in-flight tx invites a self-inflicted double-spend.
    pub async fn evict_tx(&self, txid: String) -> Result<(), BarkError> {
        let txid = txid.parse::<bitcoin::Txid>().context("invalid txid")?;
        let mut w = self.wallet.write().await;
        w.evict_tx(txid).await.context("Evict tx failed")?;
        Ok(())
    }

    /// Current chain tip height, from the wallet's chain source (cached
    /// upstream with a short TTL).
    pub async fn tip_height(&self) -> Result<u32, BarkError> {
        let height = self.chain.tip().await.context("Failed to fetch tip")?;
        Ok(height.into())
    }

    /// Cached network fee-rate estimates from the wallet's chain source.
    pub async fn fee_rates(&self) -> Result<types::FeeRates, BarkError> {
        Ok(self.chain.fee_rates().await.into())
    }

    /// Every wallet transaction with fee, balance change, confirmation and
    /// CPFP flag. Requires a prior `sync` to be meaningful.
    pub async fn transactions(&self) -> Result<Vec<types::WalletTransaction>, BarkError> {
        let w = self.wallet.read().await;
        let infos = w
            .list_transaction_infos()
            .context("Failed to list transactions")?;
        Ok(infos.iter().map(Into::into).collect())
    }

    /// The wallet's unspent outputs. Requires a prior `sync` to be meaningful.
    pub async fn utxos(&self) -> Result<Vec<types::OnchainUtxo>, BarkError> {
        let w = self.wallet.read().await;
        Ok(w.utxos().iter().map(Into::into).collect())
    }

    pub(crate) fn inner(&self) -> Arc<RwLock<dyn OnchainWalletTrait>> {
        self.wallet.clone()
    }
}
