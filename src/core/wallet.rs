use std::str::FromStr;
use std::sync::Arc;

use anyhow::Context;
use base64::Engine;
use bip39::Mnemonic;
use bitcoin::hex::FromHex;
use chrono::{DateTime, Duration, Local};
use log::{info, warn};

use ark::lightning::{Invoice, Offer, PaymentHash};
use ark::{ProtocolEncoding, VtxoId};

use bark::onchain::OnchainWalletTrait;
use bark::persist::BarkPersister;
use bark::WalletSeed;

use crate::config::Config;
use crate::core::parse_address;
use crate::error::{BarkError, BarkErrorCode};
use crate::{types, Network};

/// Core Bark wallet logic. No binding-specific behavior — pure async over
/// `bark::Wallet`. The native adapter adds runtime offload, callbacks and lifetime control.
pub struct Wallet {
    pub(super) inner: bark::Wallet,
    pub(super) board_owner: Option<std::path::PathBuf>,
    /// Outcome of the seed recovery scan `bark::Wallet::open` ran.
    /// See [`Wallet::recovery_status`].
    recovery_status: types::RecoveryStatus,
}

/// Optional arguments for [`Wallet::open`].
pub struct OpenArgs {
    pub run_daemon: bool,
    pub datadir: Option<String>,
    pub persister: Option<Arc<dyn BarkPersister>>,
    pub lock_manager: Option<Box<dyn bark::lock_manager::LockManager>>,
    pub onchain: Option<Arc<tokio::sync::RwLock<dyn OnchainWalletTrait>>>,
    pub create_if_not_exists: bool,
    pub create_without_server: bool,
    /// Skip the seed-recovery mailbox scan that otherwise runs when this open
    /// creates the wallet locally.
    pub skip_recovery: bool,
}

impl OpenArgs {
    pub(crate) fn into_bark(self) -> bark::OpenWalletArgs {
        bark::OpenWalletArgs {
            run_daemon: self.run_daemon,

            datadir: self.datadir.map(|s| s.into()),

            persister: self.persister,

            lock_manager: self.lock_manager,

            onchain: self.onchain,
            create_if_not_exists: self.create_if_not_exists,
            create_without_server: self.create_without_server,
            skip_recovery: self.skip_recovery,
            on_recovery_finished: None,
        }
    }
}

/// Parse a [WalletSeed] from a string that is either a BIP-39 mnemonic or a 64-byte hex seed
pub(crate) fn seed_from_str(
    network: bitcoin::Network,
    mnemonic_or_seed: &str,
) -> Result<WalletSeed, BarkError> {
    let seed_or_phrase = mnemonic_or_seed.trim();

    // Any whitespace, not just ' ': a phrase pasted with newlines used to fall
    // through to the hex branch and fail as "invalid hex seed".
    if seed_or_phrase.split_whitespace().nth(1).is_some() {
        let phrase = seed_or_phrase
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mnemonic = Mnemonic::parse(&phrase).context("invalid mnemonic")?;
        Ok(WalletSeed::new_from_mnemonic(network, &mnemonic))
    } else {
        let bytes = <[u8; 64]>::from_hex(seed_or_phrase)
            .context("invalid hex seed, needs to be 64 bytes")?;
        Ok(WalletSeed::new_from_seed(network, &bytes))
    }
}

/// Parse a batch of VTXO id strings, failing on the first malformed one.
pub(crate) fn parse_vtxo_ids(vtxo_ids: &[String]) -> Result<Vec<VtxoId>, BarkError> {
    vtxo_ids
        .iter()
        .map(|id| {
            id.parse::<VtxoId>()
                .with_context(|| format!("invalid vtxo id: {}", id))
                .map_err(BarkError::from)
        })
        .collect()
}

/// Human-readable one-liner for a VTXO state, for error messages.
fn describe_state(state: &types::VtxoState) -> String {
    match state {
        types::VtxoState::Unknown => "unknown (decoded without wallet state)".to_owned(),
        types::VtxoState::Spendable => "spendable".to_owned(),
        types::VtxoState::Locked { holder: None } => "locked by an unnamed holder".to_owned(),
        types::VtxoState::Locked { holder: Some(h) } => match h {
            types::VtxoLockHolder::Action { id } => format!("locked by action {}", id),
            types::VtxoLockHolder::Movement { id } => format!("locked by movement {}", id),
        },
        types::VtxoState::Spent => "spent".to_owned(),
        types::VtxoState::Exited => "exited".to_owned(),
    }
}

impl Wallet {
    pub(crate) fn inner(&self) -> &bark::Wallet {
        &self.inner
    }

    // ------------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------------

    /// Raw function to create a new wallet
    ///
    /// You will almost always want to just use the `open` function instead, as
    /// it creates a wallet if it doesn't yet exist by default.
    pub async fn create(
        network: Network,
        mnemonic_or_seed: String,
        config: Config,
        db: &dyn BarkPersister,
        lock_manager: &dyn bark::lock_manager::LockManager,
        allow_unreachable_server: bool,
    ) -> Result<(), BarkError> {
        let network = network.into();
        let cfg = config.into_bark(network)?;
        let seed = seed_from_str(network, &mnemonic_or_seed)?;

        bark::Wallet::create(
            network,
            &seed,
            &cfg,
            db,
            lock_manager,
            allow_unreachable_server,
        )
        .await?;
        Ok(())
    }

    /// Open a wallet, or create one if it doesn't exist
    pub async fn open(
        network: Network,
        mnemonic_or_seed: String,
        config: Config,
        args: OpenArgs,
    ) -> Result<Self, BarkError> {
        let network = network.into();
        let cfg = config.into_bark(network)?;
        let seed = seed_from_str(network, &mnemonic_or_seed)?;
        let board_owner = args.datadir.as_ref().map(std::path::PathBuf::from);
        let mut args = args.into_bark();
        let _onchain_operation =
            super::prepared_onchain::operation_guard(args.onchain.clone()).await;
        if let (Some(onchain), Some(db)) = (&args.onchain, &args.persister) {
            super::prepared_onchain::attach_database(onchain, db.clone()).await?;
        }

        // The seed recovery scan runs inside `bark::Wallet::open` and reports
        // through this callback, so stash its outcome and hand it to callers via
        // [`Self::recovery_status`] once open returns. Keeps the result
        // available without plumbing a foreign callback across the FFI.
        // Upstream calls it exactly once per successful open.
        let slot = Arc::new(std::sync::Mutex::new(None));
        let sink = slot.clone();
        args.on_recovery_finished = Some(Box::new(move |status| {
            let status = types::RecoveryStatus::from(status);
            match &status {
                types::RecoveryStatus::NotRun => info!("[OPEN] Seed recovery not run"),
                types::RecoveryStatus::Failed { message } => {
                    warn!(
                        "[OPEN] Seed recovery FAILED, funds may be missing: {}",
                        message
                    )
                }
                types::RecoveryStatus::Completed { report } => info!(
                    "[OPEN] Seed recovery finished: {} recovered ({} sats), {} skipped, \
                     {} exited, {} foreign, {} failed, complete={}",
                    report.recovered.vtxo_ids.len(),
                    report.recovered.total_sats,
                    report.skipped.vtxo_ids.len(),
                    report.exited.vtxo_ids.len(),
                    report.foreign.vtxo_ids.len(),
                    report.failed.vtxo_ids.len(),
                    report.is_complete,
                ),
            }
            *sink.lock().unwrap() = Some(status);
        }));

        let inner = bark::Wallet::open(network, seed, cfg, args).await?;

        if inner.ark_info().await.ok().flatten().is_some() {
            info!("[OPEN] Server connection established");
        } else {
            warn!("[OPEN] Server connection FAILED - Lightning and Ark operations will not work!");
        }

        // Upstream guarantees the callback fires on every successful open, so
        // an empty slot would be an upstream bug; degrade to `NotRun` rather
        // than fail the open over it.
        let recovery_status = slot
            .lock()
            .unwrap()
            .take()
            .unwrap_or(types::RecoveryStatus::NotRun);
        Ok(Self {
            inner,
            board_owner: board_owner
                .map(|path| path.canonicalize())
                .transpose()
                .context("invalid Ark data directory")?,
            recovery_status,
        })
    }

    /// Outcome of the seed-recovery mailbox scan that ran during
    /// [`Self::open`].
    ///
    /// Recovery only runs on the open that creates the wallet locally, and not
    /// at all when `OpenArgs::skip_recovery` is set, so this is `NotRun` on
    /// every subsequent open. `Failed` means the scan errored before producing
    /// a report — upstream logs that and lets open succeed — so funds may be
    /// missing until a retry; `Completed` carries the report, and a report
    /// with `is_complete == false` means funds may still be missing. Retry
    /// `failed` ids with [`Self::recover_vtxos`].
    pub fn recovery_status(&self) -> types::RecoveryStatus {
        self.recovery_status.clone()
    }

    /// The report of the seed-recovery scan that ran during [`Self::open`],
    /// or `None` if the scan did not complete. Use [`Self::recovery_status`]
    /// to tell a scan that failed apart from one that never ran.
    pub fn recovery_report(&self) -> Option<types::RecoveryReport> {
        self.recovery_status.report().cloned()
    }

    // ------------------------------------------------------------------------
    // Daemon
    // ------------------------------------------------------------------------

