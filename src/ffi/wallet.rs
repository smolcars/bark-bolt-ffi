use std::future::Future;
use std::sync::Arc;

use anyhow::Context;

use crate::config::Config;
use crate::core::notification::NotificationHolder;
use crate::core::wallet::{OpenArgs as CoreOpenArgs, Wallet as CoreWallet};
use crate::ffi::db::open_sqlite_db;
use crate::ffi::onchain::OnchainWallet;
use crate::ffi::runtime::run_async;
use crate::types::Network;
use crate::{types, BarkError};

/// Optional arguments for [`Wallet::open`], mirroring [`bark::OpenWalletArgs`].
///
/// Every field has a default, so callers only set what they need.
#[boltffi::data]
pub struct WalletOpenArgs {
    /// Whether to run the background daemon
    ///
    /// When disabled, you must manually call `Wallet::sync` to sync the wallet.
    ///
    /// Default: true
    #[boltffi::default(true)]
    pub run_daemon: bool,

    /// The data directory to use for this wallet
    pub datadir: String,

    /// Whether to create a new wallet if no wallet exists
    ///
    ///  Default: true
    #[boltffi::default(true)]
    pub create_if_not_exists: bool,

    /// Whether to create a new wallet even if the Ark server cannot be reached
    ///
    /// Default: false
    #[boltffi::default(false)]
    pub create_without_server: bool,

    /// Whether to skip the seed-recovery mailbox scan
    ///
    /// The scan runs on the open that creates the wallet locally and makes
    /// network calls; its outcome is available from `Wallet::recovery_status`.
    /// Set this to open without it.
    ///
    /// Default: false
    #[boltffi::default(false)]
    pub skip_recovery: bool,
}

/// BoltFFI-facing Bark wallet.
///
/// Wraps `core::Wallet` and adds:
/// - Tokio runtime offload for asynchronous Bark work
/// - daemon shutdown on drop
/// - LNURL `pay_lightning_address`
/// - Daemon control methods
/// - Notification holder
/// - Callback onchain dispatch
pub struct Wallet {
    core: Arc<CoreWallet>,
    db_path: Arc<std::path::PathBuf>,
}

/// Release reservations if foreign cancellation drops an undelivered result.
struct PreparedBoardHandoff {
    board: Option<types::PreparedBoard>,
    core: Arc<CoreWallet>,
}

impl Drop for PreparedBoardHandoff {
    fn drop(&mut self) {
        if let Some(board) = self.board.take() {
            let core = self.core.clone();
            crate::ffi::runtime::TOKIO_RT.spawn(async move {
                let _ = core.discard_prepared_board(board.id).await;
            });
        }
    }
}

impl Wallet {
    fn run<F, Fut, T>(&self, action: F) -> impl Future<Output = T>
    where
        F: FnOnce(Arc<CoreWallet>) -> Fut,
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        // Bark sync futures are too large to nest by value on Swift worker stacks.
        let future = Box::pin(action(self.core.clone()));
        let db_path = self.db_path.clone();
        run_async(async move {
            let _db_path = db_path;
            future.await
        })
    }
}

/// Low-level function to initialize a wallet
///
/// You probably want to use [`Wallet::open`] instead.
#[boltffi::export]
pub async fn init_wallet(
    network: Network,
    mnemonic_or_seed: String,
    config: Config,
    datadir: String,
    allow_unreachable_server: bool,
) -> Result<(), BarkError> {
    run_async(async move {
        let db =
            open_sqlite_db(&datadir).with_context(|| format!("opening sqlite in {}", datadir))?;
        let lock_manager = bark::lock_manager::platform_default(Some(&datadir), None)?;
        CoreWallet::create(
            network,
            mnemonic_or_seed,
            config,
            &*db,
            &*lock_manager,
            allow_unreachable_server,
        )
        .await?;
        Ok(())
    })
    .await
}

#[boltffi::export]
impl Wallet {
    // ------------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------------

    /// Open a wallet (creating it first if `create_if_not_exists` is set),
    /// mirroring [`bark::Wallet::open`]: a single entry point with everything
    /// else optional (see [`WalletOpenArgs`]). For the explicit
    /// initialize-but-don't-open path, use the top-level `init_wallet`.
    ///
    /// `mnemonic_or_seed` accepts either a BIP-39 mnemonic phrase or a 64-byte
    /// hex-encoded seed.
    pub async fn open(
        network: Network,
        mnemonic_or_seed: String,
        config: Config,
        args: WalletOpenArgs,
    ) -> Result<Self, BarkError> {
        Self::open_inner(network, mnemonic_or_seed, config, args, None).await
    }

    /// Open with a BDK or foreign callback onchain wallet, retaining its shared backend.
    pub async fn open_with_onchain(
        network: Network,
        mnemonic_or_seed: String,
        config: Config,
        args: WalletOpenArgs,
        onchain: &OnchainWallet,
    ) -> Result<Self, BarkError> {
        Self::open_inner(
            network,
            mnemonic_or_seed,
            config,
            args,
            Some(onchain.inner_dyn()),
        )
        .await
    }

    // ------------------------------------------------------------------------
    // Sync & Maintenance
    // ------------------------------------------------------------------------

