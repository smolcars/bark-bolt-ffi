use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use anyhow::{ensure, Context};
use bark::chain::ChainSource;
use bark::onchain::bdk_wallet::chain::{Indexer, Merge};
use bark::onchain::{bdk_wallet, CpfpError, MakeCpfpFees, OnchainWallet, OnchainWalletTrait};
use bitcoin::{Address, Amount, FeeRate, OutPoint, Psbt, Script, Transaction, Txid};

use crate::error::{construction_error_code, BarkError, BarkErrorCode};
use crate::types::{OnchainSendAmount, PreparedTransaction};

pub(crate) fn fee_rate(sat_per_vb: u64) -> Result<FeeRate, BarkError> {
    FeeRate::from_sat_per_vb(sat_per_vb)
        .filter(|rate| sat_per_vb > 0 && rate.fee_wu(bitcoin::Weight::from_wu(4_000_000)).is_some())
        .ok_or_else(|| {
            BarkError::typed(
                BarkErrorCode::InvalidFeeRate,
                "fee rate must be positive sat/vB without overflow",
            )
        })
}

pub(crate) async fn operation_guard(
    onchain: Option<std::sync::Arc<tokio::sync::RwLock<dyn OnchainWalletTrait>>>,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    let onchain = onchain?;
    let gate = {
        let guard = onchain.read().await;
        let state = (&*guard as &dyn std::any::Any).downcast_ref::<WalletState>()?;
        state.operation_gate.clone()
    };
    Some(gate.lock_owned().await)
}

pub(crate) async fn attach_database(
    onchain: &std::sync::Arc<tokio::sync::RwLock<dyn OnchainWalletTrait>>,
    db: std::sync::Arc<dyn bark::persist::BarkPersister>,
) -> Result<(), BarkError> {
    let mut guard = onchain.write().await;
    if let Some(state) = (&mut *guard as &mut dyn std::any::Any).downcast_mut::<WalletState>() {
        state.board_databases.retain(|db| db.strong_count() > 0);
        let inputs = board_inputs(&*db).await?;
        if inputs
            .iter()
            .any(|input| state.wallet.is_outpoint_locked(*input))
        {
            return Err(BarkError::typed(
                BarkErrorCode::PreparedInputsUnavailable,
                "wallet has pending board funding that conflicts with prepared inputs",
            ));
        }
        state.board_databases.push(std::sync::Arc::downgrade(&db));
    }
    Ok(())
}

pub(crate) async fn check_board_inputs(
    onchain: Option<std::sync::Arc<tokio::sync::RwLock<dyn OnchainWalletTrait>>>,
    transaction: &Transaction,
) -> Result<(), BarkError> {
    if let Some(onchain) = onchain {
        let guard = onchain.read().await;
        if let Some(state) = (&*guard as &dyn std::any::Any).downcast_ref::<WalletState>() {
            let pending = state.pending_board_inputs().await?;
            if transaction
                .input
                .iter()
                .any(|input| pending.contains(&input.previous_output))
            {
                return Err(BarkError::typed(
                    BarkErrorCode::PreparedInputsUnavailable,
                    "board inputs are reserved by a pending board",
                ));
            }
            state.check_inputs(transaction).map_err(|error| {
                BarkError::typed(BarkErrorCode::PreparedInputsUnavailable, error.to_string())
            })?;
        }
    }
    Ok(())
}

async fn board_inputs(db: &dyn bark::persist::BarkPersister) -> anyhow::Result<Vec<OutPoint>> {
    let mut inputs = Vec::new();
    for board in db
        .get_all_wallet_action_checkpoints()
        .await?
        .into_iter()
        .filter_map(|cp| cp.into_board())
    {
        inputs.extend(
            board
                .funding()?
                .input
                .iter()
                .map(|input| input.previous_output),
        );
    }
    Ok(inputs)
}

pub(crate) struct Prepared {
    pub transaction: Transaction,
    pub psbt: Psbt,
    pub board: Option<super::prepared_board::BoardPreparation>,
    pub recorded: bool,
    /// Submit re-checks that every parent is still confirmed (a reorg can undo it).
    pub confirmed_only: bool,
}

/// The daemon and every SDK handle share both BDK and these reservations.
pub(crate) struct WalletState {
    pub wallet: OnchainWallet,
    pub prepared: HashMap<String, Prepared>,
    pub identity: bitcoin::bip32::Xpub,
    pub db: std::sync::Arc<dyn bark::persist::BarkPersister>,
    pub board_databases: Vec<std::sync::Weak<dyn bark::persist::BarkPersister>>,
    pub operation_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub _registration: std::sync::Arc<std::path::PathBuf>,
}

