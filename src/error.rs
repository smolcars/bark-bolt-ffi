use std::fmt;

/// Stable error categories; messages retain the underlying diagnostic.
#[boltffi::data]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BarkErrorCode {
    Other,
    InvalidAddress,
    InvalidAmount,
    InvalidFeeRate,
    InsufficientFunds,
    Dust,
    PreparedTransactionNotFound,
    PreparedInputsUnavailable,
    UnsupportedOperation,
    Persistence,
    BelowMinimumBoardAmount,
    TooManyInputs,
    PreparedBoardChanged,
    /// The input is no payment string Bark understands.
    InvalidPaymentRequest,
    // Emergency exit refusals that retrying cannot fix. `BarkError.vtxoId` names the VTXO.
    ExitNonStandardVtxo,
    ExitDustLimit,
    ExitVtxoAlreadySpent,
    ExitVtxoAlreadyExited,
    ExitVtxoSwept,
    ExitUnknownVtxo,
    ExitCyclicTransactions,
    ExitMissingAnchorOutput,
    // Emergency exit chain-source failures; retry later.
    ExitTransactionRetrievalFailure,
    ExitTipRetrievalFailure,
    ExitBlockRetrievalFailure,
    ExitAncestorRetrievalFailure,
    /// Confirmed onchain funds cannot pay the exit's CPFP fees. See
    /// `BarkError.neededSats` and `BarkError.availableSats`.
    ExitInsufficientConfirmedFunds,
}

/// Error message including the complete underlying cause chain.
#[boltffi::error]
#[derive(Clone, Debug)]
pub struct BarkError {
    pub message: String,
    #[boltffi::default(BarkErrorCode::Other)]
    pub code: BarkErrorCode,
    /// The VTXO an exit error refers to, when Bark names one.
    #[boltffi::default(None)]
    pub vtxo_id: Option<String>,
    /// Sats an exit needs, for `ExitInsufficientConfirmedFunds`.
    #[boltffi::default(None)]
    pub needed_sats: Option<u64>,
    /// Confirmed sats available, for `ExitInsufficientConfirmedFunds`.
    #[boltffi::default(None)]
    pub available_sats: Option<u64>,
}

impl BarkError {
    pub(crate) fn typed(code: BarkErrorCode, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code,
            vtxo_id: None,
            needed_sats: None,
            available_sats: None,
        }
    }

    pub fn new(error: impl Into<anyhow::Error>) -> Self {
        Self::from(error.into())
    }

    pub fn message(&self) -> String {
        self.message.clone()
    }
}

impl fmt::Display for BarkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for BarkError {}
impl From<anyhow::Error> for BarkError {
    fn from(error: anyhow::Error) -> Self {
        let message = format!("{error:#}");
        // downcast_ref also finds the error beneath `.context(...)` layers.
        if let Some(exit) = error.downcast_ref::<bark::exit::ExitError>() {
            return Self {
                message,
                ..exit_error(exit)
            };
        }
        Self::typed(
            error
                .downcast_ref::<bark::onchain::bdk_wallet::error::CreateTxError>()
                .map_or(BarkErrorCode::Other, construction_error_code),
            message,
        )
    }
}
impl From<String> for BarkError {
    fn from(message: String) -> Self {
        Self::typed(BarkErrorCode::Other, message)
    }
}

/// Classify a Bark exit error, keeping the VTXO id and amounts it carries.
pub(crate) fn exit_error(error: &bark::exit::ExitError) -> BarkError {
    use bark::exit::ExitError as E;
    use BarkErrorCode as C;
    let (code, vtxo) = match error {
        E::NonStandardVtxo { vtxo, .. } => (C::ExitNonStandardVtxo, Some(vtxo)),
        E::DustLimit { vtxo, .. } => (C::ExitDustLimit, Some(vtxo)),
        E::VtxoAlreadySpent { vtxo } => (C::ExitVtxoAlreadySpent, Some(vtxo)),
        E::VtxoAlreadyExited { vtxo } => (C::ExitVtxoAlreadyExited, Some(vtxo)),
        E::VtxoSwept { vtxo } => (C::ExitVtxoSwept, Some(vtxo)),
        E::UnknownVtxo { vtxo } => (C::ExitUnknownVtxo, Some(vtxo)),
        E::CyclicExitTransactions { vtxo } => (C::ExitCyclicTransactions, Some(vtxo)),
        E::MissingAnchorOutput { .. } => (C::ExitMissingAnchorOutput, None),
        E::TransactionRetrievalFailure { .. } => (C::ExitTransactionRetrievalFailure, None),
        E::TipRetrievalFailure { .. } => (C::ExitTipRetrievalFailure, None),
        E::BlockRetrievalFailure { .. } => (C::ExitBlockRetrievalFailure, None),
        E::AncestorRetrievalFailure { .. } => (C::ExitAncestorRetrievalFailure, None),
        E::InsufficientConfirmedFunds { .. } => (C::ExitInsufficientConfirmedFunds, None),
        E::CannotCancelExit { vtxo, .. }
        | E::ClaimMissingSignableClause { vtxo }
        | E::ExitTxAlreadyBroadcast { vtxo, .. }
        | E::NotExiting { vtxo }
        | E::VtxoNotClaimable { vtxo } => (C::Other, Some(vtxo)),
        E::DatabaseVtxoStoreFailure { vtxo_id, .. } => (C::Other, Some(vtxo_id)),
        _ => (C::Other, None),
    };
    let (needed, available) = match error {
        E::InsufficientConfirmedFunds { needed, available } => {
            (Some(needed.to_sat()), Some(available.to_sat()))
        }
        _ => (None, None),
    };
    BarkError {
        vtxo_id: vtxo.map(ToString::to_string),
        needed_sats: needed,
        available_sats: available,
        ..BarkError::typed(code, error.to_string())
    }
}

