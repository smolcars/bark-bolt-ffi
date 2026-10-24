use bark::persist::models::PendingBoard as BarkPendingBoard;
use bark::WalletVtxo as BarkWalletVtxo;
use bark::{Balance as BarkBalance, WalletProperties as BarkWalletProperties};
use bark_bitcoin_ext::AmountExt;
use bitcoin::Network as BtcNetwork;
use serde::{Deserialize, Serialize};

// ============================================================================
// Network
// ============================================================================

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub enum Network {
    Bitcoin,
    Testnet,
    Signet,
    Regtest,
}

impl From<Network> for BtcNetwork {
    fn from(n: Network) -> Self {
        match n {
            Network::Bitcoin => BtcNetwork::Bitcoin,
            Network::Testnet => BtcNetwork::Testnet,
            Network::Signet => BtcNetwork::Signet,
            Network::Regtest => BtcNetwork::Regtest,
        }
    }
}

impl From<BtcNetwork> for Network {
    fn from(n: BtcNetwork) -> Self {
        match n {
            BtcNetwork::Bitcoin => Network::Bitcoin,
            BtcNetwork::Testnet => Network::Testnet,
            BtcNetwork::Testnet4 => Network::Testnet, // Map Testnet4 to Testnet
            BtcNetwork::Signet => Network::Signet,
            BtcNetwork::Regtest => Network::Regtest,
        }
    }
}

// ============================================================================
// Logging
// ============================================================================

// Serde names are lowercase (`"error"` | `"warn"` | `"info"` | `"debug"` |
// `"trace"`), matching `as_str`. Kept out of the doc comment, which the generator
// copies into the native bindings, where the level is a native enum.
/// Severity of a log record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[boltffi::data]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// The level's serde/TS name, e.g. `"warn"`.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

impl From<log::Level> for LogLevel {
    fn from(l: log::Level) -> Self {
        match l {
            log::Level::Error => Self::Error,
            log::Level::Warn => Self::Warn,
            log::Level::Info => Self::Info,
            log::Level::Debug => Self::Debug,
            log::Level::Trace => Self::Trace,
        }
    }
}

impl From<LogLevel> for log::LevelFilter {
    fn from(l: LogLevel) -> Self {
        match l {
            LogLevel::Error => Self::Error,
            LogLevel::Warn => Self::Warn,
            LogLevel::Info => Self::Info,
            LogLevel::Debug => Self::Debug,
            LogLevel::Trace => Self::Trace,
        }
    }
}

// ============================================================================
// WalletProperties
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct WalletProperties {
    pub network: Network,
    pub fingerprint: String,
}

impl From<BarkWalletProperties> for WalletProperties {
    fn from(p: BarkWalletProperties) -> Self {
        Self {
            network: p.network.into(),
            fingerprint: p.fingerprint.to_string(),
        }
    }
}

// ============================================================================
// Balance
// ============================================================================

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct Balance {
    pub spendable_sats: u64,
    pub pending_in_round_sats: u64,
    pub pending_exit_sats: u64,
    pub pending_lightning_send_sats: u64,
    pub claimable_lightning_receive_sats: u64,
    pub pending_board_sats: u64,
    /// Expired or too-deep VTXOs; excluded from spendable, included in total.
    pub needs_refresh_sats: u64,
    /// Entire locked inputs, including change, until the send finalizes.
    pub pending_arkoor_send_sats: u64,
    /// Entire locked inputs until the offboard transaction is broadcast.
    pub pending_offboard_sats: u64,
    /// Bark's sum of the seven operation buckets, including claimable receives.
    pub pending_sats: u64,
    /// Bark's total: spendable, needs-refresh and pending funds.
    pub total_sats: u64,
}

impl From<BarkBalance> for Balance {
    fn from(b: BarkBalance) -> Self {
        Self {
            spendable_sats: b.spendable.to_sat(),
            pending_in_round_sats: b.pending_in_round.to_sat(),
            pending_exit_sats: b.pending_exit.to_sat(),
            pending_lightning_send_sats: b.pending_lightning_send.to_sat(),
            // Note: Bark's Balance only has claimable_lightning_receive field.
            // There is no separate "total pending receive" field in upstream.
            claimable_lightning_receive_sats: b.claimable_lightning_receive.to_sat(),
            pending_board_sats: b.pending_board.to_sat(),
            needs_refresh_sats: b.needs_refresh.to_sat(),
            pending_arkoor_send_sats: b.pending_arkoor_send.to_sat(),
            pending_offboard_sats: b.pending_offboard.to_sat(),
            pending_sats: b.pending().to_sat(),
            total_sats: b.total().to_sat(),
        }
    }
}

// ============================================================================
// Vtxo
// ============================================================================

/// Who holds the lock on a [`VtxoState::Locked`] VTXO.
///
/// Mirrors `bark::vtxo::VtxoLockHolder`. Action-based subsystems lock with
/// `Action`; pre-action subsystems (round, offboard, board, lightning
/// receive) lock with `Movement`. As upstream converts subsystems to
/// actions, new locks migrate from `Movement` to `Action` per-subsystem.
///
/// Serde tags match upstream's serialization: `"action"` / `"movement"`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[boltffi::data]
pub enum VtxoLockHolder {
    /// A wallet action checkpoint. `id` is upstream's `WalletActionId`
    /// (an opaque string).
    Action { id: String },
    /// A pre-action subsystem, keyed by its movement.
    Movement { id: u32 },
}

impl From<&bark::vtxo::VtxoLockHolder> for VtxoLockHolder {
    fn from(h: &bark::vtxo::VtxoLockHolder) -> Self {
        match h {
            bark::vtxo::VtxoLockHolder::Action { id } => Self::Action { id: id.clone() },
            bark::vtxo::VtxoLockHolder::Movement { id } => Self::Movement { id: id.0 },
        }
    }
}

impl From<VtxoLockHolder> for bark::vtxo::VtxoLockHolder {
    fn from(h: VtxoLockHolder) -> Self {
        match h {
            VtxoLockHolder::Action { id } => Self::Action { id },
            VtxoLockHolder::Movement { id } => Self::Movement {
                id: bark::movement::MovementId::new(id),
            },
        }
    }
}

/// Rich VTXO state, mirroring `bark::vtxo::VtxoState`.
///
/// Serde tags match upstream's kebab-case serialization:
/// `"spendable"` | `"locked"` | `"spent"` | `"exited"`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[boltffi::data]
pub enum VtxoState {
    /// Pure decode has no local wallet-state information.
    Unknown,
    /// Available; can be spent in a future round.
    Spendable,
    /// Locked by an operation. `holder` is `None` only for the narrow
    /// window between creating a fresh locked VTXO and pinning it to a
    /// specific operation, so a locked VTXO can legitimately carry no
    /// lock reason yet.
    Locked { holder: Option<VtxoLockHolder> },
    /// Consumed.
    Spent,
    /// In (or completed) a unilateral exit.
    Exited,
}