    pub async fn sync(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.sync().await }).await
    }

    /// Scan for VTXOs that were force-exited on-chain without the user asking
    /// and route them into the unilateral-exit flow so the funds can be claimed.
    ///
    /// This already runs automatically as part of [`Self::sync`]; call it
    /// directly to trigger the scan on demand.
    pub async fn sync_force_exited_vtxos(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.sync_force_exited_vtxos().await })
            .await
    }

    pub async fn maintenance(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.maintenance().await })
            .await
    }

    pub async fn maintenance_delegated(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.maintenance_delegated().await })
            .await
    }

    // ------------------------------------------------------------------------
    // Address
    // ------------------------------------------------------------------------

    /// Bark's m/350'/0'/index key, only if already persisted in this wallet.
    pub async fn peek_public_key(&self, index: u32) -> Result<String, BarkError> {
        self.run(move |core| async move {
            Ok(core
                .inner()
                .peek_keypair(index)
                .await?
                .public_key()
                .to_string())
        })
        .await
    }

    /// Derive and persist the next Bark wallet key. Never returns a private key.
    pub async fn derive_store_next_keypair(&self) -> Result<types::StoredPublicKey, BarkError> {
        self.run(|core| async move {
            let (key, index) = core.inner().derive_store_next_keypair().await?;
            Ok(types::StoredPublicKey {
                public_key: key.public_key().to_string(),
                index,
            })
        })
        .await
    }

    /// Sign with a persisted wallet key using Bitcoin signed-message hash and DER hex.
    pub async fn sign_message(&self, message: String, index: u32) -> Result<String, BarkError> {
        self.run(move |core| async move {
            Ok(crate::crypto::sign(
                &message,
                &core.inner().peek_keypair(index).await?,
            ))
        })
        .await
    }

    pub async fn new_address(&self) -> Result<String, BarkError> {
        self.run(|core| async move { core.new_address().await })
            .await
    }

    pub async fn new_address_with_index(&self) -> Result<types::AddressWithIndex, BarkError> {
        self.run(|core| async move { core.new_address_with_index().await })
            .await
    }

    pub async fn peek_address(&self, index: u32) -> Result<String, BarkError> {
        self.run(|core| async move { core.peek_address(index).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Balance & VTXOs
    // ------------------------------------------------------------------------

    pub async fn balance(&self) -> Result<types::Balance, BarkError> {
        self.run(|core| async move { core.balance().await }).await
    }

    pub async fn vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        self.run(|core| async move { core.vtxos().await }).await
    }

    pub async fn get_vtxo_by_id(&self, vtxo_id: String) -> Result<types::Vtxo, BarkError> {
        self.run(|core| async move { core.get_vtxo_by_id(vtxo_id).await })
            .await
    }

    pub async fn spendable_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        self.run(|core| async move { core.spendable_vtxos().await })
            .await
    }

    pub async fn all_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        self.run(|core| async move { core.all_vtxos().await }).await
    }

    pub async fn get_expiring_vtxos(
        &self,
        threshold_blocks: u32,
    ) -> Result<Vec<types::Vtxo>, BarkError> {
        self.run(|core| async move { core.get_expiring_vtxos(threshold_blocks).await })
            .await
    }

    pub async fn get_vtxos_to_refresh(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        self.run(|core| async move { core.get_vtxos_to_refresh().await })
            .await
    }

    // ------------------------------------------------------------------------
    // VTXO locking
    // ------------------------------------------------------------------------

    /// Reserve VTXOs so wallet-driven flows leave them alone: a locked VTXO is
    /// excluded from coin selection.
    ///
    /// `holder` is what `unlockVtxos` matches on to stop one subsystem
    /// releasing another's lock. Atomic, and re-locking with the same holder
    /// is a no-op.
    pub async fn lock_vtxos(
        &self,
        vtxo_ids: Vec<String>,
        holder: Option<types::VtxoLockHolder>,
    ) -> Result<(), BarkError> {
        self.run(|core| async move { core.lock_vtxos(vtxo_ids, holder).await })
            .await
    }

    /// Release VTXOs locked by `lockVtxos`.
    ///
    /// Every VTXO must currently be held by `expected_holder` or nothing is
    /// unlocked; that is what stops cleanup freeing a VTXO a payment or
    /// in-flight round has since claimed. Leaving it unset bypasses the guard
    /// entirely, so reserve that for recovery.
    pub async fn unlock_vtxos(
        &self,
        vtxo_ids: Vec<String>,
        expected_holder: Option<types::VtxoLockHolder>,
    ) -> Result<(), BarkError> {
        self.run(|core| async move { core.unlock_vtxos(vtxo_ids, expected_holder).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Offboarding
    // ------------------------------------------------------------------------

    pub async fn offboard_all(
        &self,
        bitcoin_address: String,
    ) -> Result<types::OffboardResult, BarkError> {
        self.run(|core| async move { core.offboard_all(bitcoin_address).await })
            .await
    }

    pub async fn offboard_vtxos(
        &self,
        vtxo_ids: Vec<String>,
        bitcoin_address: String,
    ) -> Result<types::OffboardResult, BarkError> {
        self.run(|core| async move { core.offboard_vtxos(vtxo_ids, bitcoin_address).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Lightning (send)
    // ------------------------------------------------------------------------

    /// Pay a BOLT11 invoice. `options` defaults to returning once the payment
    /// starts, with the server's own retry time.
    pub async fn pay_lightning_invoice(
        &self,
        invoice: String,
        amount_sats: Option<u64>,
        #[boltffi::default(None)] options: Option<types::LightningSendOptions>,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let options = options.unwrap_or_default();
        self.run(|core| async move {
            core.pay_lightning_invoice(invoice, amount_sats, options)
                .await
        })
        .await
    }

    /// Pay to a Lightning Address (`user@domain`), resolved via LNURL-pay.
    pub async fn pay_lightning_address(
        &self,
        lightning_address: String,
        amount_sats: u64,
        comment: Option<String>,
        #[boltffi::default(None)] options: Option<types::LightningSendOptions>,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let options = options.unwrap_or_default();
        self.run(|core| async move {
            core.pay_lightning_address(lightning_address, amount_sats, comment, options)
                .await
        })
        .await
    }

    /// Pay a raw LNURL-pay link (`lnurl1…`).
    ///
    /// Resolves the LNURL-pay endpoint to a BOLT11 invoice and pays it. Errors
    /// if the link decodes to a non-pay LNURL (auth, withdraw, channel).
    pub async fn pay_lnurl(
        &self,
        lnurl: String,
        amount_sats: u64,
        comment: Option<String>,
        #[boltffi::default(None)] options: Option<types::LightningSendOptions>,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let options = options.unwrap_or_default();
        self.run(|core| async move { core.pay_lnurl(lnurl, amount_sats, comment, options).await })
            .await
    }

    pub async fn pay_lightning_offer(
        &self,
        offer: String,
        amount_sats: Option<u64>,
        #[boltffi::default(None)] options: Option<types::LightningSendOptions>,
    ) -> Result<types::LightningSendStatus, BarkError> {
        let options = options.unwrap_or_default();
        self.run(|core| async move { core.pay_lightning_offer(offer, amount_sats, options).await })
            .await
    }

    pub async fn check_lightning_payment(
        &self,
        payment_hash: String,
        #[boltffi::default(false)] wait: bool,
    ) -> Result<types::LightningSendStatus, BarkError> {
        self.run(|core| async move { core.check_lightning_payment(payment_hash, wait).await })
            .await
    }

    /// Read-only triage, for polling after a send started with `wait = false`.
    /// Unlike `checkLightningPayment`, never advances the action.
    pub async fn lightning_send_state(
        &self,
        payment_hash: String,
    ) -> Result<types::LightningSendStatus, BarkError> {
        self.run(|core| async move { core.lightning_send_state(payment_hash).await })
            .await
    }

    /// Cheap "has this invoice ever been paid?", answered from the local fact
    /// table without consulting the server.
    pub async fn is_invoice_paid(&self, payment_hash: String) -> Result<bool, BarkError> {
        self.run(|core| async move { core.is_invoice_paid(payment_hash).await })
            .await
    }

    pub async fn pending_lightning_sends(&self) -> Result<Vec<types::LightningSend>, BarkError> {
        self.run(|core| async move { core.pending_lightning_sends().await })
            .await
    }

    /// Failed lightning sends whose HTLC revocation also failed.
    pub async fn stuck_failed_lightning_sends(
        &self,
    ) -> Result<Vec<types::LightningSend>, BarkError> {
        self.run(|core| async move { core.stuck_failed_lightning_sends().await })
            .await
    }

    /// Opt one stuck send into auto-exiting its HTLCs as they near expiry.
    pub async fn allow_lightning_send_to_exit(
        &self,
        payment_hash: String,
    ) -> Result<(), BarkError> {
        self.run(|core| async move { core.allow_lightning_send_to_exit(payment_hash).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Lightning (receive)
    // ------------------------------------------------------------------------

    pub async fn bolt11_invoice(
        &self,
        amount_sats: u64,
        description: Option<String>,
        #[boltffi::default(None)] token: Option<String>,
    ) -> Result<types::LightningInvoice, BarkError> {
        self.run(|core| async move { core.bolt11_invoice(amount_sats, description, token).await })
            .await
    }

    /// Create an invoice whose claimed VTXO is delivered to `claim_destination`
    /// (an Ark address), letting that wallet receive while offline.
    ///
    /// The claim is signed directly to that address's own policy, so this wallet
    /// has no custodial control and cannot redirect the payment. It can still
    /// strand it: delivering the signed output to the destination's mailbox is a
    /// separate step only this wallet can perform, and the recipient has no
    /// independent way to recover the funds until it happens. Delivery resumes
    /// automatically on restart, so a crash recovers on its own — but running
    /// this for someone else means they trust you to stay online and eventually
    /// deliver, not that they trust you with custody.
    ///
    /// A `claim_destination` owned by this wallet is claimed locally instead of
    /// going through its mailbox.
    pub async fn bolt11_invoice_for_address(
        &self,
        amount_sats: u64,
        claim_destination: String,
        #[boltffi::default(None)] description: Option<String>,
        #[boltffi::default(None)] token: Option<String>,
    ) -> Result<types::LightningInvoice, BarkError> {
        self.run(|core| async move {
            core.bolt11_invoice_for_address(amount_sats, claim_destination, description, token)
                .await
        })
        .await
    }

    pub async fn try_claim_all_lightning_receives(
        &self,
        #[boltffi::default(false)] wait: bool,
    ) -> Result<Vec<types::LightningReceive>, BarkError> {
        self.run(|core| async move { core.try_claim_all_lightning_receives(wait).await })
            .await
    }

    pub async fn pending_lightning_receives(
        &self,
    ) -> Result<Vec<types::LightningReceive>, BarkError> {
        self.run(|core| async move { core.pending_lightning_receives().await })
            .await
    }

    pub async fn claimable_lightning_receive_balance_sats(&self) -> Result<u64, BarkError> {
        self.run(|core| async move { core.claimable_lightning_receive_balance_sats().await })
            .await
    }

    /// Triage a payment hash: settled or in-progress. Errors if no lightning
    /// receive is known for this payment hash.
    pub async fn lightning_receive_state(
        &self,
        payment_hash: String,
    ) -> Result<types::LightningReceive, BarkError> {
        self.run(|core| async move { core.lightning_receive_state(payment_hash).await })
            .await
    }

    pub async fn try_claim_lightning_receive(
        &self,
        payment_hash: String,
        #[boltffi::default(false)] wait: bool,
    ) -> Result<types::LightningReceive, BarkError> {
        self.run(|core| async move { core.try_claim_lightning_receive(payment_hash, wait).await })
            .await
    }

    pub async fn cancel_lightning_receive(&self, payment_hash: String) -> Result<(), BarkError> {
        self.run(|core| async move { core.cancel_lightning_receive(payment_hash).await })
            .await
    }

    /// Force-exit an unfinished lightning receive.
    pub async fn attempt_lightning_receive_exit(
        &self,
        payment_hash: String,
    ) -> Result<(), BarkError> {
        self.run(|core| async move { core.attempt_lightning_receive_exit(payment_hash).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Arkoor
    // ------------------------------------------------------------------------

    /// Send an out-of-round payment to an Ark address. Returns the id of the
    /// `Movement` that records it.
    ///
    /// An address this bark cannot deliver to (see `validateArkoorAddress`) is
    /// rejected up front, leaving the VTXOs spendable rather than cosigned.
    pub async fn send_arkoor_payment(
        &self,
        ark_address: String,
        amount_sats: u64,
    ) -> Result<u32, BarkError> {
        self.run(|core| async move { core.send_arkoor_payment(ark_address, amount_sats).await })
            .await
    }

    /// Whether this wallet can pay `address` out-of-round: same network and
    /// server, a supported VTXO policy, and only delivery mechanisms this bark
    /// supports. An address listing no delivery mechanism is valid. Errors only
    /// when the address does not parse.
    pub async fn validate_arkoor_address(&self, address: String) -> Result<bool, BarkError> {
        self.run(|core| async move { core.validate_arkoor_address(address).await })
            .await
    }

    pub async fn send_onchain(
        &self,
        address: String,
        amount_sats: u64,
    ) -> Result<types::OffboardResult, BarkError> {
        self.run(|core| async move { core.send_onchain(address, amount_sats).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Payment requests
    // ------------------------------------------------------------------------

    /// Parse any payment string Bark understands: BIP 321 and `lightning:`
    /// URIs, BOLT11 invoices, BOLT12 offers, Lightning Addresses, LNURL-pay
    /// links, Ark addresses, bitcoin addresses and hex output scripts.
    ///
    /// Problems with individual options are returned in their `errors`. Fails
    /// with `BarkErrorCode.invalidPaymentRequest` when nothing parses.
    pub async fn parse_payment_request(
        &self,
        input: String,
    ) -> Result<types::PaymentRequest, BarkError> {
        self.run(|core| async move { core.parse_payment_request(input).await })
            .await
    }

    /// Estimate the fee of paying `amount_sats` with one method.
    pub async fn estimate_payment_fee(
        &self,
        method: types::PaymentMethod,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_payment_fee(method, amount_sats).await })
            .await
    }

    /// Estimate every option of `request`, cheapest first. `amount_sats`
    /// defaults to the request's amount; one of them is required.
    pub async fn estimate_payment_fees(
        &self,
        request: types::PaymentRequest,
        amount_sats: Option<u64>,
    ) -> Result<Vec<types::PaymentMethodFee>, BarkError> {
        self.run(|core| async move { core.estimate_payment_fees(request, amount_sats).await })
            .await
    }

    /// Pay with one method of a parsed payment request.
    ///
    /// Pass `amount_sats` only when the option's `requiresAmount` is set, and
    /// prefer the request's own `amountSats`. Bark accepts up to twice an
    /// invoice's amount when one is passed for an invoice that has an amount.
    /// `comment` is forwarded only when `supportsComment` is set, and
    /// `options` applies to Lightning methods only.
    pub async fn send_payment(
        &self,
        method: types::PaymentMethod,
        amount_sats: Option<u64>,
        #[boltffi::default(None)] comment: Option<String>,
        #[boltffi::default(None)] options: Option<types::LightningSendOptions>,
    ) -> Result<types::PaymentResult, BarkError> {
        let options = options.unwrap_or_default();
        self.run(|core| async move {
            core.send_payment(method, amount_sats, comment, options)
                .await
        })
        .await
    }

    /// Build a BIP 321 URI for receiving, generating a new Ark address, an
    /// onchain address and a BOLT11 invoice as `options` requests.
    pub async fn bip321_uri(
        &self,
        #[boltffi::default(None)] options: Option<types::Bip321UriOptions>,
    ) -> Result<types::Bip321Uri, BarkError> {
        let options = options.unwrap_or_default();
        self.run(|core| async move { core.bip321_uri(options).await })
            .await
    }

    /// Network, mailbox id and VTXO xpub, for support requests.
    pub async fn debug_info(&self) -> Result<types::DebugInfo, BarkError> {
        self.run(|core| async move { core.debug_info().await })
            .await
    }

    // ------------------------------------------------------------------------
    // History
    // ------------------------------------------------------------------------

    pub async fn history(&self) -> Result<Vec<types::Movement>, BarkError> {
        self.run(|core| async move { core.history().await }).await
    }

    /// Merge a JSON-object patch (<=16KiB) into movement metadata, preserving Nitro semantics.
    pub async fn update_history_metadata(
        &self,
        movement_id: u32,
        patch_json: String,
    ) -> Result<(), BarkError> {
        self.run(|core| async move { core.update_history_metadata(movement_id, patch_json).await })
            .await
    }

    /// Destructively forget one VTXO using Bark's recovery API. Can cause loss of funds.
    pub async fn dangerous_drop_vtxo(&self, id: String) -> Result<(), BarkError> {
        self.run(|core| async move {
            core.inner()
                .dangerous_drop_vtxo(id.parse().context("invalid VTXO id")?)
                .await?;
            Ok(())
        })
        .await
    }

    pub async fn history_by_payment_method(
        &self,
        payment_method_type: String,
        payment_method_value: String,
    ) -> Result<Vec<types::Movement>, BarkError> {
        self.run(|core| async move {
            core.history_by_payment_method(payment_method_type, payment_method_value)
                .await
        })
        .await
    }

    // ------------------------------------------------------------------------
    // Refresh
    // ------------------------------------------------------------------------

    pub async fn refresh_vtxos(&self, vtxo_ids: Vec<String>) -> Result<Option<String>, BarkError> {
        self.run(|core| async move { core.refresh_vtxos(vtxo_ids).await })
            .await
    }

    pub async fn maintenance_refresh(&self) -> Result<Option<String>, BarkError> {
        self.run(|core| async move { core.maintenance_refresh().await })
            .await
    }

    pub async fn refresh_vtxos_delegated(
        &self,
        vtxo_ids: Vec<String>,
    ) -> Result<Option<types::RoundState>, BarkError> {
        self.run(|core| async move { core.refresh_vtxos_delegated(vtxo_ids).await })
            .await
    }

    /// Schedule a delegated refresh for `scheduled_height` instead of the next
    /// round. The refresh fee is priced against the VTXO's remaining lifetime at
    /// that height, and the server charges less the closer a VTXO is to expiry,
    /// so scheduling further out never costs more than refreshing now.
    pub async fn refresh_vtxos_scheduled(
        &self,
        vtxo_ids: Vec<String>,
        scheduled_height: u32,
    ) -> Result<Option<types::RoundState>, BarkError> {
        self.run(|core| async move {
            core.refresh_vtxos_scheduled(vtxo_ids, scheduled_height)
                .await
        })
        .await
    }

    // ------------------------------------------------------------------------
    // Recovery
    // ------------------------------------------------------------------------

    /// Recover the given VTXO ids from the server, importing the ones this
    /// wallet owns that are still spendable. Use it to retry ids a previous
    /// scan reported as `failed`.
    ///
    /// `gap_limit` overrides `Config.vtxo_key_gap_limit` for the key scan that
    /// decides which of `vtxo_ids` this wallet owns. Widen it to reach ids a
    /// previous scan bucketed as `foreign`.
    pub async fn recover_vtxos(
        &self,
        vtxo_ids: Vec<String>,
        #[boltffi::default(None)] gap_limit: Option<u32>,
    ) -> Result<types::RecoveryReport, BarkError> {
        self.run(|core| async move { core.recover_vtxos(vtxo_ids, gap_limit).await })
            .await
    }

    /// Outcome of the seed-recovery scan that ran during `Wallet::open`.
    ///
    /// Recovery only runs on the open that creates the wallet locally, and not
    /// at all when `WalletOpenArgs.skip_recovery` is set, so this is `NotRun`
    /// on every subsequent open. `Failed` means the scan errored before
    /// producing a report — bark logs that and lets open succeed — so funds
    /// may be missing until a retry; `Completed` carries the report, and
    /// `isComplete == false` there means funds may still be missing. Retry the
    /// report's `failed` ids with `recoverVtxos`.
    pub fn recovery_status(&self) -> types::RecoveryStatus {
        self.core.recovery_status()
    }

    /// The report of the seed-recovery scan that ran during `Wallet::open`, or
    /// none if the scan did not complete. Use `recovery_status` to tell a scan
    /// that failed apart from one that never ran.
    pub fn recovery_report(&self) -> Option<types::RecoveryReport> {
        self.core.recovery_report()
    }

    // ------------------------------------------------------------------------
    // Info
    // ------------------------------------------------------------------------

    pub async fn properties(&self) -> Result<types::WalletProperties, BarkError> {
        self.run(|core| async move { core.properties().await })
            .await
    }

    pub fn fingerprint(&self) -> String {
        self.core.fingerprint()
    }

    pub async fn network(&self) -> Result<types::Network, BarkError> {
        self.run(|core| async move { core.network().await }).await
    }

    pub async fn config(&self) -> Config {
        self.core.config()
    }

    pub async fn ark_info(&self) -> Option<types::ArkInfo> {
        self.run(|core| async move { core.ark_info().await }).await
    }

    pub async fn next_round_start_time(&self) -> Result<u64, BarkError> {
        self.run(|core| async move { core.next_round_start_time().await })
            .await
    }

    // ------------------------------------------------------------------------
    // Boarding (requires onchain wallet)
    // ------------------------------------------------------------------------

    /// Build/sign without storing or broadcasting. IDs and locks are process-local.
    /// Coin selection never spends `excluded_outpoints`; entries that are malformed or
    /// not this wallet's coins are ignored rather than rejected.
    pub async fn prepare_board(
        &self,
        amount: types::OnchainSendAmount,
        fee_rate_sat_per_vb: u64,
        confirmed_only: bool,
        #[boltffi::default(None)] excluded_outpoints: Option<Vec<types::OutPoint>>,
    ) -> Result<types::PreparedBoard, BarkError> {
        let mut handoff = self
            .run(|core| async move {
                let board = core
                    .prepare_board(
                        amount,
                        fee_rate_sat_per_vb,
                        confirmed_only,
                        excluded_outpoints.unwrap_or_default(),
                    )
                    .await?;
                Ok::<_, BarkError>(PreparedBoardHandoff {
                    board: Some(board),
                    core,
                })
            })
            .await?;
        Ok(handoff.board.take().expect("prepared board handoff"))
    }

    /// Commit to Bark and explicitly broadcast the exact reviewed transaction.
    pub async fn submit_prepared_board(&self, id: String) -> Result<types::BoardResult, BarkError> {
        self.run(|core| async move { core.submit_prepared_board(id).await })
            .await
    }

    /// Releases an unsubmitted board; cannot undo a durable board or a broadcast.
    pub async fn discard_prepared_board(&self, id: String) -> Result<(), BarkError> {
        self.run(|core| async move { core.discard_prepared_board(id).await })
            .await
    }

    pub async fn transaction_status(
        &self,
        txid: String,
    ) -> Result<types::TransactionStatus, BarkError> {
        self.run(|core| async move { core.transaction_status(txid).await })
            .await
    }

    pub async fn list_all_exits(
        &self,
        include_history: bool,
        include_transactions: bool,
    ) -> Result<Vec<types::ExitTransactionStatus>, BarkError> {
        self.run(|core| async move {
            core.list_all_exits(include_history, include_transactions)
                .await
        })
        .await
    }

    pub async fn board_amount(&self, amount_sats: u64) -> Result<types::PendingBoard, BarkError> {
        self.run(|core| async move { core.board_amount(amount_sats).await })
            .await
    }

    pub async fn board_all(&self) -> Result<types::PendingBoard, BarkError> {
        self.run(|core| async move { core.board_all().await }).await
    }

    pub async fn board_funding_address(&self) -> Result<types::BoardFundingInfo, BarkError> {
        self.run(|core| async move { core.board_funding_address().await })
            .await
    }

    pub async fn board_psbt(
        &self,
        psbt_base64: String,
        keypair_index: u32,
        expiry_height: u32,
    ) -> Result<types::PendingBoard, BarkError> {
        self.run(|core| async move {
            core.board_psbt(psbt_base64, keypair_index, expiry_height)
                .await
        })
        .await
    }

    pub async fn sync_pending_boards(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.sync_pending_boards().await })
            .await
    }

    pub async fn pending_boards(&self) -> Result<Vec<types::PendingBoard>, BarkError> {
        self.run(|core| async move { core.pending_boards().await })
            .await
    }

    pub async fn pending_board_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        self.run(|core| async move { core.pending_board_vtxos().await })
            .await
    }

    pub async fn pending_round_input_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        self.run(|core| async move { core.pending_round_input_vtxos().await })
            .await
    }

    pub async fn pending_lightning_send_vtxos(&self) -> Result<Vec<types::Vtxo>, BarkError> {
        self.run(|core| async move { core.pending_lightning_send_vtxos().await })
            .await
    }

    // ------------------------------------------------------------------------
    // Unilateral Exits
    // ------------------------------------------------------------------------

    pub async fn start_exit_for_entire_wallet(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.start_exit_for_entire_wallet().await })
            .await
    }

    pub async fn sync_exits(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.sync_exits().await })
            .await
    }

    pub async fn progress_exits(
        &self,
        fee_rate_sat_per_vb: Option<u64>,
    ) -> Result<Vec<types::ExitProgressStatus>, BarkError> {
        self.run(|core| async move { core.progress_exits(fee_rate_sat_per_vb).await })
            .await
    }

    pub async fn start_exit_for_vtxos(&self, vtxo_ids: Vec<String>) -> Result<(), BarkError> {
        self.run(|core| async move { core.start_exit_for_vtxos(vtxo_ids).await })
            .await
    }

    /// Like `startExitForVtxos`, but skips dust and standardness checks.
    ///
    /// Only for VTXOs already onchain, or a node that accepts non-standard
    /// transactions; otherwise the exit transactions may be unrelayable.
    pub async fn start_exit_for_vtxos_including_non_standard(
        &self,
        vtxo_ids: Vec<String>,
    ) -> Result<(), BarkError> {
        self.run(|core| async move {
            core.start_exit_for_vtxos_including_non_standard(vtxo_ids)
                .await
        })
        .await
    }

    /// Cancel a unilateral exit still in its abortable window.
    ///
    /// The VTXO stays spendable either way, and a lock you took yourself is
    /// yours to release. Expected refusals come back in the result rather than
    /// as an error, and cancelling twice succeeds.
    pub async fn cancel_exit(&self, vtxo_id: String) -> Result<types::ExitCancelResult, BarkError> {
        self.run(|core| async move { core.cancel_exit(vtxo_id).await })
            .await
    }

    pub async fn list_claimable_exits(&self) -> Result<Vec<types::ExitVtxo>, BarkError> {
        self.run(|core| async move { core.list_claimable_exits().await })
            .await
    }

    pub async fn get_exit_vtxos(&self) -> Result<Vec<types::ExitVtxo>, BarkError> {
        self.run(|core| async move { core.get_exit_vtxos().await })
            .await
    }

    pub async fn has_pending_exits(&self) -> Result<bool, BarkError> {
        self.run(|core| async move { core.has_pending_exits().await })
            .await
    }

    pub async fn pending_exits_total_sats(&self) -> Result<u64, BarkError> {
        self.run(|core| async move { core.pending_exits_total_sats().await })
            .await
    }

    pub async fn all_exits_claimable_at_height(&self) -> Result<Option<u32>, BarkError> {
        self.run(|core| async move { core.all_exits_claimable_at_height().await })
            .await
    }

    pub async fn get_exit_status(
        &self,
        vtxo_id: String,
        include_history: bool,
        include_transactions: bool,
    ) -> Result<Option<types::ExitTransactionStatus>, BarkError> {
        self.run(|core| async move {
            core.get_exit_status(vtxo_id, include_history, include_transactions)
                .await
        })
        .await
    }

    /// Build a PSBT claiming exited VTXOs to `address`.
    ///
    /// Draining everything must be asked for with `drain_all`, so an id list
    /// filtered down to nothing is an error rather than a sweep. Ids are parsed
    /// all-or-nothing; well-formed ids that are not claimable are skipped.
    pub async fn drain_exits(
        &self,
        vtxo_ids: Vec<String>,
        #[boltffi::default(false)] drain_all: bool,
        address: String,
        fee_rate_sat_per_vb: Option<u64>,
    ) -> Result<types::ExitClaimTransaction, BarkError> {
        self.run(|core| async move {
            core.drain_exits(vtxo_ids, drain_all, address, fee_rate_sat_per_vb)
                .await
        })
        .await
    }

    pub async fn sign_exit_claim_inputs(&self, psbt_base64: String) -> Result<String, BarkError> {
        self.run(|core| async move { core.sign_exit_claim_inputs(psbt_base64).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Rounds
    // ------------------------------------------------------------------------

    pub async fn pending_round_states(&self) -> Result<Vec<types::RoundState>, BarkError> {
        self.run(|core| async move { core.pending_round_states().await })
            .await
    }

    /// Advance pending rounds and report terminal outcomes before they disappear from pending state.
    pub async fn sync_pending_rounds(&self) -> Result<Vec<types::PendingRoundStatus>, BarkError> {
        self.run(|core| async move { core.sync_pending_rounds().await })
            .await
    }

    pub async fn cancel_pending_round(&self, round_id: u32) -> Result<(), BarkError> {
        self.run(|core| async move { core.cancel_pending_round(round_id).await })
            .await
    }

    pub async fn cancel_all_pending_rounds(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.cancel_all_pending_rounds().await })
            .await
    }

    pub async fn progress_pending_rounds(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.progress_pending_rounds().await })
            .await
    }

    // ------------------------------------------------------------------------
    // Server
    // ------------------------------------------------------------------------

    pub async fn refresh_server(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.refresh_server().await })
            .await
    }

    // ------------------------------------------------------------------------
    // VTXO Expiry & Refresh Scheduling
    // ------------------------------------------------------------------------

    pub async fn get_first_expiring_vtxo_blockheight(&self) -> Result<Option<u32>, BarkError> {
        self.run(|core| async move { core.get_first_expiring_vtxo_blockheight().await })
            .await
    }

    pub async fn get_next_required_refresh_blockheight(&self) -> Result<Option<u32>, BarkError> {
        self.run(|core| async move { core.get_next_required_refresh_blockheight().await })
            .await
    }

    // ------------------------------------------------------------------------
    // Broadcasting
    // ------------------------------------------------------------------------

    pub async fn broadcast_tx(&self, tx_hex: String) -> Result<String, BarkError> {
        self.run(|core| async move { core.broadcast_tx(tx_hex).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Mailbox
    // ------------------------------------------------------------------------

    pub fn mailbox_identifier(&self) -> Result<String, BarkError> {
        self.core.mailbox_identifier()
    }

    /// Create a hex-encoded authorization that lets whoever holds it read this
    /// wallet's mailbox from the Ark server for `expiry_secs` from now, so 86400
    /// for 24 hours. An authorization cannot be revoked early, so keep the
    /// window short.
    pub fn mailbox_authorization(&self, expiry_secs: u32) -> Result<String, BarkError> {
        self.core.mailbox_authorization(expiry_secs)
    }

    /// Absolute Unix expiry in seconds, unlike mailboxAuthorization's relative duration.
    pub fn mailbox_authorization_at(
        &self,
        authorization_expiry: i64,
    ) -> Result<types::MailboxAuthorization, BarkError> {
        self.core.mailbox_authorization_at(authorization_expiry)
    }

    // ------------------------------------------------------------------------
    // VTXO Import / Export
    // ------------------------------------------------------------------------

    /// Import a VTXO from its serialized form (hex or base64).
    ///
    /// The VTXO is stored in the state the server reports for it, so one that
    /// was already spent is recorded as spent rather than refused. Pass `args`
    /// to widen the key-scan gap limit, skip the server status check, or allow
    /// partial success; omit it for the defaults.
    ///
    /// The first parameter keeps its historical `vtxo_base64` name for foreign
    /// binding compatibility (bindings expose parameter names), but hex as
    /// returned by [`Wallet::vtxo_encoded`] is accepted too.
    pub async fn import_vtxo(
        &self,
        vtxo_base64: String,
        #[boltffi::default(None)] args: Option<types::ImportVtxoArgs>,
    ) -> Result<(), BarkError> {
        self.run(|core| async move { core.import_vtxo(vtxo_base64, args).await })
            .await
    }

    /// Import several VTXOs (hex or base64) under a single key scan and a
    /// single write, which is why this is not just a loop over `import_vtxo`.
    ///
    /// Returns the ids now held — whether this call stored them or found them
    /// already present — so a failed batch can be retried. One VTXO that cannot
    /// be imported discards the whole batch unless `args.allow_partial` is set,
    /// in which case the ones that did import are kept.
    pub async fn import_vtxos(
        &self,
        encoded_vtxos: Vec<String>,
        #[boltffi::default(None)] args: Option<types::ImportVtxoArgs>,
    ) -> Result<Vec<String>, BarkError> {
        self.run(|core| async move { core.import_vtxos(encoded_vtxos, args).await })
            .await
    }

    /// Hex-encoded serialization of the full VTXO (genesis chain included),
    /// re-importable via [`Wallet::import_vtxo`]. Mirrors bark-rest
    /// `GET /vtxos/{id}/encoded`.
    pub async fn vtxo_encoded(&self, vtxo_id: String) -> Result<String, BarkError> {
        self.run(|core| async move { core.vtxo_encoded(vtxo_id).await })
            .await
    }

    // ------------------------------------------------------------------------
    // Fee Estimation
    // ------------------------------------------------------------------------

    pub async fn estimate_board_fee(
        &self,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_board_fee(amount_sats).await })
            .await
    }

    pub async fn estimate_offboard_fee(
        &self,
        address: String,
        vtxo_ids: Vec<String>,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_offboard_fee(address, vtxo_ids).await })
            .await
    }

    pub async fn estimate_refresh_fee(
        &self,
        vtxo_ids: Vec<String>,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_refresh_fee(vtxo_ids).await })
            .await
    }

    pub async fn estimate_lightning_send_fee(
        &self,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_lightning_send_fee(amount_sats).await })
            .await
    }

    pub async fn estimate_lightning_receive_fee(
        &self,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_lightning_receive_fee(amount_sats).await })
            .await
    }

    pub async fn estimate_arkoor_payment_fee(
        &self,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_arkoor_payment_fee(amount_sats).await })
            .await
    }

    pub async fn estimate_offboard_all_fee(
        &self,
        address: String,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_offboard_all_fee(address).await })
            .await
    }

    pub async fn estimate_send_onchain_fee(
        &self,
        address: String,
        amount_sats: u64,
    ) -> Result<types::FeeEstimate, BarkError> {
        self.run(|core| async move { core.estimate_send_onchain_fee(address, amount_sats).await })
            .await
    }

    /// Estimate the onchain cost of unilaterally (emergency) exiting VTXOs.
    /// Mirrors bark-rest `GET /exits/fee`.
    ///
    /// Pass an empty `vtxo_ids` to price exiting the whole wallet (every
    /// spendable VTXO). `fee_rate_sat_per_vb` applies to both legs; omit it to
    /// use the chain's fast rate for the broadcast leg and regular rate for
    /// the claim leg. `destination` only affects the claim transaction's weight.
    ///
    /// Pure weight arithmetic; `fee_margin` scales broadcast fees (default 1.2).
    /// The margin must be finite and non-negative.
    pub async fn estimate_emergency_exit_fee(
        &self,
        vtxo_ids: Vec<String>,
        fee_rate_sat_per_vb: Option<u64>,
        destination: Option<String>,
        #[boltffi::default(None)] fee_margin: Option<f64>,
    ) -> Result<types::EmergencyExitFeeEstimate, BarkError> {
        self.run(|core| async move {
            core.estimate_emergency_exit_fee(vtxo_ids, fee_rate_sat_per_vb, destination, fee_margin)
                .await
        })
        .await
    }

    // ------------------------------------------------------------------------
    // Notifications
    // ------------------------------------------------------------------------

    pub fn notifications(&self) -> NotificationHolder {
        NotificationHolder::new(self.core.inner())
    }

    /// Observe every committed database change, including keys, metadata and BDK state.
    pub fn state_changes(&self) -> Result<crate::StateChangeSubscription, BarkError> {
        Ok(crate::StateChangeSubscription::new(&self.db_path)?)
    }

    /// Export a consistent, plaintext SQLite snapshot for Noah backup v2.
    /// Refuses existing destinations and live database aliases.
    pub async fn create_snapshot(
        &self,
        destination_path: String,
    ) -> Result<crate::WalletSnapshotInfo, BarkError> {
        let source = self.db_path.clone();
        run_async(async move {
            tokio::task::spawn_blocking(move || {
                crate::backup::create_wallet_snapshot_blocking(
                    &source,
                    std::path::Path::new(&destination_path),
                )
            })
            .await
            .context("snapshot worker failed")?
            .map_err(BarkError::from)
        })
        .await
    }

    // ------------------------------------------------------------------------
    // Daemon
    // ------------------------------------------------------------------------

    /// Start the background daemon. The onchain wallet used by the daemon is
    /// the one supplied to `open_with_onchain` (`openWithOnchain` in Swift/Kotlin).
    pub async fn run_daemon(&self) -> Result<(), BarkError> {
        let inner = self.core.inner().clone();
        run_async(async move { inner.start_daemon().map_err(BarkError::from) }).await?;
        Ok(())
    }

    pub async fn stop_daemon(&self) -> Result<(), BarkError> {
        self.core.inner().stop_daemon();
        Ok(())
    }

    /// Stop the background daemon and wait until its tasks have finished, so
    /// nothing runs in the background afterwards (e.g. before deleting the
    /// wallet's datadir). No-op when no daemon is running.
    pub async fn stop_daemon_wait(&self) -> Result<(), BarkError> {
        self.run(|core| async move { core.stop_daemon_wait().await })
            .await
    }
}