    /// Stop the background daemon, if running, and wait until its tasks have
    /// finished, so the caller knows nothing runs in the background anymore
    /// (e.g. before wiping the wallet's datadir). No-op without a daemon.
    pub async fn stop_daemon_wait(&self) -> Result<(), BarkError> {
        self.inner
            .stop_daemon_wait()
            .await
            .context("Stop daemon failed")?;
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Synchronization & Maintenance
    // ------------------------------------------------------------------------

    pub async fn sync(&self) -> Result<(), BarkError> {
        info!("[SYNC] Starting sync...");
        self.inner.sync().await;
        info!("[SYNC] Sync completed");

        if let Ok(balance) = self.inner.balance().await {
            info!(
                "[SYNC] Balance after sync: spendable={}, pending_board={}",
                balance.spendable.to_sat(),
                balance.pending_board.to_sat()
            );
        }

        if let Ok(vtxos) = self.inner.vtxos().await {
            info!("[SYNC] VTXOs after sync: {} total", vtxos.len());
            for (i, vtxo) in vtxos.iter().enumerate().take(3) {
                info!(
                    "[SYNC]   VTXO {}: {} sats, state={:?}",
                    i,
                    vtxo.vtxo.amount().to_sat(),
                    vtxo.state.kind()
                );
            }
        }

        Ok(())
    }

    pub async fn maintenance(&self) -> Result<(), BarkError> {
        self.inner.maintenance().await?;
        Ok(())
    }

    /// Scan for spendable VTXOs that were force-exited on-chain without the
    /// user asking (e.g. the server's watchman progressing a shared tree) and
    /// route them into the unilateral-exit flow so the funds can be claimed.
    ///
    /// This already runs automatically as part of [`Self::sync`]; expose it so
    /// callers can trigger the scan on demand.
    pub async fn sync_force_exited_vtxos(&self) -> Result<(), BarkError> {
        self.inner.sync_force_exited_vtxos().await?;
        Ok(())
    }

    pub async fn maintenance_delegated(&self) -> Result<(), BarkError> {
        self.inner.maintenance_delegated().await?;
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Address Generation
    // ------------------------------------------------------------------------

    pub async fn new_address(&self) -> Result<String, BarkError> {
        let addr = self.inner.new_address().await?;
        Ok(addr.to_string())
    }

    pub async fn new_address_with_index(&self) -> Result<types::AddressWithIndex, BarkError> {
        let (addr, index) = self.inner.new_address_with_index().await?;
        Ok(types::AddressWithIndex {
            address: addr.to_string(),
            index,
        })
    }

    pub async fn peek_address(&self, index: u32) -> Result<String, BarkError> {
        let addr = self.inner.peek_address(index).await?;
        Ok(addr.to_string())
    }

    // ------------------------------------------------------------------------
    // Balance & VTXOs
    // ------------------------------------------------------------------------

    pub async fn balance(&self) -> Result<types::Balance, BarkError> {
        Ok(self.inner.balance().await?.into())
    }

    pub async fn vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        Ok(self
            .inner
            .vtxos()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn get_vtxo_by_id(&self, vtxo_id: String) -> Result<types::Vtxo, BarkError> {
        let id = vtxo_id.parse().context("invalid vtxo id")?;
        Ok(self.inner.get_vtxo_by_id(id).await?.into())
    }

    pub async fn spendable_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        Ok(self
            .inner
            .spendable_vtxos()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn all_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        Ok(self
            .inner
            .all_vtxos()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn get_expiring_vtxos(
        &self,
        threshold_blocks: u32,
    ) -> Result<Vec<types::Vtxo>, BarkError> {
        Ok(self
            .inner
            .get_expiring_vtxos(
                threshold_blocks
                    .try_into()
                    .context("threshold_blocks exceeds 65535")?,
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn get_vtxos_to_refresh(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        Ok(self
            .inner
            .get_vtxos_to_refresh()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    // ------------------------------------------------------------------------
    // VTXO locking
    // ------------------------------------------------------------------------

    /// Reserve VTXOs so wallet-driven flows leave them alone.
    ///
    /// A locked VTXO is excluded from coin selection, which means maintenance
    /// and delegated refresh rounds skip it. That makes locking the durable way
    /// to protect a VTXO the caller is handling itself — for example one whose
    /// unilateral exit is in progress, since starting an exit does not by
    /// itself change a VTXO's state.
    ///
    /// `holder` records who the reservation belongs to. Pass
    /// [`types::VtxoLockHolder::Action`] with an id the application chooses
    /// (say `"exit:<vtxo-id>"`); that same value is what
    /// [`Self::unlock_vtxos`] matches against, so it is what stops one
    /// subsystem from releasing another's lock. `None` leaves the lock
    /// unattributed and should be avoided when the reason is known.
    ///
    /// The batch is atomic: if any VTXO is not spendable — including one
    /// already locked by a *different* holder — nothing is locked. Re-locking
    /// with the identical holder is a no-op success, so retries are safe.
    pub async fn lock_vtxos(
        &self,
        vtxo_ids: Vec<String>,
        holder: Option<types::VtxoLockHolder>,
    ) -> Result<(), BarkError> {
        let ids = parse_vtxo_ids(&vtxo_ids)?;
        if ids.is_empty() {
            return Ok(());
        }

        self.inner
            .lock_vtxos(ids, holder.map(Into::into))
            .await
            .context("Lock VTXOs failed")?;

        info!("Locked {} VTXOs", vtxo_ids.len());
        Ok(())
    }

    /// Release VTXOs locked by [`Self::lock_vtxos`], returning them to the
    /// spendable set.
    ///
    /// `expected_holder` guards the release: every VTXO must currently be
    /// locked by that holder, otherwise nothing is unlocked and this returns an
    /// error naming the mismatch. This is what keeps an application's cleanup —
    /// unlocking after cancelling an exit, say — from freeing a VTXO that a
    /// lightning payment or an in-flight round has since locked for itself.
    ///
    /// Pass `None` to unlock regardless of holder. That bypasses the guard
    /// entirely, so reserve it for recovery paths where the original holder is
    /// genuinely unknown.
    ///
    /// Already-spendable VTXOs are accepted as a no-op, so retries are safe.
    ///
    /// # Concurrency
    ///
    /// The release itself is atomic per VTXO: upstream only rewrites the state
    /// row when the stored lock holder equals `expected_holder`, so a lock that
    /// another operation took between the holder check and the release is left
    /// alone rather than stolen (upstream treats that as a no-op). The holder
    /// check up front is what turns a mismatch into an error for the caller; a
    /// VTXO that became `Spent` or `Exited` in that window is likewise left
    /// untouched, without an error.
    pub async fn unlock_vtxos(
        &self,
        vtxo_ids: Vec<String>,
        expected_holder: Option<types::VtxoLockHolder>,
    ) -> Result<(), BarkError> {
        use bark::vtxo::{VtxoState, VtxoStateKind};

        let ids = parse_vtxo_ids(&vtxo_ids)?;
        if ids.is_empty() {
            return Ok(());
        }

        match expected_holder {
            Some(expected) => {
                self.ensure_locked_by(&ids, &expected).await?;

                // Holder-checked, atomic per VTXO (see # Concurrency).
                self.inner
                    .unlock_vtxos(ids, Some(expected.into()))
                    .await
                    .context("Unlock VTXOs failed")?;
            }
            // No expected holder: release unconditionally, whoever holds the
            // lock. Upstream's `unlock_vtxos(.., None)` would instead release
            // only locks that carry *no* holder, so drive the state transition
            // directly, as upstream's unlock did before 0.7. Allowing only
            // `Locked` and `Spendable` as prior states keeps a concurrent
            // `Spent`/`Exited` transition from being clobbered, and the batch
            // fails together rather than partially applying.
            None => {
                self.inner
                    .set_vtxo_states(
                        ids,
                        &VtxoState::Spendable,
                        &[VtxoStateKind::Locked, VtxoStateKind::Spendable],
                    )
                    .await
                    .context("Unlock VTXOs failed")?;
            }
        }

        info!("Unlocked {} VTXOs", vtxo_ids.len());
        Ok(())
    }

    /// Verify every VTXO is either locked by `expected` or already spendable.
    ///
    /// Checks the whole batch before reporting, so the caller sees each
    /// offending VTXO rather than only the first.
    async fn ensure_locked_by(
        &self,
        ids: &[VtxoId],
        expected: &types::VtxoLockHolder,
    ) -> Result<(), BarkError> {
        let mut mismatches = Vec::new();

        for id in ids {
            let vtxo = self
                .inner
                .get_vtxo_by_id(*id)
                .await
                .with_context(|| format!("VTXO not found: {}", id))?;

            match types::VtxoState::from(&vtxo.state) {
                // Unlocking is idempotent, so an already-released VTXO is fine.
                types::VtxoState::Spendable => {}
                types::VtxoState::Locked { holder: Some(h) } if &h == expected => {}
                other => mismatches.push(format!("{} is {}", id, describe_state(&other))),
            }
        }

        if !mismatches.is_empty() {
            return Err(BarkError::from(anyhow::anyhow!(
                "refusing to unlock {} VTXO(s) not locked by the expected holder: {}",
                mismatches.len(),
                mismatches.join(", "),
            )));
        }

        Ok(())
    }

    // ------------------------------------------------------------------------
    // Offboarding
    // ------------------------------------------------------------------------

    pub async fn offboard_all(
        &self,
        bitcoin_address: String,
    ) -> Result<types::OffboardResult, BarkError> {
        let addr = parse_address(&bitcoin_address, self.inner.network().await?)?;

        Ok(self.inner.offboard_all(addr).await?.into())
    }

    pub async fn offboard_vtxos(
        &self,
        vtxo_ids: Vec<String>,
        bitcoin_address: String,
    ) -> Result<types::OffboardResult, BarkError> {
        let addr = parse_address(&bitcoin_address, self.inner.network().await?)?;

        let ids: Result<Vec<_>, _> = vtxo_ids
            .iter()
            .map(|id| id.parse::<VtxoId>().context("invalid vtxo id"))
            .collect();

        Ok(self.inner.offboard_vtxos(ids?, addr).await?.into())
    }

    // ------------------------------------------------------------------------
    // Lightning (send)
    // ------------------------------------------------------------------------

    pub async fn pay_lightning_invoice(
        &self,
        invoice: String,
        amount_sats: Option<u64>,
        options: types::LightningSendOptions,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let invoice: Invoice = invoice.parse().context("invalid invoice")?;
        let amount = amount_sats.map(bitcoin::Amount::from_sat);
        let resolved = self
            .inner
            .pay_lightning_invoice_with(invoice, amount, options.into())
            .await?;
        self.lightning_send_status(resolved).await
    }

    pub async fn pay_lightning_offer(
        &self,
        offer: String,
        amount_sats: Option<u64>,
        options: types::LightningSendOptions,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let offer_obj =
            Offer::from_str(&offer).map_err(|e| anyhow::anyhow!("invalid BOLT12 offer: {e:?}"))?;
        let amount = amount_sats.map(bitcoin::Amount::from_sat);
        let resolved = self
            .inner
            .pay_lightning_offer_with(offer_obj, amount, options.into())
            .await?;
        self.lightning_send_status(resolved).await
    }

    /// Pay to a Lightning Address (`user@domain`). Resolves the address to a
    /// BOLT11 invoice via LNURL-pay and pays it. `comment` is forwarded as
    /// the LNURL-pay comment field; the endpoint may reject or truncate it.
    ///
    /// Bark resolves LNURL using its native HTTP client.
    pub async fn pay_lightning_address(
        &self,
        lightning_address: String,
        amount_sats: u64,
        comment: Option<String>,
        options: types::LightningSendOptions,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let addr: bark::lnurllib::lightning_address::LightningAddress = lightning_address
            .trim()
            .parse()
            .context("invalid lightning address")?;
        let amount = bitcoin::Amount::from_sat(amount_sats);
        let resolved = self
            .inner
            .pay_lightning_address_with(&addr, amount, comment.as_deref(), options.into())
            .await?;
        self.lightning_send_status(resolved).await
    }

    /// Pay a raw LNURL-pay link (`lnurl1…`). Resolves the LNURL-pay endpoint
    /// to a BOLT11 invoice and pays it. Errors if the link decodes to a
    /// non-pay LNURL (auth, withdraw, channel).
    ///
    /// Same cross-origin caveat as [`Self::pay_lightning_address`] in
    /// browsers.
    pub async fn pay_lnurl(
        &self,
        lnurl: String,
        amount_sats: u64,
        comment: Option<String>,
        options: types::LightningSendOptions,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let lnurl: bark::lnurllib::lnurl::LnUrl = lnurl.trim().parse().context("invalid lnurl")?;
        let amount = bitcoin::Amount::from_sat(amount_sats);
        let resolved = self
            .inner
            .pay_lnurl_with(&lnurl, amount, comment.as_deref(), options.into())
            .await?;
        self.lightning_send_status(resolved).await
    }

    /// Resolve the [`types::LightningSendStatus`] for a just-initiated send.
    /// `bark` now returns only the resolved [`Invoice`] from `pay_lightning_*`,
    /// so the send state is read back from the state machine by payment hash.
    pub(crate) async fn lightning_send_status(
        &self,
        invoice: Invoice,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let state = self
            .inner
            .lightning_send_state(invoice.payment_hash())
            .await?;
        Ok(state.into())
    }

    pub async fn check_lightning_payment(
        &self,
        payment_hash: String,
        wait: bool,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let payment_hash_obj =
            PaymentHash::from_str(&payment_hash).context("invalid payment hash")?;
        let state = self
            .inner
            .check_lightning_payment(payment_hash_obj, wait)
            .await?;
        Ok(state.into())
    }

    /// Read-only triage of a payment hash without driving the send forward.
    /// Use this to poll progress after initiating a payment with `wait = false`.
    /// Unlike [`Self::check_lightning_payment`], this never advances the action.
    pub async fn lightning_send_state(
        &self,
        payment_hash: String,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let payment_hash_obj =
            PaymentHash::from_str(&payment_hash).context("invalid payment hash")?;
        let state = self.inner.lightning_send_state(payment_hash_obj).await?;
        Ok(state.into())
    }

    /// Cheap "has this invoice ever been paid?" check, answered from the local
    /// `bark_paid_invoice` fact table without consulting the server.
    pub async fn is_invoice_paid(&self, payment_hash: String) -> Result<bool, BarkError> {
        let payment_hash_obj =
            PaymentHash::from_str(&payment_hash).context("invalid payment hash")?;
        Ok(self.inner.is_invoice_paid(payment_hash_obj).await?)
    }

    pub async fn pending_lightning_sends(&self) -> Result<Vec<types::LightningSend>, BarkError> {
        Ok(self
            .inner
            .pending_lightning_sends()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// List failed lightning sends whose HTLC revocation also failed.
    pub async fn stuck_failed_lightning_sends(
        &self,
    ) -> Result<Vec<types::LightningSend>, BarkError> {
        Ok(self
            .inner
            .stuck_failed_lightning_sends()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Opt an individual stuck send into auto-exiting its HTLCs as they approach
    /// expiry.
    pub async fn allow_lightning_send_to_exit(
        &self,
        payment_hash: String,
    ) -> Result<(), BarkError> {
        let payment_hash_obj =
            PaymentHash::from_str(&payment_hash).context("invalid payment hash")?;
        self.inner
            .allow_lightning_send_to_exit(payment_hash_obj)
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Lightning (receive)
    // ------------------------------------------------------------------------

    pub async fn bolt11_invoice(
        &self,
        amount_sats: u64,
        description: Option<String>,
        token: Option<String>,
    ) -> Result<types::LightningInvoice, BarkError> {
        let amount = bitcoin::Amount::from_sat(amount_sats);
        let invoice = self
            .inner
            .bolt11_invoice(amount, description, token)
            .await?;
        Ok(types::LightningInvoice {
            invoice: invoice.to_string(),
            payment_hash: invoice.payment_hash().to_string(),
            amount_sats,
        })
    }

    /// Create an invoice whose claimed VTXO is delivered to `claim_destination`
    /// (an Ark address), letting the recipient receive while offline.
    ///
    /// The claim is signed directly to that address's own policy, so this wallet
    /// never holds a key that can spend the funds and cannot redirect them. It
    /// can still strand them: delivering the signed output to the destination's
    /// mailbox is a separate step only this wallet can perform, and nobody else
    /// can discover or recover that output until it happens. Delivery resumes
    /// automatically on restart, so a crash recovers on its own — but while this
    /// wallet stays offline the destination cannot claim funds already signed to
    /// it. Running this for someone else means they trust you to come back
    /// online and deliver, not that they trust you with custody.
    ///
    /// A `claim_destination` owned by this wallet is claimed locally instead of
    /// going through its mailbox.
    pub async fn bolt11_invoice_for_address(
        &self,
        amount_sats: u64,
        claim_destination: String,
        description: Option<String>,
        token: Option<String>,
    ) -> Result<types::LightningInvoice, BarkError> {
        let addr: ark::Address = claim_destination.parse().context("invalid ark address")?;
        let amount = bitcoin::Amount::from_sat(amount_sats);
        let invoice = self
            .inner
            .bolt11_invoice_for_address(amount, addr, description, token)
            .await?;
        Ok(types::LightningInvoice {
            invoice: invoice.to_string(),
            payment_hash: invoice.payment_hash().to_string(),
            amount_sats,
        })
    }

    pub async fn try_claim_all_lightning_receives(
        &self,
        wait: bool,
    ) -> Result<Vec<types::LightningReceive>, BarkError> {
        let receives = self.inner.try_claim_all_lightning_receives(wait).await?;
        Ok(receives.into_iter().map(Into::into).collect())
    }

    pub async fn pending_lightning_receives(
        &self,
    ) -> Result<Vec<types::LightningReceive>, BarkError> {
        Ok(self
            .inner
            .pending_lightning_receives()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn claimable_lightning_receive_balance_sats(&self) -> Result<u64, BarkError> {
        Ok(self
            .inner
            .claimable_lightning_receive_balance()
            .await?
            .to_sat())
    }

    /// Triage a payment hash: settled or in-progress. Errors if no lightning
    /// receive is known for this payment hash.
    pub async fn lightning_receive_state(
        &self,
        payment_hash: String,
    ) -> Result<types::LightningReceive, BarkError> {
        let payment_hash_obj =
            PaymentHash::from_str(&payment_hash).context("invalid payment hash")?;
        Ok(self
            .inner
            .lightning_receive_state(payment_hash_obj)
            .await?
            .into())
    }

    pub async fn try_claim_lightning_receive(
        &self,
        payment_hash: String,
        wait: bool,
    ) -> Result<types::LightningReceive, BarkError> {
        let payment_hash_obj =
            PaymentHash::from_str(&payment_hash).context("invalid payment hash")?;
        let state = self
            .inner
            .try_claim_lightning_receive(payment_hash_obj, wait)
            .await?;
        Ok(state.into())
    }

    pub async fn cancel_lightning_receive(&self, payment_hash: String) -> Result<(), BarkError> {
        let payment_hash_obj =
            PaymentHash::from_str(&payment_hash).context("invalid payment hash")?;
        self.inner
            .cancel_lightning_receive(payment_hash_obj)
            .await?;
        Ok(())
    }

    /// Force-exit an unfinished lightning receive.
    pub async fn attempt_lightning_receive_exit(
        &self,
        payment_hash: String,
    ) -> Result<(), BarkError> {
        let payment_hash_obj =
            PaymentHash::from_str(&payment_hash).context("invalid payment hash")?;
        self.inner
            .attempt_lightning_receive_exit(payment_hash_obj)
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Arkoor
    // ------------------------------------------------------------------------

    /// Send an out-of-round payment to an Ark address.
    ///
    /// An address this bark cannot deliver to (see
    /// [`Self::validate_arkoor_address`]) is rejected up front, leaving the
    /// selected VTXOs spendable, rather than cosigned and then retried forever.
    pub async fn send_arkoor_payment(
        &self,
        ark_address: String,
        amount_sats: u64,
    ) -> Result<u32, BarkError> {
        let addr: ark::Address = ark_address.parse().context("invalid ark address")?;
        let amount = bitcoin::Amount::from_sat(amount_sats);
        Ok(self.inner.send_arkoor_payment(&addr, amount).await?.0)
    }

    /// Whether this wallet can pay `address` out-of-round: same network and
    /// server, a VTXO policy arkoor can pay to, and only delivery mechanisms
    /// this bark supports. An address listing no delivery mechanism at all is
    /// valid — that is the receiver's explicit choice, and a send to it
    /// succeeds without attempting delivery.
    ///
    /// Errors only when `address` does not parse; a `false` means the parsed
    /// address fails one of the checks above. Requires a server connection.
    pub async fn validate_arkoor_address(&self, address: String) -> Result<bool, BarkError> {
        let addr: ark::Address = address.parse().context("invalid ark address")?;
        Ok(self.inner.validate_arkoor_address(&addr).await.is_ok())
    }

    // ------------------------------------------------------------------------
    // Onchain send (from offchain balance)
    // ------------------------------------------------------------------------

    pub async fn send_onchain(
        &self,
        address: String,
        amount_sats: u64,
    ) -> Result<types::OffboardResult, BarkError> {
        let addr = parse_address(&address, self.inner.network().await?)?;
        let amount = bitcoin::Amount::from_sat(amount_sats);
        Ok(self.inner.send_onchain(addr, amount).await?.into())
    }

    // ------------------------------------------------------------------------
    // Payment requests
    // ------------------------------------------------------------------------

    pub async fn parse_payment_request(
        &self,
        input: String,
    ) -> Result<types::PaymentRequest, BarkError> {
        use futures_util::FutureExt;
        let invalid =
            |message: String| BarkError::typed(BarkErrorCode::InvalidPaymentRequest, message);
        // Read the network first, so only parsing failures get the typed code.
        self.inner.network().await?;
        // Upstream unwraps BOLT12 offer amounts, which panics on fiat-priced offers.
        let parsed = std::panic::AssertUnwindSafe(self.inner.parse_payment_request(input.trim()))
            .catch_unwind()
            .await
            .map_err(|_| {
                invalid("unsupported payment request (fiat-priced BOLT12 offer?)".into())
            })?;
        let request = parsed.map_err(|e| invalid(format!("{e:#}")))?;
        if request.options.is_empty() {
            return Err(invalid(
                "no supported payment method in this request".into(),
            ));
        }
        Ok(request.into())
    }

    pub async fn estimate_payment_fee(
        &self,
        method: types::PaymentMethod,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        let option = bark::AvailablePaymentMethod {
            method: method.try_into()?,
            errors: vec![],
        };
        let amount = bitcoin::Amount::from_sat(amount_sats);
        Ok(self
            .inner
            .estimate_payment_fee(&option, amount)
            .await?
            .into())
    }

    pub async fn estimate_payment_fees(
        &self,
        request: types::PaymentRequest,
        amount_sats: Option<u64>,
    ) -> Result<Vec<types::PaymentMethodFee>, BarkError> {
        // Upstream estimates every option regardless of its parse errors.
        let options = request
            .options
            .into_iter()
            .map(|option| {
                Ok(bark::AvailablePaymentMethod {
                    method: option.method.try_into()?,
                    errors: vec![],
                })
            })
            .collect::<Result<_, BarkError>>()?;
        let request = bark::PaymentRequest {
            amount: request.amount_sats.map(bitcoin::Amount::from_sat),
            label: request.label,
            message: request.message,
            options,
        };
        let fees = self
            .inner
            .estimate_payment_fees(request, amount_sats.map(bitcoin::Amount::from_sat))
            .await?;
        Ok(fees
            .into_iter()
            .map(|(option, fee)| types::PaymentMethodFee {
                method: (&option.method).into(),
                fee: fee.into(),
            })
            .collect())
    }

    pub async fn send_payment(
        &self,
        method: types::PaymentMethod,
        amount_sats: Option<u64>,
        comment: Option<String>,
        options: types::LightningSendOptions,
    ) -> Result<types::PaymentResult, BarkError> {
        let method: bark::movement::PaymentMethod = method.try_into()?;
        // Upstream's dispatcher drops the onchain movement id; send_onchain keeps it.
        if let bark::movement::PaymentMethod::Bitcoin(address) = &method {
            let amount = amount_sats.context("amount is required for bitcoin address")?;
            let address = address.assume_checked_ref().to_string();
            let result = self.send_onchain(address, amount).await?;
            return Ok(types::PaymentResult::Onchain {
                txid: result.txid,
                movement_id: result.movement_id,
            });
        }
        let amount = amount_sats.map(bitcoin::Amount::from_sat);
        let output = self
            .inner
            .send_payment_with(&method, amount, comment.as_deref(), options.into())
            .await?;
        Ok(match output {
            bark::PaymentInitOutput::Lightning(invoice) => types::PaymentResult::Lightning {
                status: self.lightning_send_status(invoice).await?,
            },
            bark::PaymentInitOutput::Ark(movement_id) => types::PaymentResult::Ark {
                movement_id: movement_id.0,
            },
            bark::PaymentInitOutput::Onchain(_) => {
                return Err("bitcoin payments are sent above".into());
            }
        })
    }

    pub async fn bip321_uri(
        &self,
        options: types::Bip321UriOptions,
    ) -> Result<types::Bip321Uri, BarkError> {
        let onchain = self.inner.onchain().filter(|_| options.onchain);
        let invoice = options.lightning && options.amount_sats.is_some();
        if !options.ark && !invoice && onchain.is_none() {
            return Err("a BIP 321 URI needs an Ark address, an onchain address or an amount for an invoice".into());
        }
        // Ark addresses and invoices may call the server. Derive the onchain address
        // before them, rather than letting the builder hold the onchain lock throughout.
        let early_address = match &onchain {
            Some(onchain) if options.ark || invoice => {
                let mut onchain = onchain.write().await;
                Some(
                    onchain
                        .address()
                        .await
                        .context("failed to get onchain address")?,
                )
            }
            _ => None,
        };
        let mut guard = match &onchain {
            Some(onchain) if early_address.is_none() => Some(onchain.write().await),
            _ => None,
        };
        // The builder takes `&mut Wallet`; a clone is a handle to the same wallet.
        let mut wallet = self.inner.clone();
        let mut builder = wallet
            .bip321_uri()
            .ark(options.ark)
            .onchain(false)
            .lightning_bolt11(options.lightning);
        if let Some(guard) = guard.as_mut() {
            builder = builder.onchain_wallet(&mut **guard);
        }
        if let Some(amount_sats) = options.amount_sats {
            builder = builder.amount_sat(amount_sats);
        }
        if let Some(label) = options.label {
            builder = builder.label(label);
        }
        if let Some(message) = options.message {
            builder = builder.message(message);
        }
        let mut uri = builder.build().await?;
        drop(guard);
        if let Some(address) = early_address {
            // Where the builder puts it: the address on mainnet, a `tb` parameter elsewhere.
            let address = address.into_unchecked();
            if self.inner.network().await? == bitcoin::Network::Bitcoin {
                uri.set_address(address)
            } else {
                uri.push_tb(address, false)
            }
            .context("failed to add onchain address")?;
        }
        Ok(types::Bip321Uri {
            uri: uri.to_string(),
            ark_address: uri
                .extensions()
                .ark()
                .first()
                .map(|a| a.inner().to_string()),
            onchain_address: uri
                .address()
                .or_else(|| uri.tb().first().map(|a| a.inner()))
                .map(ToString::to_string),
            invoice: uri.lightning().first().map(|invoice| {
                let invoice = invoice.inner();
                types::LightningInvoice {
                    invoice: invoice.to_string(),
                    payment_hash: invoice.payment_hash().to_string(),
                    amount_sats: invoice.amount_milli_satoshis().unwrap_or(0) / 1000,
                }
            }),
        })
    }

    pub async fn debug_info(&self) -> Result<types::DebugInfo, BarkError> {
        let info = self.inner.debug_info().await?;
        Ok(types::DebugInfo {
            network: info.network.into(),
            mailbox_id: hex::encode(info.mailbox_id.serialize()),
            vtxo_xpub: info.vtxo_xpub.to_string(),
        })
    }

    // ------------------------------------------------------------------------
    // History
    // ------------------------------------------------------------------------

    pub async fn history(&self) -> Result<Vec<types::Movement>, BarkError> {
        Ok(self
            .inner
            .history()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn update_history_metadata(
        &self,
        movement_id: u32,
        patch_json: String,
    ) -> Result<(), BarkError> {
        let patch = parse_history_metadata_patch(&patch_json)?;
        self.inner
            .update_history_metadata(bark::movement::MovementId::new(movement_id), &patch)
            .await
            .context("failed to update history metadata")?;
        Ok(())
    }

    pub async fn history_by_payment_method(
        &self,
        payment_method_type: String,
        payment_method_value: String,
    ) -> Result<Vec<types::Movement>, BarkError> {
        let payment_method = bark::movement::PaymentMethod::from_type_value(
            &payment_method_type,
            &payment_method_value,
        )
        .context("Invalid payment method")?;

        Ok(self
            .inner
            .history_by_payment_method(&payment_method)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    // ------------------------------------------------------------------------
    // Refresh
    // ------------------------------------------------------------------------

    pub async fn refresh_vtxos(&self, vtxo_ids: Vec<String>) -> Result<Option<String>, BarkError> {
        let ids: Result<Vec<_>, _> = vtxo_ids
            .iter()
            .map(|id| id.parse::<VtxoId>().context("invalid vtxo id"))
            .collect();
        let result = self.inner.refresh_vtxos(ids?).await?;
        Ok(result.map(|s| format!("{:?}", s)))
    }

    pub async fn maintenance_refresh(&self) -> Result<Option<String>, BarkError> {
        let result = self.inner.maintenance_refresh().await?;
        Ok(result.map(|s| format!("{:?}", s)))
    }

    pub async fn refresh_vtxos_delegated(
        &self,
        vtxo_ids: Vec<String>,
    ) -> Result<Option<types::RoundState>, BarkError> {
        let ids: Result<Vec<_>, _> = vtxo_ids.iter().map(|s| s.parse::<VtxoId>()).collect();
        let ids = ids.context("invalid VTXO id")?;

        let state = self.inner.refresh_vtxos_delegated(ids).await?;
        Ok(state.map(|s| s.into()))
    }

    /// Schedule a delegated refresh for `scheduled_height` instead of the next
    /// round. The refresh fee is priced against the VTXO's remaining lifetime
    /// at that height, and the server's fee table charges less the closer a
    /// VTXO is to expiry, so scheduling further out never costs more than
    /// refreshing now. The height is used verbatim: one at or below the current
    /// tip just prices higher and becomes eligible for the next round, and the
    /// server rejects a height at or past any input VTXO's expiry.
    pub async fn refresh_vtxos_scheduled(
        &self,
        vtxo_ids: Vec<String>,
        scheduled_height: u32,
    ) -> Result<Option<types::RoundState>, BarkError> {
        let ids: Result<Vec<_>, _> = vtxo_ids.iter().map(|s| s.parse::<VtxoId>()).collect();
        let ids = ids.context("invalid VTXO id")?;

        let state = self
            .inner
            .refresh_vtxos_scheduled(ids, scheduled_height.into())
            .await?;
        Ok(state.map(|s| s.into()))
    }

    // ------------------------------------------------------------------------
    // Recovery
    // ------------------------------------------------------------------------

    /// Recover the given VTXO ids from the server: fetch each one, keep the
    /// ones this wallet owns and that are still spendable, and import them.
    ///
    /// Takes known ids only — the full seed-derived mailbox rescan is internal
    /// to bark and runs at wallet open (see [`Self::recovery_status`]). Use this
    /// to retry ids a previous scan reported as `failed`.
    ///
    /// `gap_limit` overrides [`Config::vtxo_key_gap_limit`] for the key scan
    /// that decides which of `vtxo_ids` this wallet owns. Widen it to reach ids
    /// a previous scan bucketed as `foreign`.
    pub async fn recover_vtxos(
        &self,
        vtxo_ids: Vec<String>,
        gap_limit: Option<u32>,
    ) -> Result<types::RecoveryReport, BarkError> {
        let ids: Result<Vec<_>, _> = vtxo_ids
            .iter()
            .map(|id| id.parse::<VtxoId>().context("invalid vtxo id"))
            .collect();

        let report = self.inner.recover_vtxos(ids?, gap_limit).await?;
        Ok((&report).into())
    }

    // ------------------------------------------------------------------------
    // Info
    // ------------------------------------------------------------------------

    pub async fn properties(&self) -> Result<types::WalletProperties, BarkError> {
        Ok(self.inner.properties().await?.into())
    }

    pub fn fingerprint(&self) -> String {
        self.inner.fingerprint().to_string()
    }

    pub async fn network(&self) -> Result<types::Network, BarkError> {
        Ok(self.inner.network().await?.into())
    }

    pub fn config(&self) -> Config {
        let cfg = self.inner.config();
        Config {
            server_address: cfg.server_address.clone(),
            // Credentials are write-only across the FFI: upstream keeps the
            // password in a Secret so it stays out of Debug, and handing it
            // back here put it straight into a printable record.
            #[allow(deprecated)]
            server_access_token: None,
            esplora_address: cfg.esplora_address.clone(),
            bitcoind_address: cfg.bitcoind_address.clone(),
            bitcoind_cookiefile: cfg
                .bitcoind_cookiefile
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            bitcoind_user: cfg.bitcoind_user.clone(),
            bitcoind_pass: None,
            vtxo_refresh_expiry_threshold: Some(cfg.vtxo_refresh_expiry_threshold.into()),
            vtxo_exit_margin: Some(cfg.vtxo_exit_margin.into()),
            htlc_recv_claim_delta: Some(cfg.htlc_recv_claim_delta.into()),
            fallback_fee_rate: cfg.fallback_fee_rate.map(|r| r.to_sat_per_kwu()),
            round_tx_required_confirmations: Some(cfg.round_tx_required_confirmations.into()),
            daemon_sync_interval_secs: Some(cfg.daemon_sync_interval_secs),
            offboard_required_confirmations: Some(cfg.offboard_required_confirmations.into()),
            daemon_manual_sync: Some(cfg.daemon_manual_sync),
            lightning_receive_claim_retries: Some(cfg.lightning_receive_claim_retries),
            user_agent: cfg.user_agent.clone(),
            vtxo_key_gap_limit: Some(cfg.vtxo_key_gap_limit),
        }
    }

    pub async fn ark_info(&self) -> Option<types::ArkInfo> {
        match self.inner.ark_info().await {
            Ok(Some(info)) => Some((&info).into()),
            _ => None,
        }
    }

    pub async fn next_round_start_time(&self) -> Result<u64, BarkError> {
        let system_time = self.inner.next_round_start_time().await?;
        Ok(system_time
            .duration_since(std::time::UNIX_EPOCH)
            .expect("next round time should be after Unix epoch")
            .as_secs())
    }

    // ------------------------------------------------------------------------
    // Boarding (requires onchain wallet)
    // ------------------------------------------------------------------------

    pub async fn board_amount(&self, amount_sats: u64) -> Result<types::PendingBoard, BarkError> {
        let _operation = super::prepared_onchain::operation_guard(self.inner.onchain()).await;
        let amount = bitcoin::Amount::from_sat(amount_sats);
        info!("[BOARD] Boarding {} sats into Ark...", amount_sats);

        let pb = self
            .inner
            .board_amount(amount)
            .await
            .context("Board failed")?;

        let txid = pb.funding_tx.compute_txid();
        let vtxo_id = pb.vtxos.first().map(|v| v.to_string()).unwrap_or_default();
        info!(
            "[BOARD] Board transaction created: {} (VTXO ID: {})",
            txid, vtxo_id
        );

        Ok(pb.into())
    }

    pub async fn board_all(&self) -> Result<types::PendingBoard, BarkError> {
        let _operation = super::prepared_onchain::operation_guard(self.inner.onchain()).await;
        info!("[BOARD] Boarding ALL funds into Ark...");

        let pb = self.inner.board_all().await.context("Board all failed")?;

        let txid = pb.funding_tx.compute_txid();
        let vtxo_id = pb.vtxos.first().map(|v| v.to_string()).unwrap_or_default();
        info!(
            "[BOARD] Board transaction created: {} (VTXO ID: {}, amount: {} sats)",
            txid,
            vtxo_id,
            pb.amount.to_sat()
        );

        Ok(pb.into())
    }

    pub async fn board_funding_address(&self) -> Result<types::BoardFundingInfo, BarkError> {
        let (keypair, keypair_index) = self
            .inner
            .derive_store_next_keypair()
            .await
            .context("Failed to derive board keypair")?;

        let (address, expiry_height) = self
            .inner
            .board_funding_address(&keypair)
            .await
            .context("Failed to get board funding address")?;

        info!(
            "[BOARD] Board funding address: {} (keypair index: {}, expiry height: {})",
            address, keypair_index, expiry_height
        );

        Ok(types::BoardFundingInfo {
            address: address.to_string(),
            expiry_height: expiry_height.into(),
            keypair_index,
        })
    }

    pub async fn board_psbt(
        &self,
        psbt_base64: String,
        keypair_index: u32,
        expiry_height: u32,
    ) -> Result<types::PendingBoard, BarkError> {
        let _operation = super::prepared_onchain::operation_guard(self.inner.onchain()).await;
        use bitcoin::base64::prelude::*;
        use bitcoin::psbt::Psbt;

        info!(
            "[BOARD] Boarding from funding PSBT (keypair index: {}, expiry height: {})...",
            keypair_index, expiry_height
        );

        let psbt_bytes = BASE64_STANDARD
            .decode(&psbt_base64)
            .context("Invalid base64")?;
        let psbt = Psbt::deserialize(&psbt_bytes).context("Invalid PSBT")?;
        super::prepared_onchain::check_board_inputs(self.inner.onchain(), &psbt.unsigned_tx)
            .await?;

        let keypair = self
            .inner
            .peek_keypair(keypair_index)
            .await
            .context("Unknown board keypair index")?;

        let pb = self
            .inner
            .board_psbt(psbt, keypair, expiry_height.into())
            .await
            .context("Board psbt failed")?;

        let txid = pb.funding_tx.compute_txid();
        let vtxo_id = pb.vtxos.first().map(|v| v.to_string()).unwrap_or_default();
        info!(
            "[BOARD] Board transaction created: {} (VTXO ID: {}, amount: {} sats)",
            txid,
            vtxo_id,
            pb.amount.to_sat()
        );

        Ok(pb.into())
    }

    pub async fn sync_pending_boards(&self) -> Result<(), BarkError> {
        info!("[BOARD] Syncing pending boards...");
        self.inner
            .sync_pending_boards()
            .await
            .context("Sync pending boards failed")?;
        info!("[BOARD] Pending boards synced");
        Ok(())
    }

    pub async fn pending_boards(&self) -> Result<Vec<types::PendingBoard>, BarkError> {
        Ok(self
            .inner
            .pending_boards()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn pending_board_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        Ok(self
            .inner
            .pending_board_vtxos()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn pending_round_input_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        Ok(self
            .inner
            .pending_round_input_vtxos()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn pending_lightning_send_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        Ok(self
            .inner
            .pending_lightning_send_vtxos()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    // ------------------------------------------------------------------------
    // Unilateral Exits (requires onchain wallet)
    // ------------------------------------------------------------------------

    pub async fn start_exit_for_entire_wallet(&self) -> Result<(), BarkError> {
        info!("[EXIT] Starting unilateral exit for entire wallet...");
        self.inner
            .exit_mgr()
            .start_exit_for_entire_wallet()
            .await
            .context("Start exit failed")?;
        info!("[EXIT] Exit initiated - call sync_exits() periodically to progress");
        Ok(())
    }

    pub async fn sync_exits(&self) -> Result<(), BarkError> {
        info!("[EXIT] Syncing exits...");
        self.inner.sync_exits().await.context("Sync exits failed")?;
        info!("[EXIT] Exits synced");
        Ok(())
    }

    pub async fn progress_exits(
        &self,
        fee_rate_sat_per_vb: Option<u64>,
    ) -> Result<Vec<types::ExitProgressStatus>, BarkError> {
        info!("[EXIT] Progressing exits...");

        let fee_rate = fee_rate_sat_per_vb.and_then(bitcoin::FeeRate::from_sat_per_vb);

        let result = self
            .inner
            .exit_mgr()
            .progress_exits_with_cpfp(&self.inner, fee_rate)
            .await
            .context("Progress exits failed")?;

        let statuses = result
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();
        info!("[EXIT] Exits progressed");
        Ok(statuses)
    }

    /// Parse VTXO id strings and load each VTXO from the wallet in bare form.
    ///
    /// Ids are parsed up front so a malformed one fails before any lookup runs.
    async fn bare_vtxos_by_id(
        &self,
        vtxo_ids: &[String],
    ) -> Result<Vec<ark::Vtxo<ark::vtxo::Bare>>, BarkError> {
        let ids = parse_vtxo_ids(vtxo_ids)?;

        let mut vtxos = Vec::with_capacity(ids.len());
        for id in ids {
            let vtxo = self
                .inner
                .get_vtxo_by_id(id)
                .await
                .with_context(|| format!("VTXO not found: {}", id))?;
            vtxos.push(vtxo.vtxo.to_bare());
        }

        Ok(vtxos)
    }

    pub async fn start_exit_for_vtxos(&self, vtxo_ids: Vec<String>) -> Result<(), BarkError> {
        info!("[EXIT] Starting exit for {} VTXOs...", vtxo_ids.len());

        let vtxos = self.bare_vtxos_by_id(&vtxo_ids).await?;

        self.inner
            .exit_mgr()
            .start_exit_for_vtxos(&vtxos)
            .await
            .context("Start exit for VTXOs failed")?;

        info!("[EXIT] Exit initiated for {} VTXOs", vtxo_ids.len());
        Ok(())
    }

    /// Like [`Self::start_exit_for_vtxos`], but skips dust and standardness
    /// checks.
    ///
    /// Only use this when the VTXOs are already onchain, or when broadcasting
    /// through a node that accepts non-standard transactions — otherwise the
    /// resulting exit transactions may be unrelayable.
    pub async fn start_exit_for_vtxos_including_non_standard(
        &self,
        vtxo_ids: Vec<String>,
    ) -> Result<(), BarkError> {
        info!(
            "[EXIT] Starting exit (non-standard allowed) for {} VTXOs...",
            vtxo_ids.len(),
        );

        let vtxos = self.bare_vtxos_by_id(&vtxo_ids).await?;

        self.inner
            .exit_mgr()
            .start_exit_for_vtxos_including_non_standard(&vtxos)
            .await
            .context("Start exit for VTXOs failed")?;

        info!("[EXIT] Exit initiated for {} VTXOs", vtxo_ids.len());
        Ok(())
    }

    /// Cancel a unilateral exit that is still in its abortable window.
    ///
    /// Starting an exit never changes the VTXO's state, so there is nothing to
    /// undo on the VTXO side: it stays spendable and a fresh exit can be
    /// started later. If the caller locked the VTXO themselves after starting
    /// the exit (see [`Self::lock_vtxos`]), they are responsible for unlocking
    /// it — this method deliberately does not, since it cannot know whether the
    /// lock was theirs.
    ///
    /// Cancelling an already-cancelled exit succeeds, so retries are safe.
    ///
    /// Expected refusals (already claimed, never exiting, transaction already
    /// broadcast) come back in the returned [`types::ExitCancelResult`] rather
    /// than as errors; `Err` is reserved for genuine faults such as a database
    /// failure or an unreachable chain source.
    pub async fn cancel_exit(&self, vtxo_id: String) -> Result<types::ExitCancelResult, BarkError> {
        use bark::exit::ExitError;

        let id = vtxo_id.parse::<VtxoId>().context("invalid vtxo id")?;

        info!("[EXIT] Cancelling exit for VTXO {}...", id);

        match self.inner.exit_mgr().cancel_exit(id).await {
            Ok(()) => {
                info!("[EXIT] Exit cancelled for VTXO {}", id);
                Ok(types::ExitCancelResult::canceled())
            }
            Err(ExitError::NotExiting { .. }) => {
                info!("[EXIT] VTXO {} has no exit to cancel", id);
                Ok(types::ExitCancelResult::refused(
                    types::ExitCancelFailure::NotExiting,
                ))
            }
            Err(ExitError::CannotCancelExit { state, .. }) => {
                info!(
                    "[EXIT] Exit for VTXO {} is past cancellation ({:?})",
                    id, state
                );
                Ok(types::ExitCancelResult::refused(
                    types::ExitCancelFailure::TooLate {
                        state: state.into(),
                    },
                ))
            }
            Err(ExitError::ExitTxAlreadyBroadcast { txid, .. }) => {
                info!("[EXIT] Exit tx {} for VTXO {} already broadcast", txid, id);
                Ok(types::ExitCancelResult::refused(
                    types::ExitCancelFailure::AlreadyBroadcast {
                        txid: txid.to_string(),
                    },
                ))
            }
            Err(e) => Err(BarkError::from(anyhow::Error::new(e))),
        }
    }

    pub async fn list_claimable_exits(&self) -> Result<Vec<types::ExitVtxo>, BarkError> {
        let exit_guard = self.inner.exit_mgr();
        let claimable = exit_guard.list_claimable().await;
        Ok(claimable.iter().map(Into::into).collect())
    }

    /// Stored live and terminal exits, including rows no longer loaded in memory.
    pub async fn list_all_exits(
        &self,
        include_history: bool,
        include_transactions: bool,
    ) -> Result<Vec<types::ExitTransactionStatus>, BarkError> {
        let statuses = self
            .inner
            .exit_mgr()
            .list_all(include_history, include_transactions)
            .await?;
        let mut result = Vec::with_capacity(statuses.len());
        for status in statuses {
            let amount = self
                .inner
                .get_vtxo_by_id(status.vtxo_id)
                .await?
                .amount()
                .to_sat();
            result.push(types::ExitTransactionStatus::from_bark(status, amount));
        }
        Ok(result)
    }

    pub async fn transaction_status(
        &self,
        txid: String,
    ) -> Result<types::TransactionStatus, BarkError> {
        Ok(self
            .inner
            .chain()
            .tx_status(txid.parse().context("invalid transaction id")?)
            .await?
            .into())
    }

    pub async fn get_exit_vtxos(&self) -> Result<Vec<types::ExitVtxo>, BarkError> {
        let exit_guard = self.inner.exit_mgr();
        Ok(exit_guard
            .get_exit_vtxos()
            .await
            .iter()
            .map(Into::into)
            .collect())
    }

    pub async fn has_pending_exits(&self) -> Result<bool, BarkError> {
        let exit_guard = self.inner.exit_mgr();
        Ok(exit_guard.has_pending_exits().await)
    }

    pub async fn pending_exits_total_sats(&self) -> Result<u64, BarkError> {
        Ok(self.inner.balance().await?.pending_exit.to_sat())
    }

    pub async fn all_exits_claimable_at_height(&self) -> Result<Option<u32>, BarkError> {
        let exit_guard = self.inner.exit_mgr();
        Ok(exit_guard.all_claimable_at_height().await.map(Into::into))
    }

    pub async fn get_exit_status(
        &self,
        vtxo_id: String,
        include_history: bool,
        include_transactions: bool,
    ) -> Result<Option<types::ExitTransactionStatus>, BarkError> {
        let vtxo_id_parsed = vtxo_id.parse::<VtxoId>().context("invalid vtxo id")?;

        let exit_guard = self.inner.exit_mgr();
        let status = exit_guard
            .get_exit_status(vtxo_id_parsed, include_history, include_transactions)
            .await
            .context("Get exit status failed")?;
        match status {
            Some(status) => {
                let amount = self
                    .inner
                    .get_vtxo_by_id(status.vtxo_id)
                    .await?
                    .amount()
                    .to_sat();
                Ok(Some(types::ExitTransactionStatus::from_bark(
                    status, amount,
                )))
            }
            None => Ok(None),
        }
    }

    /// Build a PSBT claiming exited VTXOs to `address`.
    ///
    /// Draining everything must be asked for with `drain_all`, so an id list a
    /// caller filtered down to nothing is an error rather than a sweep. Ids are
    /// parsed all-or-nothing; well-formed ids that are not claimable are
    /// skipped.
    pub async fn drain_exits(
        &self,
        vtxo_ids: Vec<String>,
        drain_all: bool,
        address: String,
        fee_rate_sat_per_vb: Option<u64>,
    ) -> Result<types::ExitClaimTransaction, BarkError> {
        info!("[EXIT] Draining {} exits to {}...", vtxo_ids.len(), address);

        let addr = parse_address(&address, self.inner.network().await?)?;

        let fee_rate = fee_rate_sat_per_vb.and_then(bitcoin::FeeRate::from_sat_per_vb);

        let exit_guard = self.inner.exit_mgr();
        let claimable = exit_guard.list_claimable().await;

        let to_drain: Vec<_> = match (drain_all, vtxo_ids.is_empty()) {
            (true, true) => claimable,
            (true, false) => {
                return Err("pass either drain_all or a list of vtxo ids, not both".into())
            }
            (false, true) => {
                return Err("no vtxo ids given; pass drain_all to claim every exit".into())
            }
            (false, false) => {
                // All-or-nothing: a malformed id used to be dropped silently.
                let requested_ids: std::collections::HashSet<_> =
                    parse_vtxo_ids(&vtxo_ids)?.into_iter().collect();
                claimable
                    .into_iter()
                    .filter(|ev| requested_ids.contains(&ev.id()))
                    .collect()
            }
        };

        if to_drain.is_empty() {
            return Err(anyhow::anyhow!("No claimable exits found").into());
        }

        let psbt = exit_guard
            .drain_exits(&to_drain, &self.inner, addr, fee_rate)
            .await
            .context("Drain exits failed")?;

        let fee_sats = psbt.fee().context("Failed to get fee")?;

        let psbt_bytes = psbt.serialize();
        use bitcoin::base64::prelude::*;
        let psbt_base64 = BASE64_STANDARD.encode(&psbt_bytes);

        info!(
            "[EXIT] Drain PSBT created (fee: {} sats)",
            fee_sats.to_sat()
        );

        Ok(types::ExitClaimTransaction {
            psbt_base64,
            fee_sats: fee_sats.to_sat(),
        })
    }

    pub async fn sign_exit_claim_inputs(&self, psbt_base64: String) -> Result<String, BarkError> {
        use bitcoin::base64::prelude::*;
        use bitcoin::psbt::Psbt;

        let psbt_bytes = BASE64_STANDARD
            .decode(&psbt_base64)
            .context("Invalid base64")?;
        let mut psbt = Psbt::deserialize(&psbt_bytes).context("Invalid PSBT")?;

        let exit_guard = self.inner.exit_mgr();
        exit_guard
            .sign_exit_claim_inputs(&mut psbt, &self.inner)
            .await
            .context("Sign exit claim inputs failed")?;

        let signed_psbt_bytes = psbt.serialize();
        Ok(BASE64_STANDARD.encode(&signed_psbt_bytes))
    }

    // ------------------------------------------------------------------------
    // Round Management
    // ------------------------------------------------------------------------

    pub async fn pending_round_states(&self) -> Result<Vec<types::RoundState>, BarkError> {
        Ok(self
            .inner
            .pending_round_states()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn sync_pending_rounds(&self) -> Result<Vec<types::PendingRoundStatus>, BarkError> {
        let mut statuses: Vec<_> = self
            .inner
            .sync_pending_rounds()
            .await?
            .into_iter()
            .map(|(id, status)| types::PendingRoundStatus::new(id.0, status))
            .collect();
        statuses.sort_by_key(|status| status.round_id);
        Ok(statuses)
    }

    pub async fn cancel_pending_round(&self, round_id: u32) -> Result<(), BarkError> {
        use bark::persist::models::RoundStateId;
        self.inner
            .cancel_pending_round(RoundStateId(round_id))
            .await?;
        Ok(())
    }

    pub async fn cancel_all_pending_rounds(&self) -> Result<(), BarkError> {
        self.inner.cancel_all_pending_rounds().await?;
        Ok(())
    }

    pub async fn progress_pending_rounds(&self) -> Result<(), BarkError> {
        self.inner.progress_pending_rounds(None).await?;
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Server
    // ------------------------------------------------------------------------

    pub async fn refresh_server(&self) -> Result<(), BarkError> {
        self.inner.refresh_server().await?;
        Ok(())
    }

    // ------------------------------------------------------------------------
    // VTXO Expiry & Refresh Scheduling
    // ------------------------------------------------------------------------

    pub async fn get_first_expiring_vtxo_blockheight(&self) -> Result<Option<u32>, BarkError> {
        Ok(self
            .inner
            .get_first_expiring_vtxo_blockheight()
            .await?
            .map(Into::into))
    }

    pub async fn get_next_required_refresh_blockheight(&self) -> Result<Option<u32>, BarkError> {
        Ok(self
            .inner
            .get_next_required_refresh_blockheight()
            .await?
            .map(Into::into))
    }

    // ------------------------------------------------------------------------
    // Broadcasting
    // ------------------------------------------------------------------------

    pub async fn broadcast_tx(&self, tx_hex: String) -> Result<String, BarkError> {
        use bitcoin::consensus::encode::deserialize_hex;
        use bitcoin::Transaction;

        let tx: Transaction = deserialize_hex(&tx_hex).context("invalid transaction")?;

        let txid = tx.compute_txid();
        info!("[BROADCAST] Broadcasting transaction: {}", txid);

        self.inner
            .chain()
            .broadcast_tx(&tx)
            .await
            .context("Broadcast failed")?;

        info!("[BROADCAST] Transaction broadcasted successfully");
        Ok(txid.to_string())
    }

    // ------------------------------------------------------------------------
    // Mailbox identifiers (sync, no run_async)
    // ------------------------------------------------------------------------

    pub fn mailbox_identifier(&self) -> Result<String, BarkError> {
        let identifier = self.inner.mailbox_identifier();
        Ok(hex::encode(identifier.serialize()))
    }

    /// Hex-encoded authorization to read this wallet's mailbox.
    pub fn mailbox_authorization(&self, expiry_secs: u32) -> Result<String, BarkError> {
        let auth = self
            .inner
            .mailbox_authorization(mailbox_auth_expiry(expiry_secs)?);
        Ok(hex::encode(auth.serialize()))
    }

    pub fn mailbox_authorization_at(
        &self,
        authorization_expiry: i64,
    ) -> Result<types::MailboxAuthorization, BarkError> {
        let expiry = DateTime::from_timestamp(authorization_expiry, 0)
            .context("invalid authorization expiry")?
            .with_timezone(&Local);
        let auth = self.inner.mailbox_authorization(expiry);
        Ok(types::MailboxAuthorization {
            mailbox_id: auth.mailbox().to_string(),
            expiry: auth.expiry().timestamp(),
            encoded: hex::encode(auth.serialize()),
        })
    }

    // ------------------------------------------------------------------------
    // VTXO Import / Export
    // ------------------------------------------------------------------------

    /// Import a VTXO from its serialized form, as produced by
    /// [`Wallet::vtxo_encoded`] or bark-rest `GET /vtxos/{id}/encoded`.
    /// Accepts hex as well as base64.
    ///
    /// The VTXO is stored in the state the server reports for it, so one that
    /// was already spent is recorded as spent rather than refused. See
    /// [`types::ImportVtxoArgs`] for the knobs; pass `None` for the defaults.
    pub async fn import_vtxo(
        &self,
        encoded_vtxo: String,
        args: Option<types::ImportVtxoArgs>,
    ) -> Result<(), BarkError> {
        let vtxo = parse_vtxo(&encoded_vtxo)?;

        self.inner
            .import_vtxo(&vtxo, args.unwrap_or_default().into())
            .await
            .context("Failed to import VTXO")?;

        info!("[IMPORT] VTXO imported successfully");
        Ok(())
    }

    /// Import several VTXOs under a single key scan and a single write, which
    /// is why this is not just a loop over [`Self::import_vtxo`].
    ///
    /// Returns the ids now held — whether this call stored them or found them
    /// already present — so a failed batch can be retried. One VTXO that cannot
    /// be imported discards the whole batch unless
    /// [`types::ImportVtxoArgs::allow_partial`] is set, in which case the ones
    /// that did import are kept and the failures are logged.
    pub async fn import_vtxos(
        &self,
        encoded_vtxos: Vec<String>,
        args: Option<types::ImportVtxoArgs>,
    ) -> Result<Vec<String>, BarkError> {
        let vtxos = encoded_vtxos
            .iter()
            .map(|encoded| parse_vtxo(encoded))
            .collect::<Result<Vec<_>, _>>()?;

        let ids = self
            .inner
            .import_vtxos(&vtxos, args.unwrap_or_default().into())
            .await
            .context("Failed to import VTXOs")?;

        info!("[IMPORT] {} VTXO(s) held after import", ids.len());
        Ok(ids.iter().map(|id| id.to_string()).collect())
    }

    /// Hex-encoded serialization of the full VTXO (genesis chain included),
    /// re-importable via [`Wallet::import_vtxo`]. Mirrors bark-rest
    /// `GET /vtxos/{id}/encoded`.
    pub async fn vtxo_encoded(&self, vtxo_id: String) -> Result<String, BarkError> {
        let id: VtxoId = vtxo_id.parse().context("invalid vtxo id")?;

        let vtxo = self
            .inner
            .get_full_vtxo(id)
            .await
            .context("Failed to export VTXO")?;

        Ok(vtxo.serialize_hex())
    }

    // ------------------------------------------------------------------------
    // Fee Estimation
    // ------------------------------------------------------------------------

    pub async fn estimate_board_fee(
        &self,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        let amount = bitcoin::Amount::from_sat(amount_sats);
        let estimate = self.inner.estimate_board_offchain_fee(amount).await?;
        Ok(estimate.into())
    }

    pub async fn estimate_offboard_fee(
        &self,
        address: String,
        vtxo_ids: Vec<String>,
    ) -> Result<types::FeeEstimate, BarkError> {
        let btc_addr = parse_address(&address, self.inner.network().await?)?;

        let ids: Result<Vec<_>, _> = vtxo_ids
            .iter()
            .map(|id| id.parse::<VtxoId>().context("invalid vtxo id"))
            .collect();
        let ids = ids?;

        let mut vtxos = Vec::new();
        for id in ids {
            let vtxo = self
                .inner
                .get_vtxo_by_id(id)
                .await
                .context("VTXO not found")?;
            vtxos.push(vtxo);
        }

        let estimate = self.inner.estimate_offboard(&btc_addr, &vtxos).await?;
        Ok(estimate.into())
    }

    pub async fn estimate_refresh_fee(
        &self,
        vtxo_ids: Vec<String>,
    ) -> Result<types::FeeEstimate, BarkError> {
        let ids: Result<Vec<_>, _> = vtxo_ids
            .iter()
            .map(|id| id.parse::<VtxoId>().context("invalid vtxo id"))
            .collect();
        let ids = ids?;

        let mut vtxos = Vec::new();
        for id in ids {
            let vtxo = self
                .inner
                .get_vtxo_by_id(id)
                .await
                .context("VTXO not found")?;
            vtxos.push(vtxo);
        }

        let estimate = self.inner.estimate_refresh_fee(&vtxos).await?;
        Ok(estimate.into())
    }

    pub async fn estimate_lightning_send_fee(
        &self,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        let amount = bitcoin::Amount::from_sat(amount_sats);
        let estimate = self.inner.estimate_lightning_send_fee(amount).await?;
        Ok(estimate.into())
    }

    pub async fn estimate_lightning_receive_fee(
        &self,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        let amount = bitcoin::Amount::from_sat(amount_sats);
        let estimate = self.inner.estimate_lightning_receive_fee(amount).await?;
        Ok(estimate.into())
    }

    pub async fn estimate_arkoor_payment_fee(
        &self,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        let amount = bitcoin::Amount::from_sat(amount_sats);
        let estimate = self.inner.estimate_arkoor_payment_fee(amount).await?;
        Ok(estimate.into())
    }

    pub async fn estimate_offboard_all_fee(
        &self,
        address: String,
    ) -> Result<types::FeeEstimate, BarkError> {
        let btc_addr = parse_address(&address, self.inner.network().await?)?;

        let estimate = self.inner.estimate_offboard_all(&btc_addr).await?;
        Ok(estimate.into())
    }

    pub async fn estimate_send_onchain_fee(
        &self,
        address: String,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        let btc_addr = parse_address(&address, self.inner.network().await?)?;

        let amount = bitcoin::Amount::from_sat(amount_sats);
        let estimate = self.inner.estimate_send_onchain(&btc_addr, amount).await?;
        Ok(estimate.into())
    }

    /// Estimate the onchain cost of unilaterally (emergency) exiting VTXOs.
    ///
    /// Mirrors bark-rest `GET /exits/fee`. An empty `vtxo_ids` prices exiting
    /// every unspent (spendable or locked) VTXO bark's whole-wallet exit would start, i.e. it skips
    /// non-standard (including dust) VTXOs and lists them in
    /// `excluded_vtxos`. `fee_rate_sat_per_vb`
    /// applies to both legs; when `None`, the broadcast leg uses the chain's
    /// *fast* rate and the claim leg its *regular* rate. `destination` only
    /// affects the claim transaction's weight; when `None` a placeholder P2TR
    /// address on the wallet's network is used.
    ///
    /// The estimate reflects current chain state: exit transactions already
    /// confirmed cost nothing. The estimate uses pure weight arithmetic,
    /// with the upstream default margin unless `fee_margin` is supplied.
    pub async fn estimate_emergency_exit_fee(
        &self,
        vtxo_ids: Vec<String>,
        fee_rate_sat_per_vb: Option<u64>,
        destination: Option<String>,
        fee_margin: Option<f64>,
    ) -> Result<types::EmergencyExitFeeEstimate, BarkError> {
        let mut excluded = Vec::new();
        let ids = if vtxo_ids.is_empty() {
            // Same eligibility as bark's start_exit_for_entire_wallet: unspent (spendable or
            // locked) VTXOs, skipping non-standard ones.
            let mut ids = Vec::new();
            for wallet_vtxo in self
                .inner
                .vtxos()
                .await
                .context("Failed to list unspent VTXOs")?
            {
                let vtxo = wallet_vtxo.vtxo;
                let refusal = match self.inner.get_full_vtxo(vtxo.id()).await {
                    Ok(full) => full.check_standard().err().map(|error| {
                        crate::error::exit_error(&bark::exit::ExitError::NonStandardVtxo {
                            vtxo: vtxo.id(),
                            error,
                        })
                    }),
                    Err(error) => Some(BarkError::from(error)),
                };
                match refusal {
                    None => ids.push(vtxo.id()),
                    Some(error) => excluded.push(types::ExcludedExitVtxo {
                        vtxo_id: vtxo.id().to_string(),
                        amount_sats: vtxo.amount().to_sat(),
                        code: error.code,
                        message: error.message,
                    }),
                }
            }
            ids
        } else {
            parse_vtxo_ids(&vtxo_ids)?
        };

        let fee_rate = match fee_rate_sat_per_vb {
            Some(v) => Some(bitcoin::FeeRate::from_sat_per_vb(v).ok_or_else(|| {
                BarkError::typed(BarkErrorCode::InvalidFeeRate, "Fee rate too large")
            })?),
            None => None,
        };

        let destination = match destination {
            Some(s) => Some(parse_address(&s, self.inner.network().await?)?),
            None => None,
        };

        let estimate = self
            .inner
            .estimate_emergency_exit_fee(&ids, fee_rate, destination, fee_margin)
            .await
            .context("Failed to estimate emergency exit fee")?;
        Ok(types::EmergencyExitFeeEstimate {
            excluded_vtxos: excluded,
            ..estimate.into()
        })
    }
}

/// Absolute expiry `expiry_secs` from now.
fn mailbox_auth_expiry(expiry_secs: u32) -> Result<DateTime<Local>, BarkError> {
    Local::now()
        .checked_add_signed(Duration::seconds(expiry_secs as i64))
        .context("mailbox authorization expiry out of range")
        .map_err(BarkError::from)
}

/// Parse a VTXO from its serialized form.
///
/// Hex is the canonical encoding ([`Wallet::vtxo_encoded`] and bark-rest use
/// it); base64 is also accepted since `import_vtxo` historically took it.
pub(crate) fn parse_vtxo(encoded: &str) -> Result<ark::Vtxo, BarkError> {
    let encoded = encoded.trim();

    if let Ok(vtxo) = ark::Vtxo::deserialize_hex(encoded) {
        return Ok(vtxo);
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("Invalid VTXO encoding: expected hex or base64")?;
    Ok(ark::Vtxo::deserialize(&bytes).context("Invalid VTXO data")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config with only the required field set.
    fn minimal_config() -> crate::config::Config {
        crate::config::Config {
            server_address: "http://127.0.0.1:3535".to_owned(),
            server_access_token: None,
            esplora_address: None,
            bitcoind_address: None,
            bitcoind_cookiefile: None,
            bitcoind_user: None,
            bitcoind_pass: None,
            vtxo_refresh_expiry_threshold: None,
            vtxo_exit_margin: None,
            htlc_recv_claim_delta: None,
            fallback_fee_rate: None,
            round_tx_required_confirmations: None,
            daemon_sync_interval_secs: None,
            offboard_required_confirmations: None,
            daemon_manual_sync: None,
            lightning_receive_claim_retries: None,
            user_agent: None,
            vtxo_key_gap_limit: None,
        }
    }

    /// Current-version board VTXO test vector, copied from bark's
    /// `ark::test_util::VTXO_VECTORS` (`lib/src/test_util/vectors.rs`).
    /// Deserializing and re-serializing it is hex-identical.
    const BOARD_VTXO_HEX: &str = "02001027000000000000928a01000365a81233741893bbe2461b8d479dadc5880594fe6f7479180d5843820af72b62e0075111d0df3738fe77c8f05b0f71292ae6ae5eddf911f8d2bb4dbde598fcbe768f00000000010102030a752219f1b94bbdf8994a0a980cdda08c2ad094cb29dd834878db6dee1612ee0365a81233741893bbe2461b8d479dadc5880594fe6f7479180d5843820af72b629c9c63d9c0f739011368e00c2441d85816c01d637da8d57ac343c95982dd3604d8bf847bcf8a2aac44483d7ea01ec9a99f7720d0694cb3cdd3cbcca97504adaf01004a0100000000000000030a752219f1b94bbdf8994a0a980cdda08c2ad094cb29dd834878db6dee1612ee24e9a421d9018690eea79b11e4e4fe59d36aa8f46d110017e09abe350b5e315600000000";

    #[test]
    fn mailbox_auth_expiry_counts_from_now() {
        let assert_offset = |secs: u32, want: Duration| {
            let offset = mailbox_auth_expiry(secs).unwrap() - Local::now();
            assert!(offset > want - Duration::seconds(5), "{offset}");
            assert!(offset <= want, "{offset}");
        };

        assert_offset(60, Duration::seconds(60));
        assert_offset(24 * 60 * 60, Duration::hours(24));
    }

    /// `u32::MAX` seconds is ~136 years, which has to resolve rather than
    /// overflow.
    #[test]
    fn mailbox_auth_expiry_accepts_the_longest_expiry() {
        assert!(mailbox_auth_expiry(u32::MAX).unwrap() > Local::now());
    }

    #[test]
    fn parse_vtxo_accepts_hex_and_roundtrips() {
        let vtxo = parse_vtxo(BOARD_VTXO_HEX).unwrap();
        assert_eq!(vtxo.serialize_hex(), BOARD_VTXO_HEX);
    }

    #[test]
    fn parse_vtxo_accepts_uppercase_hex() {
        let vtxo = parse_vtxo(&BOARD_VTXO_HEX.to_uppercase()).unwrap();
        assert_eq!(vtxo.serialize_hex(), BOARD_VTXO_HEX);
    }

    #[test]
    fn parse_vtxo_accepts_base64() {
        let from_hex = parse_vtxo(BOARD_VTXO_HEX).unwrap();
        let base64_vtxo = base64::engine::general_purpose::STANDARD.encode(from_hex.serialize());

        let vtxo = parse_vtxo(&base64_vtxo).unwrap();
        assert_eq!(vtxo.id(), from_hex.id());
        assert_eq!(vtxo.serialize_hex(), BOARD_VTXO_HEX);
    }

    #[test]
    fn parse_vtxo_trims_whitespace() {
        let vtxo = parse_vtxo(&format!("  {}\n", BOARD_VTXO_HEX)).unwrap();
        assert_eq!(vtxo.serialize_hex(), BOARD_VTXO_HEX);
    }

    /// `vtxo_encoded` takes the id string shown in the VTXO DTO
    /// (`vtxo.id().to_string()`); it must parse back to the same `VtxoId`.
    #[test]
    fn vtxo_id_string_roundtrips() {
        let vtxo = parse_vtxo(BOARD_VTXO_HEX).unwrap();
        let id_str = vtxo.id().to_string();
        assert_eq!(id_str.parse::<VtxoId>().unwrap(), vtxo.id());
    }

    #[test]
    fn parse_vtxo_rejects_garbage() {
        assert!(parse_vtxo("").is_err());
        assert!(parse_vtxo("not a vtxo at all!!").is_err());
        // Valid hex, but not a VTXO.
        assert!(parse_vtxo("deadbeef").is_err());
        // Valid base64, but not a VTXO.
        assert!(parse_vtxo("aGVsbG8gd29ybGQ=").is_err());
    }

    /// Same 12 words, four ways a caller might paste them.
    #[test]
    fn a_mnemonic_parses_whatever_whitespace_separates_the_words() {
        const WORDS: [&str; 12] = [
            "abandon", "abandon", "abandon", "abandon", "abandon", "abandon", "abandon", "abandon",
            "abandon", "abandon", "abandon", "about",
        ];
        let canonical = seed_from_str(bitcoin::Network::Regtest, &WORDS.join(" ")).unwrap();

        for sep in ["\n", "\t", "  ", " \n "] {
            let seed = seed_from_str(bitcoin::Network::Regtest, &WORDS.join(sep))
                .unwrap_or_else(|e| panic!("{:?}-separated phrase rejected: {}", sep, e.message()));
            assert_eq!(
                seed.fingerprint(),
                canonical.fingerprint(),
                "separator {:?}",
                sep
            );
        }
    }

    /// A hex seed has no whitespace, so it must still take the seed branch.
    #[test]
    fn a_hex_seed_is_still_read_as_a_seed() {
        let hex = "00".repeat(64);
        seed_from_str(bitcoin::Network::Regtest, &hex).unwrap();
        seed_from_str(bitcoin::Network::Regtest, &format!("  {}\n", hex)).unwrap();
    }

    /// The redaction in `config()` needs a live wallet to exercise, so this
    /// guards the other half: credentials must still reach bark on the way in.
    #[test]
    fn credentials_still_reach_bark_on_the_way_in() {
        let mut config = minimal_config();
        config.bitcoind_pass = Some("hunter2".to_owned());
        let cfg = config.into_bark(bitcoin::Network::Regtest).unwrap();
        assert_eq!(
            cfg.bitcoind_pass.map(|p| p.leak_ref().clone()).as_deref(),
            Some("hunter2")
        );
    }

    /// bark wraps the password in a Secret so it stays out of logs; the FFI
    /// Config derives Debug, so it must not be carrying one back.
    #[test]
    fn a_config_without_credentials_prints_none_of_them() {
        let mut config = minimal_config();
        config.bitcoind_pass = None;
        config.server_access_token = None;
        let rendered = format!("{:?}", config);
        assert!(rendered.contains("bitcoind_pass: None"), "{rendered}");
        assert!(rendered.contains("server_access_token: None"), "{rendered}");
    }

    #[test]
    fn parse_vtxo_ids_is_all_or_nothing() {
        let good = parse_vtxo(BOARD_VTXO_HEX).unwrap().id().to_string();

        assert_eq!(parse_vtxo_ids(&[]).unwrap().len(), 0);
        assert_eq!(
            parse_vtxo_ids(std::slice::from_ref(&good)).unwrap().len(),
            1
        );

        // One bad id fails the batch, so no partial lock/unlock can be issued.
        let err = parse_vtxo_ids(&[good, "nonsense".to_owned()]).unwrap_err();
        assert!(err.message().contains("nonsense"), "{}", err.message());
    }

    #[test]
    fn describe_state_names_the_holder() {
        use crate::types::{VtxoLockHolder, VtxoState};

        assert_eq!(describe_state(&VtxoState::Spendable), "spendable");
        assert_eq!(describe_state(&VtxoState::Spent), "spent");
        assert_eq!(describe_state(&VtxoState::Exited), "exited");
        assert_eq!(
            describe_state(&VtxoState::Locked { holder: None }),
            "locked by an unnamed holder",
        );
        assert_eq!(
            describe_state(&VtxoState::Locked {
                holder: Some(VtxoLockHolder::Action {
                    id: "ln-pay:xyz".into()
                }),
            }),
            "locked by action ln-pay:xyz",
        );
        assert_eq!(
            describe_state(&VtxoState::Locked {
                holder: Some(VtxoLockHolder::Movement { id: 9 }),
            }),
            "locked by movement 9",
        );
    }

    /// Same board VTXO, but in the previous encoding version (v1, no fee
    /// amount), from bark's `VTXO_NO_FEE_AMOUNT_VERSION_HEXES`. Old exports
    /// must stay importable; re-serializing upgrades to the current version.
    const BOARD_VTXO_V1_HEX: &str = "01001027000000000000928a01000365a81233741893bbe2461b8d479dadc5880594fe6f7479180d5843820af72b62e007ed4d23932a2625a78fe5c75bded751da3a99e23a297a527c01bd7bc8372128f200000000010102030a752219f1b94bbdf8994a0a980cdda08c2ad094cb29dd834878db6dee1612ee0365a81233741893bbe2461b8d479dadc5880594fe6f7479180d5843820af72b62655d61f465693e1fbf39814e9cb1d57d5eabc49548ed042626cc39c4d5fe5c1836c8c2fb634bceab363212ed4c6a8e78c9ff33884587830ffa2a1cbd84c95e77010000030a752219f1b94bbdf8994a0a980cdda08c2ad094cb29dd834878db6dee1612ee4c99b744ad009b7070f330794bf003fa8e5cd46ea1a6eb854aaf469385e3080000000000";

    #[test]
    fn parse_vtxo_accepts_legacy_encoding_version() {
        let vtxo = parse_vtxo(BOARD_VTXO_V1_HEX).unwrap();

        // Re-serialization uses the current version, so the hex differs but
        // must still parse to the same VTXO.
        let reserialized = vtxo.serialize_hex();
        assert_ne!(reserialized, BOARD_VTXO_V1_HEX);
        assert_eq!(parse_vtxo(&reserialized).unwrap().id(), vtxo.id());
    }

    #[test]
    fn parse_vtxo_rejects_truncated() {
        let truncated = &BOARD_VTXO_HEX[..BOARD_VTXO_HEX.len() - 8];
        assert!(parse_vtxo(truncated).is_err());
    }
}

fn parse_history_metadata_patch(patch_json: &str) -> Result<serde_json::Value, BarkError> {
    if patch_json.len() > 16 * 1024 {
        return Err("history metadata patch exceeds 16384 byte limit".into());
    }
    let patch: serde_json::Value =
        serde_json::from_str(patch_json).context("invalid history metadata patch JSON")?;
    if !patch.is_object() {
        return Err("history metadata patch must be a JSON object".into());
    }
    Ok(patch)
}

#[cfg(test)]
mod metadata_patch_tests {
    use super::*;
    #[test]
    fn requires_a_json_object_and_bounds_utf8_bytes() {
        for invalid in ["null", "[]", "1", "{bad"] {
            assert!(parse_history_metadata_patch(invalid).is_err());
        }
        assert!(
            parse_history_metadata_patch(&format!("{{\"note\":\"{}\"}}", "🦀".repeat(4096)))
                .is_err()
        );
        assert!(
            parse_history_metadata_patch(&format!("{{\"note\":\"{}\"}}", "a".repeat(16373)))
                .is_ok()
        );
        assert!(parse_history_metadata_patch("{\"note\":null,\"nested\":{\"x\":1}}").is_ok());
    }
}