impl From<&bark::vtxo::VtxoState> for VtxoState {
    fn from(s: &bark::vtxo::VtxoState) -> Self {
        match s {
            bark::vtxo::VtxoState::Spendable => Self::Spendable,
            bark::vtxo::VtxoState::Locked { holder } => Self::Locked {
                holder: holder.as_ref().map(Into::into),
            },
            bark::vtxo::VtxoState::Spent => Self::Spent,
            bark::vtxo::VtxoState::Exited => Self::Exited,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct Vtxo {
    pub server_pubkey: String,
    pub exit_delta: u32,
    pub anchor_point: OutPoint,
    pub point: OutPoint,
    pub id: String,
    pub amount_sats: u64,
    pub expiry_height: u32,
    pub kind: String,
    pub state: VtxoState,
    /// Genesis chain length. Compare against `ArkInfo.max_vtxo_exit_depth` to
    /// detect VTXOs nearing the server's OOR-cosign refusal threshold.
    pub exit_depth: u32,
    /// Weight units of the unilateral exit transaction chain. Lets clients
    /// estimate exit cost without loading the full genesis.
    pub exit_tx_weight_wu: u64,
    /// Whether this VTXO's recovery state has been asserted with the server:
    /// its id posted to the recovery mailbox and its signed transaction chain
    /// registered. Only ever moves from `false` to `true`; the sync-time
    /// catch-up re-uploads the ones still `false`.
    pub registered: bool,
}

impl From<BarkWalletVtxo> for Vtxo {
    fn from(v: BarkWalletVtxo) -> Self {
        Self {
            server_pubkey: v.vtxo.server_pubkey().to_string(),
            exit_delta: u32::from(v.vtxo.exit_delta()),
            anchor_point: v.vtxo.chain_anchor().into(),
            point: v.vtxo.point().into(),
            id: v.vtxo.id().to_string(),
            amount_sats: v.vtxo.amount().to_sat(),
            expiry_height: v.vtxo.expiry_height().into(),
            kind: format!("{:?}", v.vtxo.policy_type()),
            state: (&v.state).into(),
            exit_depth: v.exit_depth as u32,
            exit_tx_weight_wu: v.exit_tx_weight.to_wu(),
            registered: v.registered,
        }
    }
}

// ============================================================================
// LightningInvoice
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct LightningInvoice {
    pub invoice: String,
    pub payment_hash: String,
    pub amount_sats: u64,
}

// ============================================================================
// OffboardResult
// ============================================================================

/// What an offboard or onchain send broadcast, and its history entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct OffboardResult {
    pub txid: String,
    /// Id of the `Movement` that records this offboard or send.
    pub movement_id: u32,
}

impl From<bark::OffboardResult> for OffboardResult {
    fn from(result: bark::OffboardResult) -> Self {
        Self {
            txid: result.txid.to_string(),
            movement_id: result.movement_id.0,
        }
    }
}

// ============================================================================
// AddressWithIndex
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct AddressWithIndex {
    pub address: String,
    pub index: u32,
}

// ============================================================================
// LightningReceive
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct LightningReceive {
    pub htlc_vtxo_ids: Vec<String>,
    pub movement_id: Option<u32>,
    pub payment_hash: String,
    pub invoice: String,
    /// `None` for an amountless invoice that has not settled yet. It used to
    /// report 0, which a UI cannot tell from a genuine zero.
    pub amount_sats: Option<u64>,
    /// Receive progress: "awaiting-payment" | "htlcs-ready" |
    /// "preimage-revealed" | "delivering" | "settled"
    pub state: String,
    /// Known while in-progress; present when settled.
    pub payment_preimage: Option<String>,
    /// Unix timestamp (seconds), set only when the receive is settled.
    pub settled_at: Option<i64>,
    /// Ark address the claimed VTXO is delivered to, for receives created with
    /// `bolt11_invoice_for_address`. `None` for ordinary receives claimed by
    /// this wallet, and always `None` once settled — the settled record does
    /// not carry the destination.
    pub claim_destination: Option<String>,
}

impl From<bark::actions::lightning::receive::LightningReceive> for LightningReceive {
    fn from(r: bark::actions::lightning::receive::LightningReceive) -> Self {
        use bark::actions::lightning::receive::Progress;
        let (htlc_vtxo_ids, movement_id) = match &r.progress {
            Progress::AwaitingPayment => (Vec::new(), None),
            Progress::HtlcsReady(htlcs) | Progress::PreimageRevealed(htlcs) => (
                htlcs.vtxo_ids.iter().map(ToString::to_string).collect(),
                Some(htlcs.movement_id.0),
            ),
            Progress::Delivering(delivery) => (Vec::new(), Some(delivery.movement_id.0)),
        };
        let state = match r.progress {
            Progress::AwaitingPayment => "awaiting-payment",
            Progress::HtlcsReady(_) => "htlcs-ready",
            Progress::PreimageRevealed(_) => "preimage-revealed",
            Progress::Delivering(_) => "delivering",
        };
        Self {
            htlc_vtxo_ids,
            movement_id,
            payment_hash: r.payment_hash.to_string(),
            invoice: r.invoice.to_string(),
            amount_sats: r
                .invoice
                .amount_milli_satoshis()
                .map(|a| bitcoin::Amount::from_msat_floor(a).to_sat()),
            state: state.to_string(),
            payment_preimage: Some(r.payment_preimage.to_string()),
            settled_at: None,
            claim_destination: r.claim_destination.map(|a| a.to_string()),
        }
    }
}

impl From<bark::persist::models::SettledLightningReceive> for LightningReceive {
    fn from(r: bark::persist::models::SettledLightningReceive) -> Self {
        Self {
            htlc_vtxo_ids: Vec::new(),
            movement_id: None,
            payment_hash: r.payment_hash.to_string(),
            invoice: r.invoice.to_string(),
            amount_sats: Some(r.amount.to_sat()),
            state: "settled".to_string(),
            payment_preimage: Some(r.preimage.to_string()),
            settled_at: Some(r.settled_at.timestamp()),
            // The settled persist model doesn't carry the claim destination.
            claim_destination: None,
        }
    }
}

impl From<bark::actions::lightning::receive::LightningReceiveState> for LightningReceive {
    fn from(s: bark::actions::lightning::receive::LightningReceiveState) -> Self {
        use bark::actions::lightning::receive::LightningReceiveState;
        match s {
            LightningReceiveState::InProgress(r) => r.into(),
            LightningReceiveState::Settled(r) => r.into(),
        }
    }
}

// ============================================================================
// LightningSend
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct LightningSend {
    pub payment_hash: String,
    pub htlc_vtxo_ids: Vec<String>,
    pub movement_id: Option<u32>,
    pub invoice: String,
    /// Amount being paid, in sats (`payment_amount`).
    pub amount_sats: u64,
    /// Routing/ark fee for the send, in sats.
    pub fee_sats: u64,
    /// Number of input VTXOs locked into the in-flight HTLC.
    pub htlc_vtxo_count: u32,
    /// Whether the send is stuck in the revocation-failed state: the payment
    /// failed but revoking the HTLC also failed
    pub has_failed_revocation: bool,
}

impl From<bark::actions::lightning::pay::LightningSend> for LightningSend {
    fn from(s: bark::actions::lightning::pay::LightningSend) -> Self {
        use bark::actions::lightning::pay::Progress;
        let (htlc_vtxo_ids, movement_id) = match &s.progress {
            Progress::Start => (Vec::new(), s.movement_id.map(|id| id.0)),
            Progress::InputsLocked { movement_id } => (Vec::new(), Some(movement_id.0)),
            Progress::HtlcReceived(htlcs)
            | Progress::PaymentInitiated(htlcs)
            | Progress::RevocableHtlcs { htlcs, .. }
            | Progress::RevocationStuck { htlcs, .. } => (
                htlcs.vtxo_ids.iter().map(ToString::to_string).collect(),
                Some(htlcs.movement_id.0),
            ),
        };
        Self {
            payment_hash: s.invoice.payment_hash().to_string(),
            htlc_vtxo_ids,
            movement_id,
            invoice: s.invoice.to_string(),
            amount_sats: s.payment_amount.to_sat(),
            fee_sats: s.fee.to_sat(),
            htlc_vtxo_count: s.input_vtxo_ids.len() as u32,
            has_failed_revocation: s.has_failed_revocation(),
        }
    }
}

// ============================================================================
// LightningSendStatus
// ============================================================================

/// Terminal/in-flight state of an outgoing lightning send.
///
/// Mirrors `bark`'s `LightningSendState`. `pay_lightning_*` and
/// `check_lightning_payment` now return this so callers can drive the
/// crash-safe send flow themselves (initiate with `wait = false`, then poll).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub enum LightningSendStatus {
    /// No record of this payment (never started, or already pruned).
    Unknown,
    /// Send is in flight; HTLC VTXOs are locked.
    InProgress { send: LightningSend },
    /// Send has settled; the preimage proves payment.
    Paid {
        payment_hash: String,
        preimage: String,
    },
}

impl From<bark::actions::lightning::pay::LightningSendState> for LightningSendStatus {
    fn from(state: bark::actions::lightning::pay::LightningSendState) -> Self {
        use bark::actions::lightning::pay::LightningSendState as State;
        match state {
            State::Unknown => Self::Unknown,
            State::InProgress(send) => Self::InProgress { send: send.into() },
            State::Paid(paid) => Self::Paid {
                payment_hash: paid.payment_hash.to_string(),
                preimage: paid.preimage.to_string(),
            },
        }
    }
}

// ============================================================================
// Movement
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct Movement {
    pub id: u32,
    pub status: String,
    pub subsystem_name: String,
    pub subsystem_kind: String,
    pub metadata_json: String,
    pub intended_balance_sats: i64,
    pub effective_balance_sats: i64,
    pub offchain_fee_sats: u64,
    pub sent_to_addresses: Vec<MovementDestination>,
    pub received_on_addresses: Vec<MovementDestination>,
    pub input_vtxo_ids: Vec<String>,
    pub output_vtxo_ids: Vec<String>,
    pub exited_vtxo_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,

    // calculated fields
    pub payment_hash: Option<String>,
    pub lightning_invoice: Option<String>,
    pub lightning_offer: Option<String>,
}

impl From<bark::movement::Movement> for Movement {
    fn from(m: bark::movement::Movement) -> Self {
        Self {
            id: m.id.0,
            status: m.status.to_string(),
            subsystem_name: m.subsystem.name.clone(),
            subsystem_kind: m.subsystem.kind.clone(),
            metadata_json: serde_json::to_string(&m.metadata).unwrap_or_else(|_| "{}".to_string()),
            intended_balance_sats: m.intended_balance.to_sat(),
            effective_balance_sats: m.effective_balance.to_sat(),
            offchain_fee_sats: m.offchain_fee.to_sat(),
            sent_to_addresses: m.sent_to.iter().map(Into::into).collect(),
            received_on_addresses: m.received_on.iter().map(Into::into).collect(),
            input_vtxo_ids: m.input_vtxos.iter().map(|v| v.to_string()).collect(),
            output_vtxo_ids: m.output_vtxos.iter().map(|v| v.to_string()).collect(),
            exited_vtxo_ids: m.exited_vtxos.iter().map(|v| v.to_string()).collect(),
            created_at: m.time.created_at.to_rfc3339(),
            updated_at: m.time.updated_at.to_rfc3339(),
            completed_at: m.time.completed_at.map(|t| t.to_rfc3339()),

            payment_hash: m.lightning_payment_hash().map(|h| h.to_string()),
            lightning_invoice: m.lightning_invoice().map(|i| i.to_string()),
            lightning_offer: m.lightning_offer().map(|o| o.to_string()),
        }
    }
}

// ============================================================================
// FeeEstimate
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct FeeEstimate {
    pub gross_amount_sats: u64,
    pub fee_sats: u64,
    pub net_amount_sats: u64,
    pub vtxos_spent: Vec<String>,
}

impl From<bark::FeeEstimate> for FeeEstimate {
    fn from(f: bark::FeeEstimate) -> Self {
        Self {
            gross_amount_sats: f.gross_amount.to_sat(),
            fee_sats: f.fee.to_sat(),
            net_amount_sats: f.net_amount.to_sat(),
            vtxos_spent: f.vtxos_spent.iter().map(|v| v.to_string()).collect(),
        }
    }
}

// ============================================================================
// EmergencyExitFeeEstimate
// ============================================================================

/// Estimated onchain cost of unilaterally (emergency) exiting a set of VTXOs.
///
/// Mirrors bark-rest's `GET /exits/fee` response. The two legs are paid from
/// different pockets: `exit_broadcast_fee_sats` is spent now out of confirmed
/// onchain funds to CPFP-bump every not-yet-confirmed exit transaction, while
/// `claim_fee_sats` is deducted later from the recovered value when the
/// matured outputs are drained.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct EmergencyExitFeeEstimate {
    /// CPFP fees to broadcast every not-yet-confirmed exit transaction. Paid
    /// now from confirmed onchain funds; this is the minimum onchain balance
    /// an emergency exit needs.
    pub exit_broadcast_fee_sats: u64,
    /// Fee of the single batched transaction that later drains the matured
    /// exit outputs. Subtracted from the exited amount.
    pub claim_fee_sats: u64,
    /// `exit_broadcast_fee_sats + claim_fee_sats`.
    pub total_fee_sats: u64,
    /// Fee rate the exit-broadcast leg was priced at (sat/vB, rounded up).
    /// Unless an explicit rate was supplied, the claim leg is priced
    /// separately at the chain's `regular` rate.
    pub fee_rate_sat_per_vb: u64,
    /// Number of exit transactions that still need to be broadcast and
    /// CPFP-bumped. Already-confirmed tree transactions are not counted.
    pub txs_to_broadcast: u64,
}

impl From<bark::exit::ExitFeeEstimate> for EmergencyExitFeeEstimate {
    fn from(e: bark::exit::ExitFeeEstimate) -> Self {
        Self {
            total_fee_sats: e.total().to_sat(),
            exit_broadcast_fee_sats: e.exit_broadcast_fee.to_sat(),
            claim_fee_sats: e.claim_fee.to_sat(),
            fee_rate_sat_per_vb: e.fee_rate.to_sat_per_vb_ceil(),
            txs_to_broadcast: e.txs_to_broadcast as u64,
        }
    }
}

// ============================================================================
// OnchainBalance
// ============================================================================

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct OnchainBalance {
    pub confirmed_sats: u64,
    pub pending_sats: u64,
    pub total_sats: u64,
}

impl From<bark::onchain::bdk_wallet::Balance> for OnchainBalance {
    fn from(b: bark::onchain::bdk_wallet::Balance) -> Self {
        Self {
            confirmed_sats: b.confirmed.to_sat(),
            pending_sats: b.untrusted_pending.to_sat() + b.trusted_pending.to_sat(),
            total_sats: b.total().to_sat(),
        }
    }
}

// ============================================================================
// Onchain wallet data (transactions, UTXOs, fee rates)
// ============================================================================

/// Network fee rates by urgency, mirroring `bark::chain::FeeRates`.
///
/// Rates are in sat/kwu (satoshis per 1000 weight units) — divide by 250 for
/// sat/vB.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct FeeRates {
    pub fast_sat_per_kwu: u64,
    pub regular_sat_per_kwu: u64,
    pub slow_sat_per_kwu: u64,
}

impl From<bark::chain::FeeRates> for FeeRates {
    fn from(f: bark::chain::FeeRates) -> Self {
        Self {
            fast_sat_per_kwu: f.fast.to_sat_per_kwu(),
            regular_sat_per_kwu: f.regular.to_sat_per_kwu(),
            slow_sat_per_kwu: f.slow.to_sat_per_kwu(),
        }
    }
}