impl Drop for Wallet {
    fn drop(&mut self) {
        self.core.inner().stop_daemon();
    }
}

impl Wallet {
    async fn open_inner(
        network: Network,
        mnemonic_or_seed: String,
        config: Config,
        args: WalletOpenArgs,
        onchain: Option<Arc<tokio::sync::RwLock<dyn bark::onchain::OnchainWalletTrait>>>,
    ) -> Result<Self, BarkError> {
        run_async(async move {
            let path = crate::ffi::db::sqlite_path(&args.datadir);
            if !args.create_if_not_exists {
                crate::ffi::db::validate_existing_wallet(
                    &args.datadir,
                    network,
                    &mnemonic_or_seed,
                )?;
            }
            let db = open_sqlite_db(&args.datadir)?;
            let db_path = crate::backup::register_live_database(&path)?;
            let core = CoreWallet::open(
                network,
                mnemonic_or_seed,
                config,
                CoreOpenArgs {
                    persister: Some(db),
                    datadir: Some(args.datadir),
                    lock_manager: None,
                    onchain,
                    run_daemon: args.run_daemon,
                    create_if_not_exists: args.create_if_not_exists,
                    create_without_server: args.create_without_server,
                    skip_recovery: args.skip_recovery,
                },
            )
            .await?;
            Ok(Self {
                core: Arc::new(core),
                db_path,
            })
        })
        .await
    }
}

