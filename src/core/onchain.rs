use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Weak};
use tokio::sync::RwLock;

use bark::chain::{ChainSource, ChainSourceSpec};
use bark::onchain::bdk_wallet::{self, chain::ChainPosition};
use bark::onchain::{OnchainWallet as BarkOnchainWallet, OnchainWalletTrait};
use bark::persist::BarkPersister;
use bdk_esplora::{esplora_client, EsploraAsyncExt};
use bip39::Mnemonic;

use anyhow::Context;
use log::{info, warn};

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
    /// Same server as `chain`, for [`Self::sync`] and [`Self::initial_scan`]; `None` on bitcoind.
    esplora: Option<esplora_client::AsyncClient>,
    /// For [`Self::send`]; neither the bdk wallet nor the trait exposes one.
    network: bitcoin::Network,
}

// bark's `OnchainWallet::sync` and `initial_wallet_scan` constants (private upstream).
const STOP_GAP: usize = 50;
const PARALLEL_REQS: usize = 4;
const ONCHAIN_EVICTION_GRACE_SECS: u64 = 30;
/// Fetches per sync or scan when bark's own sync keeps moving the tip; see [`OnchainWallet::apply`].
const FETCH_ATTEMPTS: usize = 3;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
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
                sync_gate: Arc::new(tokio::sync::Mutex::new(())),
                _registration: registration.clone(),
            }));
            live.insert(registration.as_ref().clone(), Arc::downgrade(&wallet));
            wallet
        };
        drop(live);

        let mut esplora = None;
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
            // The URL normalisation bark's `ChainSource::new` applies.
            let trimmed = url.trim();
            let base = if trimmed.contains("://") {
                trimmed.to_owned()
            } else {
                format!("https://{trimmed}")
            };
            esplora = Some(
                esplora_client::Builder::new(base.strip_suffix('/').unwrap_or(&base))
                    .build_async()
                    .context("Failed to create esplora client")?,
            );
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
            esplora,
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

    /// On Esplora the fetch holds no wallet lock, so reads never wait for the network.
    /// Bitcoind keeps bark's sync, which applies blocks as it streams them.
    pub async fn sync(&self) -> Result<u64, BarkError> {
        let Some(esplora) = &self.esplora else {
            let mut w = self.wallet.write().await;
            w.sync(&self.chain).await.context("Sync failed")?;
            return Ok(w.wallet.balance().total().to_sat());
        };
        let gate = self.wallet.read().await.sync_gate.clone();
        let _sync = gate.lock().await;
        for _ in 0..FETCH_ATTEMPTS {
            // The request bark's `OnchainWallet::sync` builds.
            let (tip, request) = {
                let w = self.wallet.read().await;
                let tip = w.wallet.latest_checkpoint();
                let min_height = tip
                    .height()
                    .saturating_sub(bark_bitcoin_ext::DEEPLY_CONFIRMED.to_u32());
                let request = w
                    .wallet
                    .start_sync_with_revealed_spks_at(now_secs())
                    .outpoints(w.wallet.list_unspent().iter().map(|o| o.outpoint))
                    .txids(w.wallet.transactions().filter_map(|tx| {
                        let fresh = match tx.chain_position {
                            ChainPosition::Unconfirmed { .. } => true,
                            ChainPosition::Confirmed { anchor, .. } => {
                                anchor.block_id.height >= min_height
                            }
                        };
                        fresh.then_some(tx.tx_node.txid)
                    }));
                (tip.block_id(), request)
            };
            let update = esplora
                .sync(request, PARALLEL_REQS)
                .await
                .context("Sync failed")?;
            if let Some(balance) = self.apply(tip, update.into()).await? {
                return Ok(balance);
            }
        }
        Err(anyhow::anyhow!("Sync failed: the chain tip moved during every fetch").into())
    }

    /// Apply a fetched update and persist it under the write lock, then rebroadcast
    /// unconfirmed transactions without it, as bark's sync does.
    ///
    /// `None` if the tip is no longer `tip`, the one the update was fetched from. Only a sync
    /// run by bark (its daemon, or a bitcoind handle) can move it, and that chain may be newer.
    /// Merging this update could undo a reorg, and dropping its checkpoints would leave its
    /// anchored transactions unconfirmed, so the caller fetches again from the new tip.
    async fn apply(
        &self,
        tip: bdk_wallet::chain::BlockId,
        mut update: bdk_wallet::Update,
    ) -> Result<Option<u64>, BarkError> {
        let (balance, unconfirmed) = {
            let mut w = self.wallet.write().await;
            if w.wallet.latest_checkpoint().block_id() != tip {
                return Ok(None);
            }
            // Esplora can lag a just-broadcast tx; bark keeps it for a grace period.
            let now = now_secs();
            let recent: HashSet<bitcoin::Txid> = w
                .wallet
                .transactions()
                .filter_map(|tx| match tx.chain_position {
                    ChainPosition::Unconfirmed {
                        last_seen: Some(seen),
                        ..
                    } if now.saturating_sub(seen) < ONCHAIN_EVICTION_GRACE_SECS => {
                        Some(tx.tx_node.txid)
                    }
                    _ => None,
                })
                .collect();
            update
                .tx_update
                .evicted_ats
                .retain(|(txid, _)| !recent.contains(txid));
            w.wallet.apply_update(update).context("Sync failed")?;
            if let Some(stage) = w.wallet.staged() {
                w.db.store_bdk_wallet_changeset(stage).await?;
                w.wallet.take_staged();
            }
            let now = now_secs();
            let unconfirmed: Vec<_> = w
                .wallet
                .transactions()
                .filter(|tx| {
                    matches!(tx.chain_position, ChainPosition::Unconfirmed { last_seen, .. }
                        if last_seen.is_none_or(|seen| seen < now))
                })
                .map(|tx| tx.tx_node.tx)
                .collect();
            (w.wallet.balance().total().to_sat(), unconfirmed)
        };
        for tx in unconfirmed {
            if let Err(error) = self.chain.broadcast_tx(&tx).await {
                warn!("Error broadcasting tx {}: {error:#}", tx.compute_txid());
            }
        }
        Ok(Some(balance))
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
        let Some(esplora) = &self.esplora else {
            let mut w = self.wallet.write().await;
            let balance = w
                .initial_wallet_scan(&self.chain, birthday_height.map(Into::into))
                .await
                .context("Initial wallet scan failed")?;
            return Ok(balance.to_sat());
        };
        let gate = self.wallet.read().await.sync_gate.clone();
        let _sync = gate.lock().await;
        for _ in 0..FETCH_ATTEMPTS {
            let (tip, request) = {
                let w = self.wallet.read().await;
                (
                    w.wallet.latest_checkpoint().block_id(),
                    w.wallet.start_full_scan_at(now_secs()),
                )
            };
            let update = esplora
                .full_scan(request, STOP_GAP, PARALLEL_REQS)
                .await
                .context("Initial wallet scan failed")?;
            if let Some(balance) = self.apply(tip, update.into()).await? {
                return Ok(balance);
            }
        }
        Err(
            anyhow::anyhow!("Initial wallet scan failed: the chain tip moved during every fetch")
                .into(),
        )
    }

    pub async fn balance(&self) -> Result<OnchainBalance, BarkError> {
        let w = self.wallet.read().await;
        Ok(w.wallet.balance().into())
    }

    pub async fn new_address(&self) -> Result<String, BarkError> {
        let mut w = self.wallet.write().await;
        let addr = w.address().await.context("Failed to generate address")?;
        Ok(addr.to_string())
    }

    /// The lowest revealed address no transaction in the wallet's graph pays; reveals and
    /// persists a new one only when every revealed address is used. "Used" is as of the last
    /// sync: a payment the chain source has not reported yet does not count.
    pub async fn next_unused_address(&self) -> Result<String, BarkError> {
        let mut w = self.wallet.write().await;
        let external = bdk_wallet::KeychainKind::External;
        let revealed = w.wallet.derivation_index(external);
        let address = w.wallet.next_unused_address(external).address;
        if w.wallet.derivation_index(external) != revealed {
            if let Some(stage) = w.wallet.staged() {
                w.db.store_bdk_wallet_changeset(stage).await?;
                w.wallet.take_staged();
            }
        }
        Ok(address.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::{sha256, Hash};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// An Esplora stub that holds the first `/blocks`, a fetch's first request, until released.
    struct StubEsplora {
        url: String,
        /// Bodies of `/scripthash/{hash}/txs` by path; other scripts have no history.
        txs: Arc<Mutex<HashMap<String, String>>>,
        entered: tokio::sync::mpsc::UnboundedReceiver<()>,
        release: std::sync::mpsc::Sender<()>,
    }

    /// `blocks` (height, hash) are served by `/blocks` along with genesis.
    fn stub_esplora(blocks: Vec<(u32, bitcoin::BlockHash)>) -> StubEsplora {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let txs = Arc::new(Mutex::new(HashMap::new()));
        let (entered_tx, entered) = tokio::sync::mpsc::unbounded_channel();
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let stalled = Arc::new(AtomicBool::new(false));
        let served = txs.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let (mut stream, txs, entered_tx, release_rx, stalled, blocks) = (
                    stream.unwrap(),
                    served.clone(),
                    entered_tx.clone(),
                    release_rx.clone(),
                    stalled.clone(),
                    blocks.clone(),
                );
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        let read = stream.read(&mut buffer).unwrap();
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                    }
                    let request = String::from_utf8_lossy(&request);
                    let path = request.split_whitespace().nth(1).unwrap_or_default();
                    let genesis =
                        bitcoin::constants::genesis_block(bitcoin::Network::Regtest).block_hash();
                    let (status, body) = if path == "/block-height/0" {
                        ("200 OK", genesis.to_string())
                    } else if path == "/blocks" {
                        if !stalled.swap(true, Ordering::SeqCst) {
                            entered_tx.send(()).unwrap();
                            let _ = release_rx.lock().unwrap().recv();
                        }
                        let info = |&(height, hash): &(u32, bitcoin::BlockHash)| {
                            serde_json::json!({
                                "id": hash, "height": height, "version": 1, "timestamp": 1,
                                "tx_count": 1, "size": 1, "weight": 4,
                                "merkle_root": "00".repeat(32), "previousblockhash": null,
                                "mediantime": 1, "nonce": 0, "bits": 0, "difficulty": 1.0,
                            })
                        };
                        let all: Vec<_> = blocks.iter().copied().chain([(0, genesis)]).collect();
                        (
                            "200 OK",
                            serde_json::Value::from_iter(all.iter().map(info)).to_string(),
                        )
                    } else if path.starts_with("/scripthash/") && path.ends_with("/txs") {
                        let body = txs.lock().unwrap().get(path).cloned();
                        ("200 OK", body.unwrap_or_else(|| "[]".into()))
                    } else {
                        ("404 Not Found", String::new())
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        StubEsplora {
            url,
            txs,
            entered,
            release,
        }
    }

    /// Serve a 50,000 sat payment to `script`, confirmed at height 1 in `block`.
    fn serve_payment(
        esplora: &StubEsplora,
        script: &bitcoin::Script,
        block: bitcoin::BlockHash,
    ) -> bitcoin::Txid {
        let payment = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: bitcoin::OutPoint {
                    txid: bitcoin::Txid::from_byte_array([9; 32]),
                    vout: 0,
                },
                ..Default::default()
            }],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(50_000),
                script_pubkey: script.to_owned(),
            }],
        };
        let txid = payment.compute_txid();
        let body = serde_json::json!([{
            "txid": txid, "version": 2, "locktime": 0,
            "vin": [{
                "txid": payment.input[0].previous_output.txid, "vout": 0, "prevout": null,
                "scriptsig": "", "witness": [], "sequence": u32::MAX, "is_coinbase": false,
            }],
            "vout": [{ "value": 50_000, "scriptpubkey": script }],
            "size": payment.total_size(), "weight": payment.weight().to_wu(),
            "status": {
                "confirmed": true, "block_height": 1, "block_hash": block, "block_time": 1,
            },
            "fee": 0,
        }]);
        // esplora-client's scripthash path.
        let path = format!(
            "/scripthash/{:x}/txs",
            sha256::Hash::hash(script.as_bytes())
        );
        esplora.txs.lock().unwrap().insert(path, body.to_string());
        txid
    }

    async fn open_wallet(
        esplora: &StubEsplora,
    ) -> (
        Arc<OnchainWallet>,
        Arc<dyn BarkPersister>,
        tempfile::TempDir,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let db = crate::ffi::db::open_sqlite_db(directory.path().to_str().unwrap()).unwrap();
        let registration =
            crate::backup::register_live_database(&directory.path().join("db.sqlite")).unwrap();
        let config: Config = serde_json::from_value(serde_json::json!({
            "server_address": "http://127.0.0.1:1", "esplora_address": esplora.url,
        }))
        .unwrap();
        let mnemonic = crate::generate_mnemonic().unwrap();
        let onchain =
            OnchainWallet::default(Network::Regtest, mnemonic, config, db.clone(), registration)
                .await
                .unwrap();
        (Arc::new(onchain), db, directory)
    }

    #[test]
    fn reads_do_not_wait_for_a_sync_fetch() {
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let block = bitcoin::BlockHash::from_byte_array([1; 32]);
            let mut esplora = stub_esplora(vec![(1, block)]);
            let (onchain, db, _directory) = open_wallet(&esplora).await;
            let address = onchain.new_address().await.unwrap();
            let script = parse_address(&address, bitcoin::Network::Regtest)
                .unwrap()
                .script_pubkey();
            let txid = serve_payment(&esplora, &script, block);

            let syncing = tokio::spawn({
                let onchain = onchain.clone();
                async move { onchain.sync().await }
            });
            esplora.entered.recv().await.unwrap();
            let (balance, transactions, utxos) =
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    (
                        onchain.balance().await.unwrap(),
                        onchain.transactions().await.unwrap(),
                        onchain.utxos().await.unwrap(),
                    )
                })
                .await
                .expect("a read waited for the sync's network fetch");
            assert_eq!(balance.total_sats, 0);
            assert!(transactions.is_empty() && utxos.is_empty());

            esplora.release.send(()).unwrap();
            assert_eq!(syncing.await.unwrap().unwrap(), 50_000);
            assert_eq!(onchain.balance().await.unwrap().confirmed_sats, 50_000);
            let transactions = onchain.transactions().await.unwrap();
            assert_eq!(transactions.len(), 1);
            assert_eq!(transactions[0].txid, txid.to_string());
            let persisted = bark::onchain::bdk_wallet::Wallet::load()
                .load_wallet_no_persist(db.initialize_bdk_wallet().await.unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(persisted.balance().confirmed.to_sat(), 50_000);
        });
    }

    #[test]
    fn next_unused_address_waits_for_a_payment() {
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let block = bitcoin::BlockHash::from_byte_array([1; 32]);
            let esplora = stub_esplora(vec![(1, block)]);
            esplora.release.send(()).unwrap();
            let (onchain, db, _directory) = open_wallet(&esplora).await;
            let external = bdk_wallet::KeychainKind::External;
            let reload = || async {
                bark::onchain::bdk_wallet::Wallet::load()
                    .load_wallet_no_persist(db.initialize_bdk_wallet().await.unwrap())
                    .unwrap()
                    .unwrap()
            };

            let first = onchain.next_unused_address().await.unwrap();
            assert_eq!(first, onchain.next_unused_address().await.unwrap());
            assert!(onchain.wallet.read().await.wallet.staged().is_none());
            assert_eq!(reload().await.derivation_index(external), Some(0));

            let script = parse_address(&first, bitcoin::Network::Regtest)
                .unwrap()
                .script_pubkey();
            serve_payment(&esplora, &script, block);
            onchain.sync().await.unwrap();
            let second = onchain.next_unused_address().await.unwrap();
            assert_ne!(first, second);
            // Used-ness is rebuilt from the persisted graph; the reveal was persisted.
            let mut reopened = reload().await;
            assert_eq!(reopened.derivation_index(external), Some(1));
            assert_eq!(
                reopened.next_unused_address(external).address.to_string(),
                second
            );

            // Bark's wallet is single-descriptor, so change takes the lowest unused address:
            // the one just shown for receiving.
            let secp = bitcoin::secp256k1::Secp256k1::new();
            let key = bitcoin::secp256k1::Keypair::new(
                &secp,
                &mut bitcoin::secp256k1::rand::thread_rng(),
            );
            let other = bitcoin::Address::p2tr(
                &secp,
                key.x_only_public_key().0,
                None,
                bitcoin::KnownHrp::Regtest,
            )
            .to_string();
            let prepared = onchain
                .prepare_send(
                    other,
                    types::OnchainSendAmount::Amount {
                        amount_sats: 10_000,
                    },
                    1,
                    false,
                )
                .await
                .unwrap();
            let w = onchain.wallet.read().await;
            let change = parse_address(&second, bitcoin::Network::Regtest)
                .unwrap()
                .script_pubkey();
            assert!(w.prepared[&prepared.id]
                .transaction
                .output
                .iter()
                .any(|o| o.script_pubkey == change));
            drop(w);
            assert_ne!(onchain.next_unused_address().await.unwrap(), second);
        });
    }

    /// A restore scan finds an old payment at an address no sync has revealed, while a sync
    /// run by bark (its daemon) moves the tip. The payment must still read as confirmed.
    #[test]
    fn scan_overlapped_by_a_bark_sync_keeps_old_payments_confirmed() {
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let (old, new) = (
                bitcoin::BlockHash::from_byte_array([1; 32]),
                bitcoin::BlockHash::from_byte_array([2; 32]),
            );
            let mut esplora = stub_esplora(vec![(2, new), (1, old)]);
            let (onchain, _db, _directory) = open_wallet(&esplora).await;
            let script = {
                let w = onchain.wallet.read().await;
                let address = w.wallet.peek_address(bdk_wallet::KeychainKind::External, 3);
                address.script_pubkey()
            };
            serve_payment(&esplora, &script, old);

            let scanning = tokio::spawn({
                let onchain = onchain.clone();
                async move { onchain.initial_scan(None).await }
            });
            esplora.entered.recv().await.unwrap();
            // What bark's sync would apply: the new tip, nothing for the unrevealed address.
            {
                let mut w = onchain.wallet.write().await;
                let tip = w
                    .wallet
                    .latest_checkpoint()
                    .insert(bdk_wallet::chain::BlockId {
                        height: 2,
                        hash: new,
                    });
                w.wallet
                    .apply_update(bdk_wallet::Update {
                        chain: Some(tip),
                        ..Default::default()
                    })
                    .unwrap();
            }
            esplora.release.send(()).unwrap();
            assert_eq!(scanning.await.unwrap().unwrap(), 50_000);
            let balance = onchain.balance().await.unwrap();
            assert_eq!((balance.confirmed_sats, balance.pending_sats), (50_000, 0));
        });
    }
}