/// Summary of one onchain wallet transaction, mirroring
/// `bark::onchain::WalletTxInfo`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct WalletTransaction {
    pub txid: String,
    /// The raw transaction, consensus-serialized as hex.
    pub tx_hex: String,
    /// Total fee paid by the transaction, when computable. `None` for
    /// inbound or collaboratively-funded txs whose foreign prevouts the
    /// wallet has not indexed.
    pub onchain_fee_sats: Option<u64>,
    /// Net change to the wallet's balance: received minus sent over
    /// wallet-owned outputs.
    pub balance_change_sats: i64,
    /// `Some` if confirmed in a block, `None` if still in the mempool.
    pub confirmation: Option<BlockRef>,
    /// `true` when this tx spends a P2A fee anchor — i.e. it is a CPFP
    /// child bumping the parent that created the anchor (exit fee txs).
    pub is_cpfp: bool,
}

impl From<&bark::onchain::WalletTxInfo> for WalletTransaction {
    fn from(info: &bark::onchain::WalletTxInfo) -> Self {
        Self {
            txid: info.txid.to_string(),
            tx_hex: bitcoin::consensus::encode::serialize_hex(info.tx.as_ref()),
            onchain_fee_sats: info.onchain_fees.map(|a| a.to_sat()),
            balance_change_sats: info.balance_change.to_sat(),
            confirmation: info.confirmation.as_ref().map(Into::into),
            is_cpfp: info.is_cpfp,
        }
    }
}

/// An onchain UTXO known to the wallet, mirroring `bark::onchain::Utxo`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[boltffi::data]
pub enum OnchainUtxo {
    /// A standard wallet UTXO.
    Local {
        outpoint: OutPoint,
        amount_sats: u64,
        /// `None` if unconfirmed.
        confirmation_height: Option<u32>,
    },
    /// A spendable unilateral-exit output claimed from a VTXO.
    Exit {
        vtxo_id: String,
        amount_sats: u64,
        /// Block height associated with the exit's validity window.
        height: u32,
    },
}

impl From<&bark::onchain::Utxo> for OnchainUtxo {
    fn from(u: &bark::onchain::Utxo) -> Self {
        match u {
            bark::onchain::Utxo::Local(l) => Self::Local {
                outpoint: OutPoint {
                    txid: l.outpoint.txid.to_string(),
                    vout: l.outpoint.vout,
                },
                amount_sats: l.amount.to_sat(),
                confirmation_height: l.confirmation_height.map(Into::into),
            },
            bark::onchain::Utxo::Exit(e) => Self::Exit {
                vtxo_id: e.vtxo.id().to_string(),
                amount_sats: e.vtxo.amount().to_sat(),
                height: e.height.into(),
            },
        }
    }
}

// ============================================================================
// PendingBoard
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct PendingBoard {
    pub vtxo_ids: Vec<String>,
    pub amount_sats: u64,
    pub txid: String,
}

impl From<BarkPendingBoard> for PendingBoard {
    fn from(pb: BarkPendingBoard) -> Self {
        Self {
            vtxo_ids: pb.vtxos.iter().map(ToString::to_string).collect(),
            amount_sats: pb.amount.to_sat(),
            txid: pb.funding_tx.compute_txid().to_string(),
        }
    }
}

// ============================================================================
// BoardFundingInfo
// ============================================================================

/// Destination for an externally funded board (e.g. a payjoin receive).
///
/// The funding script commits to the derived keypair and expiry height, so
/// `keypair_index` and `expiry_height` must be passed back unchanged to
/// `board_psbt` once the funding PSBT is available — including on retries
/// after the sender changes the transaction.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct BoardFundingInfo {
    pub address: String,
    pub expiry_height: u32,
    pub keypair_index: u32,
}

// ============================================================================
// FeeSchedule
// ============================================================================

/// One tier of a PPM-by-expiry fee table.
///
/// The entry applies when a VTXO expires in at most `expiry_blocks_threshold`
/// blocks and no other entry has a threshold between this one and the VTXO's
/// actual expiry distance. Tables are sorted ascending by threshold.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct PpmExpiryFeeEntry {
    pub expiry_blocks_threshold: u32,
    /// Parts-per-million fee rate applied for this expiry period.
    pub ppm: u64,
}

/// Fees for boarding onchain funds into the Ark.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct BoardFees {
    pub min_fee_sats: u64,
    pub base_fee_sats: u64,
    /// Parts-per-million fee rate on the boarded amount.
    pub ppm: u64,
}

/// Fees for offboarding VTXOs to an onchain address.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct OffboardFees {
    pub base_fee_sats: u64,
    /// Fixed number of virtual bytes charged on top of the output size.
    pub fixed_additional_vb: u64,
    pub ppm_expiry_table: Vec<PpmExpiryFeeEntry>,
}

/// Fees for refreshing VTXOs in a round.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct RefreshFees {
    pub base_fee_sats: u64,
    pub ppm_expiry_table: Vec<PpmExpiryFeeEntry>,
}

/// Fees for receiving over lightning.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct LightningReceiveFees {
    pub base_fee_sats: u64,
    /// Parts-per-million fee rate on the received amount.
    pub ppm: u64,
}

/// Fees for sending over lightning.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct LightningSendFees {
    pub min_fee_sats: u64,
    pub base_fee_sats: u64,
    pub ppm_expiry_table: Vec<PpmExpiryFeeEntry>,
}

/// The Ark server's complete fee schedule, mirroring `ark::fees::FeeSchedule`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct FeeSchedule {
    pub board: BoardFees,
    pub offboard: OffboardFees,
    pub refresh: RefreshFees,
    pub lightning_receive: LightningReceiveFees,
    pub lightning_send: LightningSendFees,
}

impl From<&ark::fees::FeeSchedule> for FeeSchedule {
    fn from(f: &ark::fees::FeeSchedule) -> Self {
        let table = |t: &[ark::fees::PpmExpiryFeeEntry]| {
            t.iter()
                .map(|e| PpmExpiryFeeEntry {
                    expiry_blocks_threshold: e.expiry_blocks_threshold,
                    ppm: e.ppm.0,
                })
                .collect()
        };
        Self {
            board: BoardFees {
                min_fee_sats: f.board.min_fee.to_sat(),
                base_fee_sats: f.board.base_fee.to_sat(),
                ppm: f.board.ppm.0,
            },
            offboard: OffboardFees {
                base_fee_sats: f.offboard.base_fee.to_sat(),
                fixed_additional_vb: f.offboard.fixed_additional_vb,
                ppm_expiry_table: table(&f.offboard.ppm_expiry_table),
            },
            refresh: RefreshFees {
                base_fee_sats: f.refresh.base_fee.to_sat(),
                ppm_expiry_table: table(&f.refresh.ppm_expiry_table),
            },
            lightning_receive: LightningReceiveFees {
                base_fee_sats: f.lightning_receive.base_fee.to_sat(),
                ppm: f.lightning_receive.ppm.0,
            },
            lightning_send: LightningSendFees {
                min_fee_sats: f.lightning_send.min_fee.to_sat(),
                base_fee_sats: f.lightning_send.base_fee.to_sat(),
                ppm_expiry_table: table(&f.lightning_send.ppm_expiry_table),
            },
        }
    }
}

// ============================================================================
// ArkInfo
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct ArkInfo {
    pub mailbox_pubkey: String,
    pub network: Network,
    pub server_pubkey: String,
    pub round_interval_secs: u64,
    pub nb_round_nonces: u32,
    pub vtxo_exit_delta: u32,
    /// The number of blocks a VTXO lives before it expires.
    pub vtxo_lifetime: u32,
    /// The number of blocks a VTXO lives before it expires.
    ///
    /// Deprecated: upstream renamed this to `vtxo_lifetime`. Both names are
    /// exposed and carry the same value; migrate to `vtxo_lifetime`, this one
    /// goes away once upstream drops it.
    pub vtxo_expiry_delta: u32,
    pub htlc_send_expiry_delta: u32,
    pub htlc_expiry_delta: u32,
    pub max_vtxo_amount_sats: Option<u64>,
    pub required_board_confirmations: u32,
    pub max_user_invoice_cltv_delta: u16,
    pub min_board_amount_sats: u64,
    pub ln_receive_anti_dos_required: bool,
    /// The server's fee schedule (board, offboard, refresh, lightning fees).
    pub fee_schedule: FeeSchedule,
    /// Maximum exit depth (genesis chain length) allowed for a VTXO before the
    /// server refuses to cosign further OOR transactions spending it.
    pub max_vtxo_exit_depth: u16,
}

impl From<&bark::ark::ArkInfo> for ArkInfo {
    fn from(info: &bark::ark::ArkInfo) -> Self {
        use bitcoin::hex::DisplayHex;

        Self {
            network: match info.network {
                BtcNetwork::Bitcoin => Network::Bitcoin,
                BtcNetwork::Testnet => Network::Testnet,
                BtcNetwork::Signet => Network::Signet,
                BtcNetwork::Regtest => Network::Regtest,
                _ => Network::Testnet,
            },
            server_pubkey: info.server_pubkey.serialize().as_hex().to_string(),
            mailbox_pubkey: info.mailbox_pubkey.to_string(),
            round_interval_secs: info.round_interval.as_secs(),
            nb_round_nonces: info.nb_round_nonces as u32,
            vtxo_exit_delta: u32::from(info.vtxo_exit_delta),
            vtxo_lifetime: u32::from(info.vtxo_lifetime),
            vtxo_expiry_delta: u32::from(info.vtxo_lifetime),
            htlc_send_expiry_delta: u32::from(info.htlc_send_expiry_delta),
            htlc_expiry_delta: u32::from(info.htlc_expiry_delta),
            max_vtxo_amount_sats: info.max_vtxo_amount.map(|a| a.to_sat()),
            required_board_confirmations: info.required_board_confirmations as u32,
            max_user_invoice_cltv_delta: info.max_user_invoice_cltv_delta.into(),
            min_board_amount_sats: info.min_board_amount.to_sat(),
            ln_receive_anti_dos_required: info.ln_receive_anti_dos_required,
            fee_schedule: (&info.fees).into(),
            max_vtxo_exit_depth: info.max_vtxo_exit_depth,
        }
    }
}

// ============================================================================
// Exit Types
// ============================================================================

use bark::exit::ExitProgressStatus as BarkExitProgressStatus;
use bark::exit::ExitVtxo as BarkExitVtxo;

/// Where an exit transaction was first seen, mirroring `bark::exit::ExitTxOrigin`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[boltffi::data]
pub enum ExitTxOrigin {
    /// Broadcast by this wallet.
    Wallet { confirmed_in: Option<BlockRef> },
    /// Seen in the mempool.
    Mempool,
    /// Seen confirmed in a block.
    Block { confirmed_in: BlockRef },
}

impl From<&bark::exit::ExitTxOrigin> for ExitTxOrigin {
    fn from(o: &bark::exit::ExitTxOrigin) -> Self {
        use bark::exit::ExitTxOrigin as B;
        match o {
            B::Wallet { confirmed_in } => Self::Wallet {
                confirmed_in: confirmed_in.as_ref().map(Into::into),
            },
            B::Mempool => Self::Mempool,
            B::Block { confirmed_in } => Self::Block {
                confirmed_in: confirmed_in.into(),
            },
        }
    }
}

/// Broadcast/confirmation status of one transaction in an exit chain,
/// mirroring `bark::exit::ExitTxStatus`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[boltffi::data]
pub enum ExitTxStatus {
    /// Inputs are still being verified.
    VerifyInputs,
    /// Waiting for the given input txids to confirm. Sorted for a stable
    /// order (upstream keeps them in a set).
    AwaitingInputConfirmation { txids: Vec<String> },
    /// Ready for its CPFP child to be broadcast.
    AwaitingCpfpBroadcast,
    /// CPFP child broadcast; waiting for confirmation.
    AwaitingConfirmation {
        child_txid: String,
        origin: ExitTxOrigin,
    },
    /// Confirmed in a block.
    Confirmed {
        child_txid: String,
        block: BlockRef,
        origin: ExitTxOrigin,
    },
}