#[cfg(test)]
mod noah_tests {
    use super::*;
    #[test]
    fn regtest_wallet_keys_snapshots_and_subscription() {
        if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
            return;
        }
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let fixture: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/nitro-v0.0.144-crypto.json")).unwrap();
            let config: Config = serde_json::from_value(serde_json::json!({"server_address":crate::ffi::regtest_url("ARK_PORT", "3635"), "bitcoind_address":crate::ffi::regtest_url("BITCOIN_PORT", "19443"),"bitcoind_user":"second","bitcoind_pass":"ark"})).unwrap();
            let wallet = Wallet::open(Network::Regtest, fixture["mnemonic"].as_str().unwrap().into(), config, WalletOpenArgs { run_daemon:false,datadir:directory.path().to_string_lossy().into(),create_if_not_exists:true,create_without_server:true,skip_recovery:true }).await.unwrap();
            assert!(wallet.peek_public_key(0).await.is_err());
            assert!(wallet.sign_message("test".into(),0).await.is_err());
            let subscription = wallet.state_changes().unwrap();
            assert_eq!(subscription.next_change().await.unwrap().unwrap().reason,"initial");
            for index in 0..=7 {
                let stored = wallet.derive_store_next_keypair().await.unwrap();
                assert_eq!(stored.index,index);
                if let Some(vector) = fixture["vectors"].as_array().unwrap().iter().find(|v| v["index"].as_u64() == Some(index as u64)) {
                    assert_eq!(stored.public_key,vector["walletPublicKey"].as_str().unwrap());
                    assert_eq!(wallet.peek_public_key(index).await.unwrap(),stored.public_key);
                    assert_eq!(wallet.sign_message(fixture["message"].as_str().unwrap().into(),index).await.unwrap(),vector["walletSignature"].as_str().unwrap());
                }
            }
            let changed = tokio::time::timeout(std::time::Duration::from_secs(3),subscription.next_change()).await.unwrap().unwrap().unwrap();
            assert_eq!(changed.reason,"databaseChanged");
            let snapshot = directory.path().join("snapshot.sqlite");
            let created = wallet.create_snapshot(snapshot.to_string_lossy().into()).await.unwrap();
            let validated = crate::validate_wallet_snapshot(created.path.clone(),None).await.unwrap();
            assert_eq!(created.sha256,validated.sha256);
            assert!(crate::validate_wallet_snapshot(directory.path().join("db.sqlite").to_string_lossy().into(),None).await.is_err());
            subscription.stop();
        });
    }
}