pub(crate) fn construction_error_code(
    error: &bark::onchain::bdk_wallet::error::CreateTxError,
) -> BarkErrorCode {
    use bark::onchain::bdk_wallet::error::CreateTxError;
    match error {
        CreateTxError::CoinSelection(_) | CreateTxError::NoUtxosSelected => {
            BarkErrorCode::InsufficientFunds
        }
        CreateTxError::OutputBelowDustLimit(_) => BarkErrorCode::Dust,
        _ => BarkErrorCode::Other,
    }
}
impl From<&str> for BarkError {
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}
impl From<boltffi::UnexpectedFfiCallbackError> for BarkError {
    fn from(error: boltffi::UnexpectedFfiCallbackError) -> Self {
        Self::from(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_message() {
        let err = BarkError::from(anyhow::anyhow!("something broke"));
        assert_eq!(err.message(), "something broke");
    }

    #[test]
    fn shows_full_cause_chain() {
        // Display uses the alternate `{:#}` form, which joins the anyhow chain.
        let err = BarkError::from(anyhow::anyhow!("root cause").context("outer context"));
        let msg = err.message();
        assert!(msg.contains("outer context"), "{msg}");
        assert!(msg.contains("root cause"), "{msg}");
    }

    #[test]
    fn exit_errors_keep_distinct_codes_under_context() {
        use bark::exit::ExitError as E;
        use bitcoin::hashes::Hash;
        use bitcoin::{Amount, Txid};
        let vtxo = ark::VtxoId::from(bitcoin::OutPoint::new(Txid::all_zeros(), 7));
        let txid = Txid::all_zeros();
        let error = || "boom".to_owned();
        let cases = [
            (
                E::NonStandardVtxo {
                    vtxo,
                    error: ark::vtxo::VtxoStandardnessError::Dusty,
                },
                BarkErrorCode::ExitNonStandardVtxo,
            ),
            (
                E::DustLimit {
                    vtxo,
                    amount: Amount::from_sat(1),
                    dust: Amount::from_sat(330),
                },
                BarkErrorCode::ExitDustLimit,
            ),
            (
                E::VtxoAlreadySpent { vtxo },
                BarkErrorCode::ExitVtxoAlreadySpent,
            ),
            (
                E::VtxoAlreadyExited { vtxo },
                BarkErrorCode::ExitVtxoAlreadyExited,
            ),
            (E::VtxoSwept { vtxo }, BarkErrorCode::ExitVtxoSwept),
            (E::UnknownVtxo { vtxo }, BarkErrorCode::ExitUnknownVtxo),
            (
                E::CyclicExitTransactions { vtxo },
                BarkErrorCode::ExitCyclicTransactions,
            ),
            (
                E::MissingAnchorOutput { txid },
                BarkErrorCode::ExitMissingAnchorOutput,
            ),
            (
                E::TransactionRetrievalFailure {
                    txid,
                    error: error(),
                },
                BarkErrorCode::ExitTransactionRetrievalFailure,
            ),
            (
                E::TipRetrievalFailure { error: error() },
                BarkErrorCode::ExitTipRetrievalFailure,
            ),
            (
                E::BlockRetrievalFailure {
                    height: 1u32.into(),
                    error: error(),
                },
                BarkErrorCode::ExitBlockRetrievalFailure,
            ),
            (
                E::AncestorRetrievalFailure {
                    txid,
                    error: error(),
                },
                BarkErrorCode::ExitAncestorRetrievalFailure,
            ),
            (
                E::InsufficientConfirmedFunds {
                    needed: Amount::from_sat(5_000),
                    available: Amount::from_sat(1_200),
                },
                BarkErrorCode::ExitInsufficientConfirmedFunds,
            ),
            (E::InternalError { error: error() }, BarkErrorCode::Other),
        ];
        let mut seen = std::collections::HashSet::new();
        for (exit, code) in cases {
            let names_vtxo = exit.to_string().contains(&vtxo.to_string());
            let converted = BarkError::from(anyhow::Error::from(exit).context("outer context"));
            assert_eq!(converted.code, code);
            assert!(
                converted.message.starts_with("outer context: "),
                "{}",
                converted.message
            );
            assert_eq!(converted.vtxo_id.is_some(), names_vtxo, "{code:?}");
            assert!(
                code == BarkErrorCode::Other || seen.insert(code),
                "{code:?} reused"
            );
            if code == BarkErrorCode::ExitInsufficientConfirmedFunds {
                assert_eq!(
                    (converted.needed_sats, converted.available_sats),
                    (Some(5_000), Some(1_200))
                );
            } else {
                assert_eq!(
                    (converted.needed_sats, converted.available_sats),
                    (None, None)
                );
            }
        }
    }
}