impl From<&bark::exit::ExitTxStatus> for ExitTxStatus {
    fn from(s: &bark::exit::ExitTxStatus) -> Self {
        use bark::exit::ExitTxStatus as B;
        match s {
            B::VerifyInputs => Self::VerifyInputs,
            B::AwaitingInputConfirmation { txids } => {
                let mut txids: Vec<String> = txids.iter().map(|t| t.to_string()).collect();
                txids.sort_unstable();
                Self::AwaitingInputConfirmation { txids }
            }
            B::AwaitingCpfpBroadcast => Self::AwaitingCpfpBroadcast,
            B::AwaitingConfirmation { child_txid, origin } => Self::AwaitingConfirmation {
                child_txid: child_txid.to_string(),
                origin: origin.into(),
            },
            B::Confirmed {
                child_txid,
                block,
                origin,
            } => Self::Confirmed {
                child_txid: child_txid.to_string(),
                block: block.into(),
                origin: origin.into(),
            },
        }
    }
}

/// One transaction in an exit's unilateral broadcast chain.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[boltffi::data]
pub struct ExitTx {
    pub txid: String,
    pub status: ExitTxStatus,
}

impl From<&bark::exit::ExitTx> for ExitTx {
    fn from(t: &bark::exit::ExitTx) -> Self {
        Self {
            txid: t.txid.to_string(),
            status: (&t.status).into(),
        }
    }
}

/// State of a unilateral exit, mirroring `bark::exit::ExitState`.
///
/// Serde tags match upstream's kebab-case serialization (`"start"`,
/// `"processing"`, `"awaiting-delta"`, `"claimable"`, `"claim-in-progress"`,
/// `"claimed"`, `"vtxo-already-spent"`, `"vtxo-swept"`, `"canceled"`), with camelCase fields.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[boltffi::data]
pub enum ExitState {
    /// The exit was requested at the given tip.
    Start { tip_height: u32 },
    /// The exit transaction chain is being broadcast and confirmed.
    Processing {
        tip_height: u32,
        transactions: Vec<ExitTx>,
    },
    /// Fully confirmed; waiting out the exit delta until claimable.
    AwaitingDelta {
        tip_height: u32,
        confirmed_block: BlockRef,
        claimable_height: u32,
    },
    /// The exit output can be claimed.
    Claimable {
        tip_height: u32,
        claimable_since: BlockRef,
        last_scanned_block: Option<BlockRef>,
    },
    /// A claim transaction has been broadcast.
    ClaimInProgress {
        tip_height: u32,
        claimable_since: BlockRef,
        claim_txid: String,
    },
    /// Terminal: the exit output was claimed (or spent deeper in the tree).
    Claimed {
        tip_height: u32,
        txid: String,
        block: BlockRef,
    },
    /// Terminal: the VTXO was already spent offchain, so the exit cannot
    /// proceed.
    VtxoAlreadySpent { tip_height: u32 },
    /// Terminal: confirmed spends took inputs needed by the exit.
    VtxoSwept {
        tip_height: u32,
        spent_inputs: Vec<OutPoint>,
    },
    /// Resumable: the user canceled the exit before its final transaction
    /// was broadcast. The VTXO stays spendable.
    Canceled { tip_height: u32 },
}

impl From<&bark::exit::ExitState> for ExitState {
    fn from(s: &bark::exit::ExitState) -> Self {
        use bark::exit::ExitState as B;
        match s {
            B::Start(v) => Self::Start {
                tip_height: v.tip_height.into(),
            },
            B::Processing(v) => Self::Processing {
                tip_height: v.tip_height.into(),
                transactions: v.transactions.iter().map(Into::into).collect(),
            },
            B::AwaitingDelta(v) => Self::AwaitingDelta {
                tip_height: v.tip_height.into(),
                confirmed_block: (&v.confirmed_block).into(),
                claimable_height: v.claimable_height.into(),
            },
            B::Claimable(v) => Self::Claimable {
                tip_height: v.tip_height.into(),
                claimable_since: (&v.claimable_since).into(),
                last_scanned_block: v.last_scanned_block.as_ref().map(Into::into),
            },
            B::ClaimInProgress(v) => Self::ClaimInProgress {
                tip_height: v.tip_height.into(),
                claimable_since: (&v.claimable_since).into(),
                claim_txid: v.claim_txid.to_string(),
            },
            B::Claimed(v) => Self::Claimed {
                tip_height: v.tip_height.into(),
                txid: v.txid.to_string(),
                block: (&v.block).into(),
            },
            B::VtxoAlreadySpent(v) => Self::VtxoAlreadySpent {
                tip_height: v.tip_height.into(),
            },
            B::VtxoSwept(v) => Self::VtxoSwept {
                tip_height: v.tip_height.into(),
                spent_inputs: v
                    .spent_inputs
                    .iter()
                    .map(|p| OutPoint {
                        txid: p.txid.to_string(),
                        vout: p.vout,
                    })
                    .collect(),
            },
            B::Canceled(v) => Self::Canceled {
                tip_height: v.tip_height.into(),
            },
        }
    }
}

/// Lightweight discriminator for [`ExitState`], mirroring
/// `bark::exit::ExitStateKind`.
///
/// Carries no payload — use it where only the variant matters (e.g. reporting
/// which state blocked a cancellation). Serde tags match upstream's
/// kebab-case serialization.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[boltffi::data]
pub enum ExitStateKind {
    Start,
    Processing,
    AwaitingDelta,
    Claimable,
    ClaimInProgress,
    Claimed,
    VtxoAlreadySpent,
    VtxoSwept,
    Canceled,
}

impl From<bark::exit::ExitStateKind> for ExitStateKind {
    fn from(k: bark::exit::ExitStateKind) -> Self {
        use bark::exit::ExitStateKind as B;
        match k {
            B::Start => Self::Start,
            B::Processing => Self::Processing,
            B::AwaitingDelta => Self::AwaitingDelta,
            B::Claimable => Self::Claimable,
            B::ClaimInProgress => Self::ClaimInProgress,
            B::Claimed => Self::Claimed,
            B::VtxoAlreadySpent => Self::VtxoAlreadySpent,
            B::VtxoSwept => Self::VtxoSwept,
            B::Canceled => Self::Canceled,
        }
    }
}

/// Why a [`Wallet::cancel_exit`](crate::core::Wallet::cancel_exit) did not
/// cancel the exit.
///
/// These are the *expected* negative outcomes of asking to cancel, not faults:
/// each is a normal state the exit can legitimately be in. Genuine failures
/// (database, chain source unreachable) still surface as `Err`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[boltffi::data]
pub enum ExitCancelFailure {
    /// No exit was ever started for this VTXO.
    NotExiting,
    /// The exit has progressed past its abortable window. `state` is the
    /// state that blocked the cancellation.
    TooLate { state: ExitStateKind },
    /// The final exit transaction is already in the mempool or a block, so
    /// the exit can no longer be called off.
    AlreadyBroadcast { txid: String },
}

/// Outcome of a cancellation request.
///
/// `canceled` is `true` when the exit is now canceled — including when it was
/// already canceled by an earlier call, since cancellation is idempotent.
/// When `false`, `reason` says why.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct ExitCancelResult {
    pub canceled: bool,
    /// Present exactly when `canceled` is `false`.
    pub reason: Option<ExitCancelFailure>,
}

impl ExitCancelResult {
    /// The exit is canceled.
    pub(crate) fn canceled() -> Self {
        Self {
            canceled: true,
            reason: None,
        }
    }

    /// The exit was left as-is, for the given reason.
    pub(crate) fn refused(reason: ExitCancelFailure) -> Self {
        Self {
            canceled: false,
            reason: Some(reason),
        }
    }
}

/// A VTXO that is being unilaterally exited
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct ExitVtxo {
    pub vtxo_id: String,
    pub amount_sats: u64,
    pub state: ExitState,
    pub is_claimable: bool,
}

impl From<&BarkExitVtxo> for ExitVtxo {
    fn from(ev: &BarkExitVtxo) -> Self {
        Self {
            vtxo_id: ev.id().to_string(),
            amount_sats: ev.amount().to_sat(),
            state: ev.state().into(),
            is_claimable: ev.is_claimable(),
        }
    }
}

/// Status of an exit progression
#[derive(Clone, Debug)]
#[boltffi::data]
pub struct ExitProgressStatus {
    pub vtxo_id: String,
    pub state: ExitState,
    /// Typed like errors thrown by exit calls; `code` tells permanent from transient.
    pub error: Option<crate::error::BarkError>,
}

impl From<BarkExitProgressStatus> for ExitProgressStatus {
    fn from(eps: BarkExitProgressStatus) -> Self {
        Self {
            vtxo_id: eps.vtxo_id.to_string(),
            state: (&eps.state).into(),
            error: eps.error.as_ref().map(crate::error::exit_error),
        }
    }
}

/// Claim transaction for exited funds
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct ExitClaimTransaction {
    pub psbt_base64: String,
    pub fee_sats: u64,
}

/// Detailed status of an exit transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct ExitTransactionStatus {
    pub vtxo_id: String,
    pub amount_sats: u64,
    pub state: ExitState,
    pub history: Option<Vec<ExitState>>,
    pub transaction_count: u32,
    pub transactions: Vec<ExitTransactionPackage>,
}

impl ExitTransactionStatus {
    pub(crate) fn from_bark(ets: bark::exit::ExitTransactionStatus, amount_sats: u64) -> Self {
        let transactions = ets
            .transactions
            .iter()
            .map(|package| {
                let tracked_status = match &ets.state {
                    bark::exit::ExitState::Processing(processing) => processing
                        .transactions
                        .iter()
                        .find(|tx| tx.txid == package.exit.txid)
                        .map(|tx| (&tx.status).into()),
                    _ => None,
                };
                ExitTransactionPackage {
                    parent_txid: package.exit.txid.to_string(),
                    parent_tx_hex: bitcoin::consensus::encode::serialize_hex(&package.exit.tx),
                    child_txid: package
                        .child
                        .as_ref()
                        .map(|child| child.info.txid.to_string()),
                    child_tx_hex: package
                        .child
                        .as_ref()
                        .map(|child| bitcoin::consensus::encode::serialize_hex(&child.info.tx)),
                    child_origin: package.child.as_ref().map(|child| (&child.origin).into()),
                    status: tracked_status,
                }
            })
            .collect();
        Self {
            vtxo_id: ets.vtxo_id.to_string(),
            amount_sats,
            state: (&ets.state).into(),
            history: ets.history.map(|h| h.iter().map(Into::into).collect()),
            transaction_count: ets.transactions.len() as u32,
            transactions,
        }
    }
}

// ============================================================================
// Round Types
// ============================================================================

use bark::persist::models::{StoredRoundState, Unlocked};
use bark::round::RoundFlowKind as BarkRoundFlowKind;

/// Lifecycle phase of a round participation, mirroring
/// `bark::round::RoundFlowKind`.
///
/// Serde tags match upstream's `Display` form: `"delegated-pending"` |
/// `"pending"` | `"ongoing"` | `"awaiting-confirmations"` | `"failed"` |
/// `"canceled"`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[boltffi::data]
pub enum RoundFlowKind {
    /// Delegated participation waiting for its scheduled round. See
    /// [`RoundState::scheduled_height`].
    DelegatedPending,
    /// Interactive participation waiting for its round.
    Pending,
    /// The interactive part is being played out with the server.
    Ongoing,
    /// The round finished; waiting for its funding tx to confirm.
    AwaitingConfirmations,
    /// The participation failed.
    Failed,
    /// The user canceled the participation.
    Canceled,
}