#[cfg(test)]
mod noah_p1_tests {
    use super::*;
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
    #[test]
    fn live_full_balance_buckets() {
        if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
            return;
        }
        use std::io::BufRead;
        struct Proxy(std::process::Child);
        impl Drop for Proxy {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut proxy = Proxy(
            std::process::Command::new("python3")
                .arg("tests/support/ark-proxy.py")
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut port = String::new();
        std::io::BufReader::new(proxy.0.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        async fn consistent(wallet: &Wallet) -> crate::types::Balance {
            let upstream = wallet.core.inner().balance().await.unwrap();
            let ffi = wallet.balance().await.unwrap();
            assert_eq!(ffi.total_sats, upstream.total().to_sat());
            assert_eq!(ffi.pending_sats, upstream.pending().to_sat());
            ffi
        }
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let config: Config = serde_json::from_value(serde_json::json!({"server_address":format!("http://127.0.0.1:{}",port.trim()),"bitcoind_address":crate::ffi::regtest_url("BITCOIN_PORT", "19443"),"bitcoind_user":"second","bitcoind_pass":"ark","vtxo_exit_margin":2})).unwrap();
            let mnemonic = crate::generate_mnemonic().unwrap();
            let args = |path: std::path::PathBuf| WalletOpenArgs { run_daemon:false,datadir:path.to_string_lossy().into(),create_if_not_exists:true,create_without_server:true,skip_recovery:true };
            let onchain = OnchainWallet::default(Network::Regtest,mnemonic.clone(),config.clone(),directory.path().to_string_lossy().into()).await.unwrap();
            let wallet = Arc::new(Wallet::open_with_onchain(Network::Regtest,mnemonic,config.clone(),args(directory.path().into()),&onchain).await.unwrap());
            let recipient = Wallet::open(Network::Regtest,crate::generate_mnemonic().unwrap(),config,args(directory.path().join("recipient"))).await.unwrap();
            let address = onchain.new_address().await.unwrap();
            regtest(&["send-to",&address,"0.005"]);
            onchain.sync().await.unwrap();
            let boarded = wallet.board_amount(100_000).await.unwrap();
            let board = consistent(&wallet).await;
            assert_eq!(board.pending_board_sats,boarded.amount_sats);
            assert_eq!(board.total_sats,boarded.amount_sats);
            regtest(&["generate","6"]); wallet.sync().await.unwrap();
            let funded = consistent(&wallet).await;
            assert_eq!(funded.spendable_sats,boarded.amount_sats);
            let destination = recipient.new_address().await.unwrap();
            let sender = wallet.clone();
            let send = tokio::spawn(async move { sender.send_arkoor_payment(destination,10_000).await });
            let pending = tokio::time::timeout(std::time::Duration::from_secs(10),async {
                loop { let b = wallet.balance().await.unwrap(); if b.pending_arkoor_send_sats>0 { break b; } tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
            }).await.unwrap();
            assert_eq!(pending.pending_arkoor_send_sats,funded.total_sats);
            assert_eq!(pending.pending_sats,funded.total_sats);
            assert_eq!(pending.spendable_sats,0);
            assert_eq!(pending.total_sats,funded.total_sats);
            send.await.unwrap().unwrap();
            recipient.sync().await.unwrap();
            let before = consistent(&wallet).await;
            let destination = onchain.new_address().await.unwrap();
            let sender = wallet.clone();
            let offboard = tokio::spawn(async move { sender.offboard_all(destination).await });
            let pending = tokio::time::timeout(std::time::Duration::from_secs(10),async {
                loop { let b = wallet.balance().await.unwrap(); if b.pending_offboard_sats>0 { break b; } tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
            }).await.unwrap();
            assert_eq!(pending.pending_offboard_sats,before.total_sats);
            assert_eq!(pending.pending_sats,before.total_sats);
            assert_eq!(pending.total_sats,before.total_sats);
            assert_eq!(pending.spendable_sats,0);
            offboard.await.unwrap().unwrap();
            let vtxo = recipient.spendable_vtxos().await.unwrap().remove(0);
            let tip: u32 = regtest(&["bcli","getblockcount"]).parse().unwrap();
            regtest(&["generate",&(vtxo.expiry_height-tip+1).to_string()]);
            recipient.sync().await.unwrap();
            let expired = consistent(&recipient).await;
            assert_eq!(expired.needs_refresh_sats,10_000);
            assert_eq!(expired.spendable_sats,0);
            assert_eq!(expired.pending_sats,0);
            assert_eq!(expired.total_sats,10_000);
            println!("full balance: real board, in-flight arkoor, in-flight offboard and expired VTXO passed");
        });
    }

    #[test]
    fn live_board_metadata_exit_packages_drop_and_bdk_drain() {
        if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
            return;
        }
        crate::ffi::runtime::TOKIO_RT.block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let config: Config = serde_json::from_value(serde_json::json!({"server_address":crate::ffi::regtest_url("ARK_PORT", "3635"),"bitcoind_address":crate::ffi::regtest_url("BITCOIN_PORT", "19443"),"bitcoind_user":"second","bitcoind_pass":"ark","vtxo_exit_margin":2})).unwrap();
            let mnemonic = crate::generate_mnemonic().unwrap();
            let onchain = OnchainWallet::default(Network::Regtest,mnemonic.clone(),config.clone(),directory.path().to_string_lossy().into()).await.unwrap();
            let wallet = Wallet::open_with_onchain(Network::Regtest,mnemonic,config.clone(),WalletOpenArgs { run_daemon:false,datadir:directory.path().to_string_lossy().into(),create_if_not_exists:true,create_without_server:true,skip_recovery:true },&onchain).await.unwrap();
            let address = onchain.new_address().await.unwrap();
            assert!(onchain.is_mine(address.clone()).await.unwrap());
            let script = address.parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>().unwrap().assume_checked().script_pubkey();
            let bitcoin_address = bitcoin::Address::from_script(&script, bitcoin::Network::Bitcoin).unwrap().to_string();
            assert!(onchain.is_mine(bitcoin_address).await.unwrap());
            assert!(!onchain.is_mine("1BoatSLRHtKNngkdXEeobR76b53LETtpyT".into()).await.unwrap());
            assert!(onchain.is_mine("invalid".into()).await.is_err());
            assert!(onchain.drain(address.clone(),Some(u64::MAX)).await.is_err());
            regtest(&["send-to",&address,"0.005"]);
            onchain.sync().await.unwrap();
            let board = wallet.board_amount(100_000).await.unwrap();
            assert!(!board.vtxo_ids.is_empty());
            regtest(&["generate","6"]);
            wallet.sync().await.unwrap();
            let movement = wallet.history().await.unwrap().into_iter().find(|m| m.subsystem_kind=="board").unwrap();
            wallet.update_history_metadata(movement.id,"{\"note\":\"Noah native\"}".into()).await.unwrap();
            let updated = wallet.history().await.unwrap().into_iter().find(|m| m.id==movement.id).unwrap();
            let metadata: serde_json::Value = serde_json::from_str(&updated.metadata_json).unwrap();
            assert_eq!(metadata["note"],"Noah native");
            assert!(wallet.update_history_metadata(movement.id,"[]".into()).await.is_err());
            let recipient_directory = directory.path().join("recipient");
            let recipient = Wallet::open(Network::Regtest,crate::generate_mnemonic().unwrap(),config.clone(), WalletOpenArgs { run_daemon:false,datadir:recipient_directory.to_string_lossy().into(),create_if_not_exists:true,create_without_server:true,skip_recovery:true }).await.unwrap();
            let receive_address = recipient.new_address().await.unwrap();
            wallet.send_arkoor_payment(receive_address.clone(),10_000).await.unwrap();
            recipient.sync().await.unwrap();
            let received: u64 = recipient.history().await.unwrap().iter().flat_map(|movement| &movement.received_on_addresses).filter(|destination| destination.payment_method.value == receive_address).map(|destination| destination.amount_sats).sum();
            assert_eq!(received,10_000);
            let ark_address = wallet.new_address_with_index().await.unwrap();
            let parsed = crate::parse_ark_address(ark_address.address).unwrap();
            assert_eq!(parsed.user_pubkey.as_deref(),Some(wallet.peek_public_key(ark_address.index).await.unwrap().as_str()));
            assert!(!parsed.ark_id.is_empty());
            let info = wallet.ark_info().await.unwrap();
            assert!(!info.mailbox_pubkey.is_empty());
            let auth = wallet.mailbox_authorization_at(2_000_000_000).unwrap();
            assert_eq!(auth.expiry,2_000_000_000);
            assert!(!auth.encoded.is_empty());
            assert!(wallet.mailbox_authorization_at(i64::MAX).is_err());
            assert!(wallet.sync_pending_rounds().await.unwrap().is_empty());
            let id = wallet.spendable_vtxos().await.unwrap()[0].id.clone();
            assert!(!wallet.get_vtxo_by_id(id.clone()).await.unwrap().server_pubkey.is_empty());
            wallet.start_exit_for_vtxos(vec![id.clone()]).await.unwrap();
            let status = wallet.get_exit_status(id.clone(),true,true).await.unwrap().unwrap();
            assert!(!status.transactions.is_empty());
            for package in status.transactions {
                let tx: bitcoin::Transaction = bitcoin::consensus::deserialize(&hex::decode(package.parent_tx_hex).unwrap()).unwrap();
                assert_eq!(tx.compute_txid().to_string(),package.parent_txid);
            }
            assert!(wallet.get_exit_status(id.clone(),false,false).await.unwrap().unwrap().transactions.is_empty());
            let snapshot = wallet.create_snapshot(directory.path().join("with-exit.sqlite").to_string_lossy().into()).await.unwrap();
            crate::validate_wallet_snapshot(snapshot.path,None).await.unwrap();
            wallet.cancel_exit(id.clone()).await.unwrap();
            wallet.dangerous_drop_vtxo(id.clone()).await.unwrap();
            assert!(wallet.get_vtxo_by_id(id).await.is_err());
            assert!(wallet.dangerous_drop_vtxo("invalid".into()).await.is_err());
            let destination = regtest(&["bcli","getnewaddress"]);
            onchain.sync().await.unwrap();
            let drain = onchain.drain(destination.clone(),None).await.unwrap();
            let transaction = onchain.transactions().await.unwrap().into_iter().find(|t| t.txid==drain).unwrap();
            let tx: bitcoin::Transaction = bitcoin::consensus::deserialize(&hex::decode(transaction.tx_hex).unwrap()).unwrap();
            assert_eq!(tx.output.len(),1);
            assert!(!onchain.is_mine(destination).await.unwrap());
            regtest(&["generate","1"]);
            onchain.sync().await.unwrap();
            assert_eq!(onchain.balance().await.unwrap().total_sats,0);
            regtest(&["send-to",&address,"0.002"]);
            onchain.sync().await.unwrap();
            let all = wallet.board_all().await.unwrap();
            assert!(!all.vtxo_ids.is_empty());
            regtest(&["generate","6"]);
            wallet.sync().await.unwrap();
            assert!(all.vtxo_ids.iter().all(|id| !id.is_empty()));
        });
    }
}