impl Deref for WalletState {
    type Target = OnchainWallet;
    fn deref(&self) -> &Self::Target {
        &self.wallet
    }
}

impl DerefMut for WalletState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.wallet
    }
}

/// Pending checkpoint locks must disappear even if a daemon build is cancelled.
struct PendingBoardGuard<'a> {
    state: &'a mut WalletState,
    locks: Vec<OutPoint>,
}
impl Drop for PendingBoardGuard<'_> {
    fn drop(&mut self) {
        for point in &self.locks {
            self.state.transient_lock(*point, false);
        }
    }
}

impl WalletState {
    async fn reserve_pending_boards(&mut self) -> anyhow::Result<PendingBoardGuard<'_>> {
        let locks: Vec<_> = self
            .pending_board_inputs()
            .await?
            .into_iter()
            .filter(|point| !self.wallet.is_outpoint_locked(*point))
            .collect();
        for point in &locks {
            self.transient_lock(*point, true);
        }
        Ok(PendingBoardGuard { state: self, locks })
    }

    pub async fn send(
        &mut self,
        chain: &ChainSource,
        address: Address,
        amount: Amount,
        fee_rate: FeeRate,
    ) -> anyhow::Result<Txid> {
        let guard = self.reserve_pending_boards().await?;
        guard
            .state
            .wallet
            .send(chain, address, amount, fee_rate)
            .await
    }

    pub async fn drain(
        &mut self,
        chain: &ChainSource,
        address: Address,
        fee_rate: FeeRate,
    ) -> anyhow::Result<Txid> {
        let guard = self.reserve_pending_boards().await?;
        guard.state.wallet.drain(chain, address, fee_rate).await
    }

    pub async fn pending_board_inputs(&self) -> anyhow::Result<Vec<OutPoint>> {
        let mut inputs = board_inputs(&*self.db).await?;
        for db in self
            .board_databases
            .iter()
            .filter_map(std::sync::Weak::upgrade)
        {
            inputs.extend(board_inputs(&*db).await?);
        }
        Ok(inputs)
    }
    fn draft_wallet(&self) -> anyhow::Result<bdk_wallet::Wallet> {
        let changes = bdk_wallet::ChangeSet {
            descriptor: Some(
                self.wallet
                    .public_descriptor(bdk_wallet::KeychainKind::External)
                    .clone(),
            ),
            network: Some(self.wallet.network()),
            local_chain: self.wallet.local_chain().initial_changeset(),
            tx_graph: self.wallet.tx_graph().initial_changeset(),
            indexer: self.wallet.spk_index().initial_changeset(),
            locked_outpoints: bdk_wallet::locked_outpoints::ChangeSet {
                outpoints: self
                    .wallet
                    .list_locked_outpoints()
                    .map(|point| (point, true))
                    .collect(),
            },
            ..Default::default()
        };
        let mut draft = bdk_wallet::Wallet::load()
            .load_wallet_no_persist(changes)?
            .context("could not copy onchain wallet state")?;
        for prepared in self.prepared.values() {
            for output in &prepared.transaction.output {
                if let Some((keychain, index)) =
                    draft.derivation_of_spk(output.script_pubkey.clone())
                {
                    draft.mark_used(keychain, index);
                }
            }
        }
        Ok(draft)
    }

    /// Commit before changing the live graph, so a failed write cannot later auto-broadcast.
    pub async fn record_prepared(&mut self, transaction: &Transaction) -> anyhow::Result<()> {
        let mut draft = self.draft_wallet()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let seen = now.max(
            self.wallet
                .tx_graph()
                .get_last_evicted(transaction.compute_txid())
                .map_or(0, |evicted| evicted.saturating_add(1)),
        );
        draft.apply_unconfirmed_txs([(transaction.clone(), seen)]);
        let mut changes = self.wallet.staged().cloned().unwrap_or_default();
        if let Some(additions) = draft.take_staged() {
            changes.merge(additions);
        }
        self.db.store_bdk_wallet_changeset(&changes).await?;
        self.wallet
            .apply_unconfirmed_txs([(transaction.clone(), seen)]);
        self.wallet.take_staged();
        Ok(())
    }

    fn transient_lock(&mut self, outpoint: OutPoint, locked: bool) {
        if locked {
            self.wallet.lock_outpoint(outpoint);
        } else {
            self.wallet.unlock_outpoint(outpoint);
        }
        // Preparations are process-local; sync must never persist their locks.
        if let Some(stage) = self.wallet.staged_mut() {
            stage.locked_outpoints.outpoints.remove(&outpoint);
        }
    }

    pub fn release_inputs(&mut self, transaction: &Transaction) {
        for input in &transaction.input {
            self.transient_lock(input.previous_output, false);
        }
    }

    pub fn prepare(
        &mut self,
        address: Address,
        amount: OnchainSendAmount,
        fee_rate: FeeRate,
        confirmed_only: bool,
        unavailable: &[OutPoint],
    ) -> Result<PreparedTransaction, BarkError> {
        let script = address.script_pubkey();
        if let OnchainSendAmount::Amount { amount_sats } = amount {
            if amount_sats == 0 || amount_sats > Amount::MAX_MONEY.to_sat() {
                return Err(BarkError::typed(
                    BarkErrorCode::InvalidAmount,
                    "amount must be positive and within Bitcoin's money range",
                ));
            }
            if Amount::from_sat(amount_sats) < script.minimal_non_dust() {
                return Err(BarkError::typed(
                    BarkErrorCode::Dust,
                    "recipient amount is below the dust limit",
                ));
            }
        }
        let mut draft = self.draft_wallet()?;
        let mut builder = draft.build_tx();
        builder.unspendable(unavailable.to_vec());
        builder
            .ordering(bdk_wallet::TxOrdering::Untouched)
            .fee_rate(fee_rate);
        if confirmed_only {
            builder.exclude_unconfirmed();
        }
        match amount {
            OnchainSendAmount::Amount { amount_sats } => {
                builder.add_recipient(script.clone(), Amount::from_sat(amount_sats));
            }
            OnchainSendAmount::DrainAll => {
                builder.drain_wallet().drain_to(script.clone());
            }
        }
        let mut psbt = builder.finish().map_err(|error| {
            BarkError::typed(construction_error_code(&error), error.to_string())
        })?;
        let fee_sats = psbt
            .fee()
            .context("prepared transaction fee is unavailable")?
            .to_sat();
        let finalized = self
            .wallet
            .inner
            .sign(&mut psbt, bdk_wallet::SignOptions::default())
            .context("failed to sign prepared transaction")?;
        finalized
            .then_some(())
            .context("prepared transaction was not fully signed")?;
        let transaction = psbt
            .clone()
            .extract_tx()
            .context("failed to extract prepared transaction")?;
        let amount_sats = match amount {
            OnchainSendAmount::Amount { amount_sats } => amount_sats,
            OnchainSendAmount::DrainAll => transaction
                .output
                .iter()
                .filter(|output| output.script_pubkey == script)
                .map(|output| output.value.to_sat())
                .sum(),
        };
        let id = hex::encode(bitcoin::secp256k1::rand::random::<[u8; 16]>());
        let result = PreparedTransaction {
            id: id.clone(),
            txid: transaction.compute_txid().to_string(),
            amount_sats,
            fee_sats,
            vsize: transaction.vsize() as u64,
            inputs: transaction
                .input
                .iter()
                .map(|input| input.previous_output.into())
                .collect(),
        };
        self.wallet
            .apply_update(bdk_wallet::Update {
                last_active_indices: draft.spk_index().last_revealed_indices(),
                ..Default::default()
            })
            .context("failed to reserve change address")?;
        for output in &transaction.output {
            if let Some((keychain, index)) =
                self.wallet.derivation_of_spk(output.script_pubkey.clone())
            {
                self.wallet.mark_used(keychain, index);
            }
        }
        for input in &transaction.input {
            self.transient_lock(input.previous_output, true);
        }
        self.prepared.insert(
            id,
            Prepared {
                transaction,
                psbt,
                board: None,
                recorded: false,
                confirmed_only,
            },
        );
        Ok(result)
    }

    fn check_inputs(&self, transaction: &Transaction) -> anyhow::Result<()> {
        ensure!(
            transaction
                .input
                .iter()
                .all(|input| !self.wallet.is_outpoint_locked(input.previous_output)),
            "transaction spends inputs reserved by a prepared transaction"
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl OnchainWalletTrait for WalletState {
    async fn balance(&self) -> Amount {
        OnchainWalletTrait::balance(&self.wallet).await
    }
    async fn address(&mut self) -> anyhow::Result<Address> {
        self.wallet.address().await
    }
    async fn sync(&mut self, chain: &ChainSource) -> anyhow::Result<()> {
        self.wallet.sync(chain).await
    }
    async fn is_mine(&self, script: &Script) -> anyhow::Result<bool> {
        self.wallet.is_mine(script).await
    }
    async fn register_tx(&mut self, transaction: &Transaction) -> anyhow::Result<()> {
        self.check_inputs(transaction)?;
        self.wallet.register_tx(transaction).await
    }
    async fn evict_tx(&mut self, txid: Txid) -> anyhow::Result<()> {
        self.wallet.evict_tx(txid).await
    }
    async fn prepare_tx(
        &mut self,
        destinations: &[(Address, Amount)],
        fee_rate: FeeRate,
    ) -> anyhow::Result<Psbt> {
        let guard = self.reserve_pending_boards().await?;
        guard.state.wallet.prepare_tx(destinations, fee_rate).await
    }
    async fn prepare_drain_tx(
        &mut self,
        destination: Address,
        fee_rate: FeeRate,
    ) -> anyhow::Result<Psbt> {
        let guard = self.reserve_pending_boards().await?;
        guard
            .state
            .wallet
            .prepare_drain_tx(destination, fee_rate)
            .await
    }
    async fn sign_psbt(&mut self, psbt: Psbt) -> anyhow::Result<Psbt> {
        self.check_inputs(&psbt.unsigned_tx)?;
        OnchainWalletTrait::sign_psbt(&mut self.wallet, psbt).await
    }
    async fn finish_psbt(&mut self, psbt: Psbt) -> anyhow::Result<Psbt> {
        self.check_inputs(&psbt.unsigned_tx)?;
        self.wallet.finish_psbt(psbt).await
    }
    async fn make_signed_p2a_cpfp(
        &mut self,
        transaction: &Transaction,
        fees: MakeCpfpFees,
    ) -> Result<Transaction, CpfpError> {
        let guard = self
            .reserve_pending_boards()
            .await
            .map_err(|error| CpfpError::StoreError(error.to_string()))?;
        guard
            .state
            .wallet
            .make_signed_p2a_cpfp(transaction, fees)
            .await
    }
    async fn store_signed_p2a_cpfp(&mut self, transaction: &Transaction) -> Result<(), CpfpError> {
        self.check_inputs(transaction)
            .map_err(|error| CpfpError::StoreError(error.to_string()))?;
        self.wallet.store_signed_p2a_cpfp(transaction).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bark::persist::BarkPersister;
    use bitcoin::hashes::Hash;
    use std::sync::Arc;

    async fn funded() -> (WalletState, tempfile::TempDir, Address) {
        let directory = tempfile::tempdir().unwrap();
        let db = crate::ffi::db::open_sqlite_db(directory.path().to_str().unwrap()).unwrap();
        let seed = [42; 64];
        let identity = bitcoin::bip32::Xpub::from_priv(
            &bitcoin::secp256k1::Secp256k1::new(),
            &bitcoin::bip32::Xpriv::new_master(bitcoin::Network::Regtest, &seed).unwrap(),
        );
        let mut wallet = OnchainWallet::load_or_create(bitcoin::Network::Regtest, seed, db.clone())
            .await
            .unwrap();
        let address = wallet.address().await.unwrap();
        let block = bitcoin::BlockHash::from_byte_array([3; 32]);
        let mut update = bdk_wallet::Update {
            chain: Some(
                wallet
                    .latest_checkpoint()
                    .insert(bdk_wallet::chain::BlockId {
                        height: 1,
                        hash: block,
                    }),
            ),
            ..Default::default()
        };
        for index in 0..12 {
            let transaction = Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![bitcoin::TxIn {
                    previous_output: OutPoint {
                        txid: Txid::from_byte_array([index + 1; 32]),
                        vout: 0,
                    },
                    ..Default::default()
                }],
                output: vec![bitcoin::TxOut {
                    value: Amount::from_sat(10_000),
                    script_pubkey: address.script_pubkey(),
                }],
            };
            update.tx_update.anchors.insert((
                bdk_wallet::chain::ConfirmationBlockTime {
                    block_id: bdk_wallet::chain::BlockId {
                        height: 1,
                        hash: block,
                    },
                    confirmation_time: 1,
                },
                transaction.compute_txid(),
            ));
            update.tx_update.txs.push(Arc::new(transaction));
        }
        wallet.apply_update(update).unwrap();
        db.store_bdk_wallet_changeset(&wallet.take_staged().unwrap())
            .await
            .unwrap();
        let registration =
            crate::backup::register_live_database(&directory.path().join("db.sqlite")).unwrap();
        let state = WalletState {
            wallet,
            prepared: HashMap::new(),
            identity,
            db,
            board_databases: Vec::new(),
            operation_gate: Arc::new(tokio::sync::Mutex::new(())),
            _registration: registration,
        };
        let destination = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"
            .parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()
            .unwrap()
            .require_network(bitcoin::Network::Regtest)
            .unwrap();
        (state, directory, destination)
    }

    fn files(directory: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read(path).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn exact_many_input_fee_reservations_and_no_prepare_writes() {
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let (mut state, directory, destination) = funded().await;
            let before = files(directory.path());
            let prepared = state
                .prepare(
                    destination.clone(),
                    OnchainSendAmount::Amount {
                        amount_sats: 105_000,
                    },
                    FeeRate::from_sat_per_vb(10).unwrap(),
                    true,
                    &[],
                )
                .unwrap();
            assert!(prepared.inputs.len() > 10);
            let tx = state.prepared[&prepared.id].transaction.clone();
            assert_eq!(prepared.txid, tx.compute_txid().to_string());
            assert_eq!(prepared.vsize, tx.vsize() as u64);
            assert_eq!(
                prepared.fee_sats,
                10_000 * tx.input.len() as u64
                    - tx.output.iter().map(|o| o.value.to_sat()).sum::<u64>()
            );
            assert!(prepared.fee_sats >= prepared.vsize * 10);
            assert!(tx.input.iter().all(|input| !input.witness.is_empty()));
            assert!(state.wallet.get_tx(tx.compute_txid()).is_none());
            assert_eq!(files(directory.path()), before);
            assert!(state.prepared.values().all(|p| !p.recorded));
            assert!(state
                .wallet
                .staged()
                .is_none_or(|stage| stage.locked_outpoints.outpoints.is_empty()));
            assert!(state
                .prepare_drain_tx(destination.clone(), FeeRate::from_sat_per_vb(10).unwrap())
                .await
                .is_err());
            let psbt = Psbt::from_unsigned_tx(Transaction {
                input: tx
                    .input
                    .iter()
                    .map(|i| bitcoin::TxIn {
                        witness: bitcoin::Witness::new(),
                        ..i.clone()
                    })
                    .collect(),
                ..tx.clone()
            })
            .unwrap();
            assert!(state.sign_psbt(psbt).await.is_err());
            let error = state
                .prepare(
                    destination.clone(),
                    OnchainSendAmount::DrainAll,
                    FeeRate::from_sat_per_vb(10).unwrap(),
                    true,
                    &[],
                )
                .unwrap_err();
            assert_eq!(error.code, BarkErrorCode::InsufficientFunds);
            state.prepared.remove(&prepared.id);
            state.release_inputs(&tx);
            assert_eq!(files(directory.path()), before);
            let drain = state
                .prepare(
                    destination,
                    OnchainSendAmount::DrainAll,
                    FeeRate::from_sat_per_vb(10).unwrap(),
                    true,
                    &[],
                )
                .unwrap();
            assert_eq!(drain.amount_sats + drain.fee_sats, 120_000);
            assert_eq!(state.prepared[&drain.id].transaction.output.len(), 1);
        });
    }

    #[test]
    fn construction_errors_leave_database_and_reservations_unchanged() {
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let (mut state, directory, destination) = funded().await;
            let before = files(directory.path());
            for (amount_sats, code) in [
                (0, BarkErrorCode::InvalidAmount),
                (1, BarkErrorCode::Dust),
                (200_000, BarkErrorCode::InsufficientFunds),
            ] {
                let error = state
                    .prepare(
                        destination.clone(),
                        OnchainSendAmount::Amount { amount_sats },
                        FeeRate::from_sat_per_vb(10).unwrap(),
                        true,
                        &[],
                    )
                    .unwrap_err();
                assert_eq!(error.code, code);
                assert!(state.prepared.is_empty());
                assert_eq!(state.wallet.list_locked_outpoints().count(), 0);
                assert_eq!(files(directory.path()), before);
            }
        });
    }

    #[test]
    fn recording_commits_exact_bytes_and_never_persists_transient_locks() {
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let (mut state, _directory, destination) = funded().await;
            let prepared = state
                .prepare(
                    destination,
                    OnchainSendAmount::DrainAll,
                    FeeRate::from_sat_per_vb(10).unwrap(),
                    true,
                    &[],
                )
                .unwrap();
            let tx = state.prepared[&prepared.id].transaction.clone();
            state.record_prepared(&tx).await.unwrap();
            let changes = state.db.initialize_bdk_wallet().await.unwrap();
            let reopened = bdk_wallet::Wallet::load()
                .load_wallet_no_persist(changes)
                .unwrap()
                .unwrap();
            assert_eq!(
                reopened
                    .get_tx(tx.compute_txid())
                    .unwrap()
                    .tx_node
                    .tx
                    .as_ref(),
                &tx
            );
            assert_eq!(reopened.list_locked_outpoints().count(), 0);
        });
    }
}