impl From<BarkRoundFlowKind> for RoundFlowKind {
    fn from(k: BarkRoundFlowKind) -> Self {
        match k {
            BarkRoundFlowKind::DelegatedPending => Self::DelegatedPending,
            BarkRoundFlowKind::Pending => Self::Pending,
            BarkRoundFlowKind::Ongoing => Self::Ongoing,
            BarkRoundFlowKind::AwaitingConfirmations => Self::AwaitingConfirmations,
            BarkRoundFlowKind::Failed => Self::Failed,
            BarkRoundFlowKind::Canceled => Self::Canceled,
        }
    }
}

/// A pending round state
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct RoundState {
    pub id: u32,
    /// Whether the interactive part of the round is ongoing. Equivalent to
    /// `state` being `Pending` or `Ongoing`; kept for compatibility.
    pub ongoing: bool,
    /// Lifecycle phase of the participation.
    pub state: RoundFlowKind,
    /// Block height a delegated participation waits for, if it asked the
    /// server to schedule one. Only set while `state` is `DelegatedPending`.
    pub scheduled_height: Option<u32>,
}

impl From<StoredRoundState<Unlocked>> for RoundState {
    fn from(rs: StoredRoundState<Unlocked>) -> Self {
        Self {
            id: rs.id().0,
            ongoing: rs.state().ongoing_participation(),
            state: rs.state().flow_kind().into(),
            scheduled_height: rs.state().scheduled_height().map(Into::into),
        }
    }
}

// ============================================================================
// Recovery Types
// ============================================================================

/// One bucket of a [`RecoveryReport`].
///
/// `total_sats` only sums the VTXOs whose amount is known, so it can
/// under-count `failed`, where a VTXO may have failed before being fetched.
/// `vtxo_ids` is sorted, since upstream buckets them unordered.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[boltffi::data]
pub struct RecoveryBucket {
    pub vtxo_ids: Vec<String>,
    pub total_sats: u64,
}

/// Arguments for `Wallet::import_vtxo` / `Wallet::import_vtxos`, mirroring
/// `bark::ImportVtxoArgs`.
///
/// Every field has a default, so callers only set what they need.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[boltffi::data]
pub struct ImportVtxoArgs {
    /// Gap limit for the key scan that decides whether we own the VTXOs,
    /// overriding `Config.vtxo_key_gap_limit` for this call.
    ///
    /// Default: none (use the wallet's configured limit)

    #[boltffi::default(None)]
    #[serde(default)]
    pub gap_limit: Option<u32>,

    /// Import as spendable without asking the server for each VTXO's state.
    /// Skipping the check is faster but can leave the wallet holding spent
    /// VTXOs marked spendable, which then fail when selected as inputs.
    ///
    /// Default: false

    #[boltffi::default(false)]
    #[serde(default)]
    pub skip_status_check: bool,

    /// Keep the VTXOs that import successfully even when another one in the
    /// batch fails; the returned ids are the ones that were kept.
    ///
    /// Default: false, so a single failure discards the whole batch.

    #[boltffi::default(false)]
    #[serde(default)]
    pub allow_partial: bool,
}

impl From<ImportVtxoArgs> for bark::ImportVtxoArgs {
    fn from(a: ImportVtxoArgs) -> Self {
        bark::ImportVtxoArgs {
            gap_limit: a.gap_limit,
            skip_status_check: a.skip_status_check,
            allow_partial: a.allow_partial,
        }
    }
}

/// Outcome of a recovery scan: every VTXO id the scan looked at, bucketed by
/// what was decided about it.
///
/// `skipped` vs `failed` is the load-bearing distinction: a `skipped` VTXO was
/// decided not to be spendable (spent, exited, or reported non-spendable),
/// while a `failed` one could not be decided because of an error, so its funds
/// may still be missing. `failed` is retryable via `recover_vtxos`; a `foreign`
/// id instead sits beyond the key-derivation gap limit and needs a wider scan.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[boltffi::data]
pub struct RecoveryReport {
    /// Spendable VTXOs that were successfully re-imported.
    pub recovered: RecoveryBucket,
    /// Deliberately left out: spent into a newer recovered VTXO, exited
    /// on-chain, or reported non-spendable by the server.
    pub skipped: RecoveryBucket,
    /// No matching key could be derived within the gap limit (the configured
    /// run of consecutive unused key indices, 250 by default). For a mailbox scan these are most likely this wallet's
    /// own VTXOs, keyed beyond the limit, so funds may be missing; retrying
    /// won't help, only a wider gap limit (see `Config.vtxo_key_gap_limit` and
    /// the `gap_limit` override on `recover_vtxos`). For `recover_vtxos` it
    /// just means the caller passed an id this wallet doesn't own.
    pub foreign: RecoveryBucket,
    /// Could not be decided due to an error. Not known to be spent, so funds
    /// may be missing. Retryable.
    pub failed: RecoveryBucket,
    /// Already fully exited on-chain.
    pub exited: RecoveryBucket,
    /// Whether the scan accounted for every VTXO: no `failed`, no `foreign`.
    pub is_complete: bool,
}

impl From<&bark::RecoveryReportEntry> for RecoveryBucket {
    fn from(e: &bark::RecoveryReportEntry) -> Self {
        // Upstream keeps entries in a HashMap, so sort for a stable order.
        let mut vtxo_ids = e.ids().map(|id| id.to_string()).collect::<Vec<_>>();
        vtxo_ids.sort_unstable();
        Self {
            vtxo_ids,
            total_sats: e.total_amount().to_sat(),
        }
    }
}

impl From<&bark::RecoveryReport> for RecoveryReport {
    fn from(r: &bark::RecoveryReport) -> Self {
        Self {
            recovered: r.recovered().into(),
            skipped: r.skipped().into(),
            foreign: r.foreign().into(),
            failed: r.failed().into(),
            exited: r.exited().into(),
            is_complete: r.is_complete(),
        }
    }
}

/// Outcome of the seed-recovery scan `Wallet::open` runs, mirroring
/// `bark::RecoveryStatus`.
///
/// The three variants are distinguishable on purpose: `NotRun` means nothing
/// was attempted (wallet already existed locally, or `skip_recovery` was set),
/// `Failed` means the scan errored before producing a report so funds may be
/// missing until a retry succeeds, and `Completed` carries the report — whose
/// `is_complete` can still be `false` for individual VTXOs.
///
/// Serde tags: `"not-run"` | `"failed"` | `"completed"`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[boltffi::data]
pub enum RecoveryStatus {
    /// No scan was attempted.
    NotRun,
    /// The scan errored before it could produce a report.
    Failed { message: String },
    /// The scan ran to the end.
    Completed { report: RecoveryReport },
}

impl RecoveryStatus {
    /// The report, if the scan completed.
    pub fn report(&self) -> Option<&RecoveryReport> {
        match self {
            Self::Completed { report } => Some(report),
            _ => None,
        }
    }
}

impl From<bark::RecoveryStatus> for RecoveryStatus {
    fn from(s: bark::RecoveryStatus) -> Self {
        match s {
            bark::RecoveryStatus::NotRun => Self::NotRun,
            // Alternate form joins the anyhow cause chain on one line.
            bark::RecoveryStatus::Failed(e) => Self::Failed {
                message: format!("{:#}", e),
            },
            bark::RecoveryStatus::Completed(r) => Self::Completed {
                report: (&r).into(),
            },
        }
    }
}

// ============================================================================
// Callback Wallet Types
// ============================================================================

/// A Bitcoin transaction output destination
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct Destination {
    pub address: String,
    pub amount_sats: u64,
}

/// A Bitcoin transaction outpoint (reference to a previous output)
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[boltffi::data]
pub struct OutPoint {
    pub txid: String,
    pub vout: u32,
}

/// A fixed recipient amount, or all eligible wallet funds minus the exact fee.
#[derive(Clone, Debug)]
#[boltffi::data]
pub enum OnchainSendAmount {
    Amount { amount_sats: u64 },
    DrainAll,
}

/// A signed, process-local transaction. It has not been recorded or broadcast.
#[derive(Clone, Debug)]
#[boltffi::data]
pub struct PreparedTransaction {
    pub id: String,
    pub txid: String,
    pub amount_sats: u64,
    pub fee_sats: u64,
    pub vsize: u64,
    pub inputs: Vec<OutPoint>,
}

/// Signed boarding proposal; no key, movement or transaction has been stored.
#[derive(Clone, Debug)]
#[boltffi::data]
pub struct PreparedBoard {
    pub id: String,
    pub txid: String,
    pub gross_sats: u64,
    pub net_sats: u64,
    pub ark_fee_sats: u64,
    pub onchain_fee_sats: u64,
    pub vsize: u64,
    pub inputs: Vec<OutPoint>,
}

/// Once accepted by Bark, the board remains pending across discard/reopen.
#[derive(Clone, Debug)]
#[boltffi::data]
pub struct BoardResult {
    pub txid: String,
    pub broadcast: BroadcastOutcome,
    pub vtxo_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[boltffi::data]
pub enum TransactionStatus {
    NotFound,
    InMempool,
    Confirmed { height: u32 },
}

impl From<bark_bitcoin_ext::TxStatus> for TransactionStatus {
    fn from(status: bark_bitcoin_ext::TxStatus) -> Self {
        match status {
            bark_bitcoin_ext::TxStatus::NotFound => Self::NotFound,
            bark_bitcoin_ext::TxStatus::Mempool => Self::InMempool,
            bark_bitcoin_ext::TxStatus::Confirmed(block) => Self::Confirmed {
                height: block.height.into(),
            },
        }
    }
}

#[derive(Clone, Debug)]
#[boltffi::data]
pub enum BroadcastOutcome {
    Accepted,
    Failed { reason: String },
    NotAttempted { reason: String },
}

#[derive(Clone, Debug)]
#[boltffi::data]
pub struct BroadcastResult {
    pub txid: String,
    pub broadcast: BroadcastOutcome,
}

impl From<bitcoin::OutPoint> for OutPoint {
    fn from(point: bitcoin::OutPoint) -> Self {
        Self {
            txid: point.txid.to_string(),
            vout: point.vout,
        }
    }
}

/// Reference to a block in the blockchain
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[boltffi::data]
pub struct BlockRef {
    pub height: u32,
    pub hash: String,
}

impl From<&bark_bitcoin_ext::BlockRef> for BlockRef {
    fn from(br: &bark_bitcoin_ext::BlockRef) -> Self {
        Self {
            height: br.height.into(),
            hash: br.hash.to_string(),
        }
    }
}

/// Parameters for creating a CPFP (Child Pays For Parent) transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct CpfpParams {
    pub tx_hex: String,
    pub fees_type: String,
    pub effective_fee_rate_sat_per_vb: u64,
    pub current_package_fee_sats: Option<u64>,
}

// ============================================================================
// WalletNotification
// ============================================================================

/// A notification event from the wallet
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub enum WalletNotification {
    /// A new movement was created
    MovementCreated { movement: Movement },
    /// An existing movement was updated
    MovementUpdated { movement: Movement },
    /// The notification channel is lagging (notifications were dropped)
    ChannelLagging,
}

impl From<bark::WalletNotification> for WalletNotification {
    fn from(n: bark::WalletNotification) -> Self {
        match n {
            bark::WalletNotification::MovementCreated { movement } => {
                WalletNotification::MovementCreated {
                    movement: movement.into(),
                }
            }
            bark::WalletNotification::MovementUpdated { movement } => {
                WalletNotification::MovementUpdated {
                    movement: movement.into(),
                }
            }
            bark::WalletNotification::ChannelLagging => WalletNotification::ChannelLagging,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_vtxo_args_default_to_upstream_defaults() {
        let args: bark::ImportVtxoArgs = ImportVtxoArgs::default().into();
        assert_eq!(args.gap_limit, None);
        assert!(!args.skip_status_check);
        assert!(!args.allow_partial);
    }

    #[test]
    fn import_vtxo_args_are_forwarded() {
        let args: bark::ImportVtxoArgs = ImportVtxoArgs {
            gap_limit: Some(1_000),
            skip_status_check: true,
            allow_partial: true,
        }
        .into();
        assert_eq!(args.gap_limit, Some(1_000));
        assert!(args.skip_status_check);
        assert!(args.allow_partial);
    }

    #[test]
    fn lightning_send_options_are_forwarded() {
        let default: bark::LightningSendOptions = LightningSendOptions::default().into();
        assert!(!default.wait);
        assert_eq!(default.retry_for, None);
        let single: bark::LightningSendOptions = LightningSendOptions {
            wait: true,
            retry_for_secs: Some(0),
        }
        .into();
        assert!(single.wait);
        assert_eq!(single.retry_for, Some(std::time::Duration::ZERO));
    }

    #[test]
    fn payment_methods_round_trip() {
        let address = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
        let method = PaymentMethod {
            method_type: "bitcoin".into(),
            value: address.into(),
        };
        let upstream = bark::movement::PaymentMethod::try_from(method).unwrap();
        assert!(upstream.is_bitcoin() && upstream.requires_amount());
        let back = PaymentMethod::from(&upstream);
        assert_eq!(
            (back.method_type.as_str(), back.value.as_str()),
            ("bitcoin", address)
        );
        let unknown = PaymentMethod {
            method_type: "carrier-pigeon".into(),
            value: address.into(),
        };
        assert!(bark::movement::PaymentMethod::try_from(unknown).is_err());
    }
    use std::str::FromStr;

    fn txid(byte: u8) -> bitcoin::Txid {
        bitcoin::Txid::from_str(&hex::encode([byte; 32])).unwrap()
    }

    fn block_ref(height: u32, byte: u8) -> bark_bitcoin_ext::BlockRef {
        bark_bitcoin_ext::BlockRef {
            height: height.into(),
            hash: bitcoin::BlockHash::from_str(&hex::encode([byte; 32])).unwrap(),
        }
    }

    // ------------------------------------------------------------------
    // VtxoState (item: locked-VTXO lock reasons)
    // ------------------------------------------------------------------

    #[test]
    fn vtxo_state_conversion_carries_lock_holder() {
        use bark::movement::MovementId;
        use bark::vtxo::{VtxoLockHolder as B, VtxoState as BS};

        let cases: Vec<(BS, VtxoState)> = vec![
            (BS::Spendable, VtxoState::Spendable),
            (BS::Spent, VtxoState::Spent),
            (BS::Exited, VtxoState::Exited),
            (
                BS::Locked { holder: None },
                VtxoState::Locked { holder: None },
            ),
            (
                BS::Locked {
                    holder: Some(B::Movement { id: MovementId(7) }),
                },
                VtxoState::Locked {
                    holder: Some(VtxoLockHolder::Movement { id: 7 }),
                },
            ),
            (
                BS::Locked {
                    holder: Some(B::Action {
                        id: "ln-pay:abc".into(),
                    }),
                },
                VtxoState::Locked {
                    holder: Some(VtxoLockHolder::Action {
                        id: "ln-pay:abc".into(),
                    }),
                },
            ),
        ];
        for (upstream, expected) in cases {
            assert_eq!(VtxoState::from(&upstream), expected);
        }
    }

    #[test]
    fn vtxo_lock_holder_roundtrips_through_upstream() {
        // The lock guard in `unlock_vtxos` compares a holder the caller passed
        // in against one read back from the database, so the two directions
        // must agree exactly or a caller could never release its own lock.
        let cases = vec![
            VtxoLockHolder::Action {
                id: "exit:abc123".into(),
            },
            VtxoLockHolder::Movement { id: 42 },
        ];
        for holder in cases {
            let upstream = bark::vtxo::VtxoLockHolder::from(holder.clone());
            assert_eq!(VtxoLockHolder::from(&upstream), holder);
        }
    }

    #[test]
    fn exit_state_kind_conversion_covers_all_variants() {
        for kind in bark::exit::ExitStateKind::ALL {
            // Exercises every arm; a new upstream variant fails to compile
            // rather than silently mapping to something wrong.
            let _: ExitStateKind = (*kind).into();
        }
    }

    #[test]
    fn exit_cancel_result_serde_shape() {
        let ok = serde_json::to_value(ExitCancelResult::canceled()).unwrap();
        assert_eq!(ok["canceled"], serde_json::json!(true));
        assert!(ok["reason"].is_null());

        let refused = ExitCancelResult::refused(ExitCancelFailure::TooLate {
            state: ExitStateKind::Claimed,
        });
        let v = serde_json::to_value(&refused).unwrap();
        assert_eq!(v["canceled"], serde_json::json!(false));
        assert_eq!(v["reason"]["type"], serde_json::json!("too-late"));
        assert_eq!(v["reason"]["state"], serde_json::json!("claimed"));

        let broadcast = ExitCancelResult::refused(ExitCancelFailure::AlreadyBroadcast {
            txid: "ff00".into(),
        });
        let v = serde_json::to_value(&broadcast).unwrap();
        assert_eq!(v["reason"]["type"], serde_json::json!("already-broadcast"));
        assert_eq!(v["reason"]["txid"], serde_json::json!("ff00"));
    }

    #[test]
    fn vtxo_state_serde_tags_match_upstream() {
        let locked = VtxoState::Locked {
            holder: Some(VtxoLockHolder::Movement { id: 42 }),
        };
        let json = serde_json::to_value(&locked).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "locked",
                "holder": { "type": "movement", "id": 42 }
            })
        );
        assert_eq!(
            serde_json::to_value(&VtxoState::Spendable).unwrap(),
            serde_json::json!({ "type": "spendable" })
        );
    }

    // ------------------------------------------------------------------
    // ExitState (item: exit state block refs / txids)
    // ------------------------------------------------------------------

    #[test]
    fn exit_state_conversion_carries_payloads() {
        use bark::exit as be;

        let claimed: ExitState = (&be::ExitState::Claimed(be::ExitClaimedState {
            tip_height: 900.into(),
            txid: txid(0xaa),
            block: block_ref(890, 0xbb),
        }))
            .into();
        assert_eq!(
            claimed,
            ExitState::Claimed {
                tip_height: 900,
                txid: hex::encode([0xaa; 32]),
                block: BlockRef {
                    height: 890,
                    hash: hex::encode([0xbb; 32])
                },
            }
        );

        let awaiting: ExitState = (&be::ExitState::AwaitingDelta(be::ExitAwaitingDeltaState {
            tip_height: 100.into(),
            confirmed_block: block_ref(95, 0xcc),
            claimable_height: 107.into(),
        }))
            .into();
        assert_eq!(
            awaiting,
            ExitState::AwaitingDelta {
                tip_height: 100,
                confirmed_block: BlockRef {
                    height: 95,
                    hash: hex::encode([0xcc; 32])
                },
                claimable_height: 107,
            }
        );
    }

    #[test]
    fn exit_state_awaiting_inputs_txids_are_sorted() {
        use bark::exit as be;
        use std::collections::HashSet;

        let upstream = be::ExitState::Processing(be::ExitProcessingState {
            tip_height: 5.into(),
            transactions: vec![be::ExitTx {
                txid: txid(0x01),
                status: be::ExitTxStatus::AwaitingInputConfirmation {
                    txids: HashSet::from([txid(0xff), txid(0x02), txid(0x0a)]),
                },
            }],
        });
        let state: ExitState = (&upstream).into();
        let ExitState::Processing { transactions, .. } = state else {
            panic!("expected processing");
        };
        let ExitTxStatus::AwaitingInputConfirmation { txids } = &transactions[0].status else {
            panic!("expected awaiting-input-confirmation");
        };
        let mut sorted = txids.clone();
        sorted.sort_unstable();
        assert_eq!(txids, &sorted);
        assert_eq!(txids.len(), 3);
    }

    #[test]
    fn exit_state_serde_tags_match_bark_web_domain() {
        // bark-web's domain type expects kebab-case `type` tags with
        // camelCase fields (same shape barkd serves). Guard the contract.
        let state = ExitState::ClaimInProgress {
            tip_height: 12,
            claimable_since: BlockRef {
                height: 10,
                hash: "ab".into(),
            },
            claim_txid: "deadbeef".into(),
        };
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::json!({
                "type": "claim-in-progress",
                "tipHeight": 12,
                "claimableSince": { "height": 10, "hash": "ab" },
                "claimTxid": "deadbeef"
            })
        );

        let state = ExitState::VtxoAlreadySpent { tip_height: 3 };
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::json!({ "type": "vtxo-already-spent", "tipHeight": 3 })
        );
    }

    // ------------------------------------------------------------------
    // FeeSchedule (item: fee schedule as raw JSON)
    // ------------------------------------------------------------------

    /// Mirrors bark-json's `EmergencyExitFeeEstimateResponse`: total is the
    /// sum of both legs and the rate is rounded up to whole sat/vB.
    #[test]
    fn emergency_exit_fee_estimate_conversion_and_serde_roundtrip() {
        use bitcoin::{Amount, FeeRate};

        let upstream = bark::exit::ExitFeeEstimate {
            exit_broadcast_fee: Amount::from_sat(4576),
            claim_fee: Amount::from_sat(645),
            // 2.5 sat/vB → reported as 3 sat/vB, like bark-json.
            fee_rate: FeeRate::from_sat_per_kwu(625),
            txs_to_broadcast: 3,
        };

        let local: EmergencyExitFeeEstimate = upstream.into();
        assert_eq!(local.exit_broadcast_fee_sats, 4576);
        assert_eq!(local.claim_fee_sats, 645);
        assert_eq!(local.total_fee_sats, 4576 + 645);
        assert_eq!(local.fee_rate_sat_per_vb, 3);
        assert_eq!(local.txs_to_broadcast, 3);

        let json = serde_json::to_string(&local).unwrap();
        let back: EmergencyExitFeeEstimate = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_fee_sats, local.total_fee_sats);
        assert_eq!(back.fee_rate_sat_per_vb, local.fee_rate_sat_per_vb);
        assert_eq!(back.txs_to_broadcast, local.txs_to_broadcast);
    }

    #[test]
    fn fee_schedule_conversion_and_serde_shape() {
        use ark::fees as af;
        use bitcoin::Amount;

        let upstream = af::FeeSchedule {
            board: af::BoardFees {
                min_fee: Amount::from_sat(100),
                base_fee: Amount::from_sat(10),
                ppm: af::PpmFeeRate(4000),
            },
            offboard: af::OffboardFees {
                base_fee: Amount::from_sat(20),
                fixed_additional_vb: 110,
                ppm_expiry_table: vec![af::PpmExpiryFeeEntry {
                    expiry_blocks_threshold: 144,
                    ppm: af::PpmFeeRate(500),
                }],
            },
            refresh: af::RefreshFees {
                base_fee: Amount::from_sat(30),
                ppm_expiry_table: vec![
                    af::PpmExpiryFeeEntry {
                        expiry_blocks_threshold: 144,
                        ppm: af::PpmFeeRate(100),
                    },
                    af::PpmExpiryFeeEntry {
                        expiry_blocks_threshold: 288,
                        ppm: af::PpmFeeRate(200),
                    },
                ],
            },
            lightning_receive: af::LightningReceiveFees {
                base_fee: Amount::from_sat(40),
                ppm: af::PpmFeeRate(600),
            },
            lightning_send: af::LightningSendFees {
                min_fee: Amount::from_sat(50),
                base_fee: Amount::from_sat(5),
                ppm_expiry_table: vec![],
            },
        };

        let ffi: FeeSchedule = (&upstream).into();
        assert_eq!(ffi.board.min_fee_sats, 100);
        assert_eq!(ffi.board.ppm, 4000);
        assert_eq!(ffi.offboard.fixed_additional_vb, 110);
        assert_eq!(ffi.refresh.ppm_expiry_table.len(), 2);
        assert_eq!(ffi.refresh.ppm_expiry_table[1].expiry_blocks_threshold, 288);
        assert_eq!(ffi.refresh.ppm_expiry_table[1].ppm, 200);
        assert_eq!(ffi.lightning_receive.ppm, 600);
        assert_eq!(ffi.lightning_send.min_fee_sats, 50);
        assert!(ffi.lightning_send.ppm_expiry_table.is_empty());

        // Field naming contract for serialized wallet data.
        let json = serde_json::to_value(&ffi).unwrap();
        assert_eq!(json["board"]["minFeeSats"], 100);
        assert_eq!(
            json["refresh"]["ppmExpiryTable"][0]["expiryBlocksThreshold"],
            144
        );
        assert_eq!(json["lightningReceive"]["ppm"], 600);
        assert_eq!(json["lightningSend"]["baseFeeSats"], 5);
    }

    // ------------------------------------------------------------------
    // Onchain data (item: on-chain data, CPFP detection, raw tx hex)
    // ------------------------------------------------------------------

    #[test]
    fn wallet_transaction_conversion_roundtrips_tx_hex() {
        use bitcoin::absolute::LockTime;
        use bitcoin::transaction::Version;
        use std::sync::Arc;

        let tx = bitcoin::Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(1234),
                script_pubkey: bitcoin::ScriptBuf::new(),
            }],
        };
        let info = bark::onchain::WalletTxInfo {
            txid: tx.compute_txid(),
            tx: Arc::new(tx.clone()),
            onchain_fees: Some(bitcoin::Amount::from_sat(200)),
            balance_change: bitcoin::SignedAmount::from_sat(-1434),
            confirmation: Some(block_ref(500, 0x11)),
            is_cpfp: true,
        };

        let ffi: WalletTransaction = (&info).into();
        assert_eq!(ffi.txid, tx.compute_txid().to_string());
        assert_eq!(ffi.onchain_fee_sats, Some(200));
        assert_eq!(ffi.balance_change_sats, -1434);
        assert!(ffi.is_cpfp);
        assert_eq!(ffi.confirmation.as_ref().unwrap().height, 500);

        // The hex must decode back to the same transaction.
        let decoded: bitcoin::Transaction =
            bitcoin::consensus::encode::deserialize_hex(&ffi.tx_hex).unwrap();
        assert_eq!(decoded.compute_txid(), tx.compute_txid());
    }

    #[test]
    fn onchain_utxo_local_conversion_and_tags() {
        let upstream = bark::onchain::Utxo::Local(bark::onchain::LocalUtxo {
            outpoint: bitcoin::OutPoint {
                txid: txid(0x33),
                vout: 1,
            },
            amount: bitcoin::Amount::from_sat(5000),
            confirmation_height: None,
        });
        let ffi: OnchainUtxo = (&upstream).into();
        let json = serde_json::to_value(&ffi).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "local",
                "outpoint": { "txid": hex::encode([0x33; 32]), "vout": 1 },
                "amountSats": 5000,
                "confirmationHeight": null
            })
        );
    }

    // ------------------------------------------------------------------
    // RoundFlowKind / RecoveryStatus (bark 0.7.0)
    // ------------------------------------------------------------------

    #[test]
    fn round_flow_kind_maps_every_upstream_variant() {
        use bark::round::RoundFlowKind as B;
        let cases = [
            (B::DelegatedPending, RoundFlowKind::DelegatedPending),
            (B::Pending, RoundFlowKind::Pending),
            (B::Ongoing, RoundFlowKind::Ongoing),
            (
                B::AwaitingConfirmations,
                RoundFlowKind::AwaitingConfirmations,
            ),
            (B::Failed, RoundFlowKind::Failed),
            (B::Canceled, RoundFlowKind::Canceled),
        ];
        for (upstream, expected) in cases {
            // Tag matches upstream's Display form, so a TS client can compare
            // against `bark` CLI output.
            assert_eq!(
                serde_json::to_value(RoundFlowKind::from(upstream)).unwrap(),
                serde_json::Value::String(upstream.to_string()),
            );
            assert_eq!(RoundFlowKind::from(upstream), expected);
        }
    }

    #[test]
    fn recovery_status_not_run_has_no_report() {
        let status = RecoveryStatus::from(bark::RecoveryStatus::NotRun);
        assert!(matches!(status, RecoveryStatus::NotRun));
        assert!(status.report().is_none());
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({ "type": "not-run" })
        );
    }

    #[test]
    fn recovery_status_failed_keeps_full_cause_chain() {
        let err = anyhow::anyhow!("mailbox stream closed").context("recovery scan failed");
        let status = RecoveryStatus::from(bark::RecoveryStatus::Failed(err));
        let RecoveryStatus::Failed { message } = &status else {
            panic!("expected Failed, got {status:?}");
        };
        assert!(message.contains("recovery scan failed"), "{message}");
        assert!(message.contains("mailbox stream closed"), "{message}");
        assert!(status.report().is_none());
        assert_eq!(
            serde_json::to_value(&status).unwrap()["type"],
            serde_json::json!("failed")
        );
    }

    #[test]
    fn recovery_status_completed_exposes_report() {
        let status = RecoveryStatus::Completed {
            report: RecoveryReport::default(),
        };
        assert!(status.report().is_some());
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["type"], serde_json::json!("completed"));
        assert!(json["report"].is_object(), "{json}");
    }

    #[test]
    fn log_level_name_matches_serde() {
        for level in log::Level::iter().map(LogLevel::from) {
            assert_eq!(serde_json::to_value(level).unwrap(), level.as_str());
        }
    }

    #[test]
    fn log_level_maps_to_the_same_log_filter() {
        for level in log::Level::iter() {
            assert_eq!(
                log::LevelFilter::from(LogLevel::from(level)),
                level.to_level_filter()
            );
        }
    }
    #[test]
    fn swept_exit_preserves_spent_inputs() {
        let upstream = bark::exit::ExitState::new_vtxo_swept(
            123.into(),
            vec![bitcoin::OutPoint {
                txid: txid(0x42),
                vout: 7,
            }],
        );
        let local = ExitState::from(&upstream);
        assert_eq!(
            local,
            ExitState::VtxoSwept {
                tip_height: 123,
                spent_inputs: vec![OutPoint {
                    txid: hex::encode([0x42; 32]),
                    vout: 7
                }]
            }
        );
        assert_eq!(serde_json::to_value(&local).unwrap()["type"], "vtxo-swept");
        assert_eq!(
            ExitStateKind::from(bark::exit::ExitStateKind::VtxoSwept),
            ExitStateKind::VtxoSwept
        );
    }
}
/// A persisted wallet key; private key material never crosses the FFI.
#[boltffi::data]
pub struct StoredPublicKey {
    pub public_key: String,
    pub index: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct PaymentMethod {
    pub method_type: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct MovementDestination {
    pub payment_method: PaymentMethod,
    pub amount_sats: u64,
}

impl From<&bark::movement::PaymentMethod> for PaymentMethod {
    fn from(method: &bark::movement::PaymentMethod) -> Self {
        Self {
            method_type: method.type_str().into(),
            value: method.value_string(),
        }
    }
}

impl TryFrom<PaymentMethod> for bark::movement::PaymentMethod {
    type Error = crate::BarkError;

    fn try_from(method: PaymentMethod) -> Result<Self, Self::Error> {
        Ok(Self::from_type_value(&method.method_type, &method.value)?)
    }
}

impl From<&bark::movement::MovementDestination> for MovementDestination {
    fn from(destination: &bark::movement::MovementDestination) -> Self {
        Self {
            payment_method: (&destination.destination).into(),
            amount_sats: destination.amount.to_sat(),
        }
    }
}

// ============================================================================
// Lightning send options and payment requests
// ============================================================================

/// Options for a Lightning payment, mirroring `bark::LightningSendOptions`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[boltffi::data]
pub struct LightningSendOptions {
    /// Keep the call open until the payment settles or fails, instead of
    /// returning once it has started.
    ///
    /// Default: false
    #[boltffi::default(false)]
    pub wait: bool,
    /// Seconds the server keeps trying to pay. The server caps it at its
    /// maximum (5 minutes by default); 0 makes a single attempt.
    ///
    /// Default: unset, the server's own retry time
    #[boltffi::default(None)]
    pub retry_for_secs: Option<u32>,
}

impl From<LightningSendOptions> for bark::LightningSendOptions {
    fn from(options: LightningSendOptions) -> Self {
        Self::default().wait(options.wait).retry_for(
            options
                .retry_for_secs
                .map(|secs| std::time::Duration::from_secs(secs.into())),
        )
    }
}

/// Why an Ark address cannot receive an arkoor payment from this wallet.
/// Mirrors `bark::ArkoorAddressError`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[boltffi::data]
pub enum ArkAddressError {
    NetworkMismatch,
    /// Another Ark server's address, including Arkade addresses.
    ServerMismatch,
    PolicyNotSupported {
        policy: String,
    },
    UnknownDeliveryMechanism {
        mechanism: String,
    },
    Other {
        message: String,
    },
}

impl From<&bark::ArkoorAddressError> for ArkAddressError {
    fn from(error: &bark::ArkoorAddressError) -> Self {
        use bark::ArkoorAddressError as E;
        match error {
            E::NetworkMismatch => Self::NetworkMismatch,
            E::ServerMismatch => Self::ServerMismatch,
            E::PolicyNotSupported(policy) => Self::PolicyNotSupported {
                policy: policy.policy_type().to_string(),
            },
            E::UnknownDeliveryMechanism(mechanism) => Self::UnknownDeliveryMechanism {
                mechanism: mechanism.clone(),
            },
            E::Other(message) => Self::Other {
                message: message.clone(),
            },
        }
    }
}

/// A problem with one option of a parsed payment request.
/// Mirrors `bark::PaymentMethodParsingError`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[boltffi::data]
pub enum PaymentMethodParsingError {
    /// The destination is for another bitcoin network.
    NetworkMismatch,
    InvalidArkAddress {
        error: ArkAddressError,
    },
    /// The URI amount differs from the amount the destination encodes.
    AmountMismatch {
        expected_sats: u64,
        got_sats: u64,
    },
    InvalidAmount,
    Unsupported,
}

impl From<&bark::PaymentMethodParsingError> for PaymentMethodParsingError {
    fn from(error: &bark::PaymentMethodParsingError) -> Self {
        use bark::PaymentMethodParsingError as E;
        match error {
            E::NetworkMismatch => Self::NetworkMismatch,
            E::InvalidArkAddress(error) => Self::InvalidArkAddress {
                error: error.into(),
            },
            E::AmountMismatch { expected, got } => Self::AmountMismatch {
                expected_sats: expected.to_sat(),
                got_sats: got.to_sat(),
            },
            E::InvalidAmount => Self::InvalidAmount,
            E::Unsupported => Self::Unsupported,
        }
    }
}

/// One way to pay a parsed payment request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct PaymentOption {
    pub method: PaymentMethod,
    /// Problems with this option; it is usable only when empty.
    pub errors: Vec<PaymentMethodParsingError>,
    /// The destination carries no amount, so `send_payment` needs one.
    pub requires_amount: bool,
    /// `send_payment` forwards a comment (Lightning Address and LNURL).
    pub supports_comment: bool,
}

impl From<&bark::AvailablePaymentMethod> for PaymentOption {
    fn from(option: &bark::AvailablePaymentMethod) -> Self {
        Self {
            method: (&option.method).into(),
            errors: option.errors.iter().map(Into::into).collect(),
            requires_amount: option.method.requires_amount(),
            supports_comment: option.method.supports_comment(),
        }
    }
}

/// A parsed payment string. Mirrors `bark::PaymentRequest`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct PaymentRequest {
    pub amount_sats: Option<u64>,
    pub label: Option<String>,
    pub message: Option<String>,
    pub options: Vec<PaymentOption>,
    /// Bark's choice when the caller doesn't pick: the first error-free Ark,
    /// then Lightning, then onchain option.
    pub default_option: Option<PaymentOption>,
}

impl From<bark::PaymentRequest> for PaymentRequest {
    fn from(request: bark::PaymentRequest) -> Self {
        Self {
            amount_sats: request.amount.map(|a| a.to_sat()),
            default_option: request.default_option().map(Into::into),
            options: request.options.iter().map(Into::into).collect(),
            label: request.label,
            message: request.message,
        }
    }
}

/// Estimated fee for paying with one method.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct PaymentMethodFee {
    pub method: PaymentMethod,
    pub fee: FeeEstimate,
}

/// Outcome of `Wallet::send_payment`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub enum PaymentResult {
    /// The onchain send was broadcast.
    Onchain { txid: String, movement_id: u32 },
    /// The Lightning payment's state after it started, or after it settled
    /// or failed when `wait` is set.
    Lightning { status: LightningSendStatus },
    /// The arkoor payment completed.
    Ark { movement_id: u32 },
}

/// What `Wallet::bip321_uri` includes. Mirrors bark's `BarkBip321UriBuilder`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct Bip321UriOptions {
    #[boltffi::default(None)]
    pub amount_sats: Option<u64>,
    #[boltffi::default(None)]
    pub label: Option<String>,
    #[boltffi::default(None)]
    pub message: Option<String>,
    /// Include a new Ark address.
    ///
    /// Default: true
    #[boltffi::default(true)]
    pub ark: bool,
    /// Include a new address from the attached onchain wallet. Skipped when
    /// the wallet has no onchain wallet.
    ///
    /// Default: true
    #[boltffi::default(true)]
    pub onchain: bool,
    /// Include a BOLT11 invoice for `amount_sats`. Skipped without an amount.
    ///
    /// Default: true
    #[boltffi::default(true)]
    pub lightning: bool,
}

impl Default for Bip321UriOptions {
    fn default() -> Self {
        Self {
            amount_sats: None,
            label: None,
            message: None,
            ark: true,
            onchain: true,
            lightning: true,
        }
    }
}

/// A BIP 321 payment URI and the destinations generated for it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct Bip321Uri {
    pub uri: String,
    pub ark_address: Option<String>,
    pub onchain_address: Option<String>,
    /// A pending Lightning receive, like one from `bolt11_invoice`.
    pub invoice: Option<LightningInvoice>,
}

/// Wallet identifiers to share in support requests. Mirrors `bark::DebugInfo`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct DebugInfo {
    pub network: Network,
    /// Hex-encoded server mailbox id, as returned by `mailbox_identifier`.
    pub mailbox_id: String,
    /// The xpub every VTXO key derives from. It reveals the wallet's VTXO
    /// public keys but cannot spend.
    pub vtxo_xpub: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct ParsedArkAddress {
    pub user_pubkey: Option<String>,
    pub ark_id: String,
    pub is_testnet: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct MailboxAuthorization {
    pub mailbox_id: String,
    pub expiry: i64,
    pub encoded: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[boltffi::data]
pub enum RoundOutcome {
    Confirmed,
    Unconfirmed,
    Pending,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct PendingRoundStatus {
    pub round_id: u32,
    pub status: RoundOutcome,
    pub funding_txid: Option<String>,
    /// Empty on Bark bfbe6b7c, whose RoundStatus has no unsigned-funding variant.
    pub unsigned_funding_txids: Vec<String>,
    pub error: Option<String>,
    pub is_final: bool,
    pub is_success: bool,
}

impl PendingRoundStatus {
    pub(crate) fn new(round_id: u32, value: bark::round::RoundStatus) -> Self {
        use bark::round::RoundStatus as R;
        let is_final = value.is_final();
        let is_success = value.is_success();
        let (status, funding_txid, error) = match value {
            R::Confirmed { funding_txid } => (
                RoundOutcome::Confirmed,
                Some(funding_txid.to_string()),
                None,
            ),
            R::Unconfirmed { funding_txid } => (
                RoundOutcome::Unconfirmed,
                Some(funding_txid.to_string()),
                None,
            ),
            R::Pending => (RoundOutcome::Pending, None, None),
            R::Failed { error } => (RoundOutcome::Failed, None, Some(error)),
            R::Canceled => (RoundOutcome::Canceled, None, None),
        };
        Self {
            round_id,
            status,
            funding_txid,
            unsigned_funding_txids: vec![],
            error,
            is_final,
            is_success,
        }
    }
}

impl From<ark::Vtxo> for Vtxo {
    fn from(v: ark::Vtxo) -> Self {
        Self {
            id: v.id().to_string(),
            amount_sats: v.amount().to_sat(),
            expiry_height: v.expiry_height().into(),
            kind: format!("{:?}", v.policy_type()),
            state: VtxoState::Unknown,
            exit_depth: u32::from(v.exit_depth()),
            exit_tx_weight_wu: v.transactions().map(|tx| tx.tx.weight().to_wu()).sum(),
            registered: false,
            server_pubkey: v.server_pubkey().to_string(),
            exit_delta: u32::from(v.exit_delta()),
            anchor_point: v.chain_anchor().into(),
            point: v.point().into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct ExitTransactionPackage {
    pub parent_txid: String,
    pub parent_tx_hex: String,
    pub child_txid: Option<String>,
    pub child_tx_hex: Option<String>,
    pub child_origin: Option<ExitTxOrigin>,
    /// Present when the current or historical exit state tracks this parent.
    pub status: Option<ExitTxStatus>,
}

#[cfg(test)]
mod noah_parity_tests {
    use super::*;
    use bitcoin::hashes::Hash;
    #[test]
    fn destinations_keep_amounts_and_boarding_keeps_every_id() {
        let destination = bark::movement::MovementDestination::new(
            bark::movement::PaymentMethod::Custom("Noah receipt".into()),
            bitcoin::Amount::from_sat(u64::MAX),
        );
        let converted: MovementDestination = (&destination).into();
        assert_eq!(converted.amount_sats, u64::MAX);
        assert_eq!(converted.payment_method.method_type, "custom");
        assert_eq!(converted.payment_method.value, "Noah receipt");
        let vtxos: Vec<ark::VtxoId> = (0..2)
            .map(|vout| {
                bitcoin::OutPoint {
                    txid: bitcoin::Txid::all_zeros(),
                    vout,
                }
                .into()
            })
            .collect();
        let board: PendingBoard = BarkPendingBoard {
            funding_tx: bitcoin::Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![],
                output: vec![],
            },
            vtxos: vtxos.clone(),
            amount: bitcoin::Amount::from_sat(1000),
            movement_id: bark::movement::MovementId::new(3),
        }
        .into();
        assert_eq!(
            board.vtxo_ids,
            vtxos.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }

    #[test]
    fn pending_round_status_preserves_all_outcomes() {
        use bark::round::RoundStatus as R;
        for (original, expected, finality, success) in [
            (R::Pending, RoundOutcome::Pending, false, false),
            (
                R::Confirmed {
                    funding_txid: bitcoin::Txid::all_zeros(),
                },
                RoundOutcome::Confirmed,
                true,
                true,
            ),
            (
                R::Unconfirmed {
                    funding_txid: bitcoin::Txid::all_zeros(),
                },
                RoundOutcome::Unconfirmed,
                false,
                true,
            ),
            (
                R::Failed {
                    error: "denied".into(),
                },
                RoundOutcome::Failed,
                true,
                false,
            ),
            (R::Canceled, RoundOutcome::Canceled, true, false),
        ] {
            let converted = PendingRoundStatus::new(9, original);
            assert_eq!(converted.round_id, 9);
            assert_eq!(converted.status, expected);
            assert_eq!(converted.is_final, finality);
            assert_eq!(converted.is_success, success);
            assert_eq!(converted.funding_txid.is_some(), success);
            assert_eq!(
                converted.error.as_deref(),
                if expected == RoundOutcome::Failed {
                    Some("denied")
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn pure_decode_preserves_public_fields_and_local_state_is_unknown() {
        let hex = include_str!("../tests/fixtures/board-vtxo.hex");
        let decoded = crate::decode_vtxo(hex.into()).unwrap();
        let original = crate::core::wallet::parse_vtxo(hex).unwrap();
        assert_eq!(decoded.id, original.id().to_string());
        assert_eq!(decoded.server_pubkey, original.server_pubkey().to_string());
        assert_eq!(decoded.exit_delta, u32::from(original.exit_delta()));
        assert_eq!(decoded.anchor_point, original.chain_anchor().into());
        assert_eq!(decoded.point, original.point().into());
        assert!(matches!(decoded.state, VtxoState::Unknown));
        assert!(!decoded.registered);
        use base64::Engine;
        let base64 =
            base64::engine::general_purpose::STANDARD.encode(hex::decode(hex.trim()).unwrap());
        assert_eq!(crate::decode_vtxo(base64).unwrap().id, decoded.id);
        assert!(crate::decode_vtxo("not a VTXO".into()).is_err());
    }
}

#[cfg(test)]
mod balance_contract_tests {
    #[test]
    fn every_bucket_and_upstream_aggregates_survive_conversion() {
        let sats = bitcoin::Amount::from_sat;
        let b = bark::Balance {
            spendable: sats(11),
            needs_refresh: sats(23),
            pending_lightning_send: sats(37),
            claimable_lightning_receive: sats(41),
            pending_in_round: sats(53),
            pending_exit: sats(67),
            pending_board: sats(71),
            pending_arkoor_send: sats(83),
            pending_offboard: sats(97),
        };
        let ffi = super::Balance::from(b.clone());
        assert_eq!(ffi.pending_sats, b.pending().to_sat());
        assert_eq!(ffi.total_sats, b.total().to_sat());
        assert_eq!(
            [
                ffi.spendable_sats,
                ffi.needs_refresh_sats,
                ffi.pending_lightning_send_sats,
                ffi.claimable_lightning_receive_sats,
                ffi.pending_in_round_sats,
                ffi.pending_exit_sats,
                ffi.pending_board_sats,
                ffi.pending_arkoor_send_sats,
                ffi.pending_offboard_sats
            ],
            [11, 23, 37, 41, 53, 67, 71, 83, 97]
        );
    }
}
