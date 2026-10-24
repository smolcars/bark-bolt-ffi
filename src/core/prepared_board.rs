use std::path::PathBuf;

use anyhow::Context;
use bitcoin::Amount;

use super::prepared_onchain::WalletState;
use super::wallet::Wallet;
use crate::error::{BarkError, BarkErrorCode};
use crate::types::{self, BroadcastOutcome};

#[derive(Clone)]
pub(super) struct BoardPreparation {
    owner: PathBuf,
    keypair: bitcoin::key::Keypair,
    key_index: u32,
    expiry_height: u32,
    scan_start: u32,
    gross: Amount,
    ark_fee: Amount,
    funding_script: bitcoin::ScriptBuf,
    submitted: Option<Vec<String>>,
}

fn unsupported() -> BarkError {
    BarkError::typed(
        BarkErrorCode::UnsupportedOperation,
        "prepared boarding requires the BDK onchain wallet and an Ark data directory",
    )
}

fn unavailable(message: &str) -> BarkError {
    BarkError::typed(BarkErrorCode::PreparedInputsUnavailable, message)
}

fn board_fee(info: &ark::ArkInfo, gross: Amount) -> Result<Amount, BarkError> {
    if gross < info.min_board_amount {
        return Err(BarkError::typed(
            BarkErrorCode::BelowMinimumBoardAmount,
            "board output is below the server minimum",
        ));
    }
    let fee = info
        .fees
        .board
        .calculate(gross)
        .context("board fee overflow")?;
    ark::fees::validate_and_subtract_fee(gross, fee)
        .map_err(|error| BarkError::typed(BarkErrorCode::Dust, format!("{error:#}")))?;
    Ok(fee)
}

impl Wallet {
    pub async fn prepare_board(
        &self,
        amount: types::OnchainSendAmount,
        fee_rate_sat_per_vb: u64,
        confirmed_only: bool,
        excluded_outpoints: Vec<types::OutPoint>,
    ) -> Result<types::PreparedBoard, BarkError> {
        let fee_rate = super::prepared_onchain::fee_rate(fee_rate_sat_per_vb)?;
        let owner = self.board_owner.clone().ok_or_else(unsupported)?;
        let onchain = self.inner.onchain().ok_or_else(unsupported)?;
        let _operation = super::prepared_onchain::operation_guard(Some(onchain.clone())).await;
        // Check the backend before any key or server operation.
        {
            let guard = onchain.read().await;
            if (&*guard as &dyn std::any::Any)
                .downcast_ref::<WalletState>()
                .is_none()
            {
                return Err(unsupported());
            }
        }
        // Lazy server initialization may pin keys in SQLite. Require those keys
        // before calling it so even an offline-created wallet gets no prepare writes.
        let properties = self.inner.properties().await?;
        if properties.server_pubkey.is_none() || properties.server_mailbox_pubkey.is_none() {
            return Err(BarkError::typed(
                BarkErrorCode::UnsupportedOperation,
                "sync the wallet to pin its Ark server keys before preparing a board",
            ));
        }
        let info = self.inner.require_ark_info().await?;
        if let types::OnchainSendAmount::Amount { amount_sats } = &amount {
            if *amount_sats == 0 || *amount_sats > Amount::MAX_MONEY.to_sat() {
                return Err(BarkError::typed(
                    BarkErrorCode::InvalidAmount,
                    "board amount is outside Bitcoin's money range",
                ));
            }
            board_fee(&info, Amount::from_sat(*amount_sats))?;
        }
        let (keypair, key_index) = self.inner.peek_next_keypair().await?;
        let expiry = u32::from(self.inner.chain().tip().await?)
            .checked_add(u32::from(info.vtxo_lifetime))
            .context("board expiry overflow")?;
        let builder = ark::board::BoardBuilder::new(
            keypair.public_key(),
            expiry.into(),
            info.server_pubkey,
            info.vtxo_exit_delta,
        );
        let funding_script = builder.funding_script_pubkey();
        let address = bitcoin::Address::from_script(&funding_script, properties.network)
            .context("invalid board funding script")?;
        let mut guard = onchain.write().await;
        let state = (&mut *guard as &mut dyn std::any::Any)
            .downcast_mut::<WalletState>()
            .ok_or_else(unsupported)?;
        let mut unavailable = state.pending_board_inputs().await?;
        // Unparseable or foreign outpoints cannot be selected anyway, so they are ignored.
        unavailable.extend(excluded_outpoints.iter().filter_map(|point| {
            Some(bitcoin::OutPoint::new(point.txid.parse().ok()?, point.vout))
        }));
        let prepared = state.prepare(address, amount, fee_rate, confirmed_only, &unavailable)?;
        // Failures after coin selection must release their process-local locks too.
        let checked = (|| {
            // Pinned server-rpc::MAX_NB_BOARD_FUNDING_INPUTS; Bark checks it again at submit.
            if prepared.inputs.len() > 100 {
                return Err(BarkError::typed(
                    BarkErrorCode::TooManyInputs,
                    "board funding exceeds the server's 100-input limit",
                ));
            }
            let gross = Amount::from_sat(prepared.amount_sats);
            let ark_fee = board_fee(&info, gross)?;
            Ok((gross, ark_fee))
        })();
        let (gross, ark_fee) = match checked {
            Ok(value) => value,
            Err(error) => {
                let entry = state
                    .prepared
                    .remove(&prepared.id)
                    .expect("new preparation");
                state.release_inputs(&entry.transaction);
                return Err(error);
            }
        };
        let scan_start = state.wallet.latest_checkpoint().height();
        state
            .prepared
            .get_mut(&prepared.id)
            .expect("new preparation")
            .board = Some(BoardPreparation {
            owner,
            scan_start,
            keypair,
            key_index,
            expiry_height: expiry,
            gross,
            ark_fee,
            funding_script,
            submitted: None,
        });
        Ok(types::PreparedBoard {
            id: prepared.id,
            txid: prepared.txid,
            gross_sats: gross.to_sat(),
            net_sats: (gross - ark_fee).to_sat(),
            ark_fee_sats: ark_fee.to_sat(),
            onchain_fee_sats: prepared.fee_sats,
            vsize: prepared.vsize,
            inputs: prepared.inputs,
        })
    }

    pub async fn submit_prepared_board(&self, id: String) -> Result<types::BoardResult, BarkError> {
        let onchain = self.inner.onchain().ok_or_else(unsupported)?;
        let _operation = super::prepared_onchain::operation_guard(Some(onchain.clone())).await;
        let (transaction, psbt, board, cache_recorded) = {
            let mut guard = onchain.write().await;
            let state = (&mut *guard as &mut dyn std::any::Any)
                .downcast_mut::<WalletState>()
                .ok_or_else(unsupported)?;
            let entry = state.prepared.get(&id).ok_or_else(|| {
                BarkError::typed(
                    BarkErrorCode::PreparedTransactionNotFound,
                    "unknown prepared board id",
                )
            })?;
            let board = entry
                .board
                .as_ref()
                .filter(|board| Some(&board.owner) == self.board_owner.as_ref())
                .ok_or_else(|| {
                    BarkError::typed(
                        BarkErrorCode::PreparedTransactionNotFound,
                        "prepared id does not belong to this Ark wallet",
                    )
                })?
                .clone();
            if board.submitted.is_none() {
                let available: std::collections::HashSet<_> = state
                    .wallet
                    .list_unspent()
                    .into_iter()
                    .map(|output| output.outpoint)
                    .collect();
                let pending = state.pending_board_inputs().await?;
                if entry.transaction.input.iter().any(|input| {
                    !available.contains(&input.previous_output)
                        || pending.contains(&input.previous_output)
                }) {
                    return Err(unavailable("prepared board inputs are no longer available"));
                }
            }
            (
                entry.transaction.clone(),
                entry.psbt.clone(),
                board,
                entry.recorded,
            )
        };
        // Check the chain as well as the local graph: another wallet can spend an
        // input while this wallet is idle. Scan only since its last synced tip.
        #[allow(deprecated)]
        let spends = self
            .inner
            .chain()
            .txs_spending_inputs(
                transaction
                    .input
                    .iter()
                    .map(|input| input.previous_output)
                    .collect::<Vec<_>>(),
                board.scan_start.into(),
            )
            .await?;
        if spends
            .map
            .values()
            .any(|(spender, _)| *spender != transaction.compute_txid())
        {
            return Err(unavailable(
                "prepared board inputs were spent by another transaction",
            ));
        }
        let txid = transaction.compute_txid().to_string();
        if matches!(
            self.inner
                .chain()
                .tx_status(transaction.compute_txid())
                .await?,
            bark_bitcoin_ext::TxStatus::NotFound
        ) {
            for input in &transaction.input {
                match self
                    .inner
                    .chain()
                    .tx_status(input.previous_output.txid)
                    .await?
                {
                    bark_bitcoin_ext::TxStatus::NotFound => {
                        return Err(unavailable(
                            "prepared board parent transaction is no longer on the chain source",
                        ))
                    }
                    bark_bitcoin_ext::TxStatus::Confirmed(_)
                        if self
                            .inner
                            .chain()
                            .outpoint_spent_confirmed(input.previous_output)
                            .await? =>
                    {
                        return Err(unavailable(
                            "prepared board input has a confirmed competing spend",
                        ))
                    }
                    _ => {}
                }
            }
        }
        let vtxo_ids = if let Some(ids) = board.submitted {
            ids
        } else {
            let info = self.inner.require_ark_info().await?;
            let builder = ark::board::BoardBuilder::new(
                board.keypair.public_key(),
                board.expiry_height.into(),
                info.server_pubkey,
                info.vtxo_exit_delta,
            );
            if board_fee(&info, board.gross)? != board.ark_fee
                || builder.funding_script_pubkey() != board.funding_script
                || u32::from(self.inner.chain().tip().await?) >= board.expiry_height
            {
                return Err(BarkError::typed(
                    BarkErrorCode::PreparedBoardChanged,
                    "board parameters changed or proposal expired; discard and prepare again",
                ));
            }
            // A key can have been revealed by another operation after preparation.
            if self.inner.peek_keypair(board.key_index).await.is_err() {
                let (stored, index) = self.inner.derive_store_next_keypair().await?;
                if index != board.key_index || stored.public_key() != board.keypair.public_key() {
                    return Err(BarkError::typed(
                        BarkErrorCode::PreparedBoardChanged,
                        "board key changed; prepare again",
                    ));
                }
            }
            // Keep transient locks while Bark cosigns and checkpoints. Its best-effort
            // BDK register may refuse these locks; commit the same tx before releasing them.
            let pending = match self
                .inner
                .board_psbt(psbt, board.keypair, board.expiry_height.into())
                .await
            {
                Ok(pending) => pending,
                Err(error) => {
                    return Ok(types::BoardResult {
                        txid,
                        broadcast: BroadcastOutcome::NotAttempted {
                            reason: format!("{error:#}"),
                        },
                        vtxo_ids: Vec::new(),
                    })
                }
            };
            let ids: Vec<_> = pending.vtxos.iter().map(ToString::to_string).collect();
            let mut guard = onchain.write().await;
            let state = (&mut *guard as &mut dyn std::any::Any)
                .downcast_mut::<WalletState>()
                .ok_or_else(unsupported)?;
            let entry = state
                .prepared
                .get_mut(&id)
                .expect("operation gate protects board");
            entry.board.as_mut().expect("board preparation").submitted = Some(ids.clone());
            ids
        };
        if !cache_recorded {
            let mut guard = onchain.write().await;
            let state = (&mut *guard as &mut dyn std::any::Any)
                .downcast_mut::<WalletState>()
                .ok_or_else(unsupported)?;
            if let Err(error) = state.record_prepared(&transaction).await {
                // Bark has already attempted broadcast and durably accepted the board.
                // Keep its locks until a retry can commit the funding graph as well.
                return Ok(types::BoardResult { txid, vtxo_ids, broadcast: BroadcastOutcome::Failed { reason: format!("board accepted but funding cache write failed; retry submit: {error:#}") } });
            }
            state
                .prepared
                .get_mut(&id)
                .expect("operation gate protects board")
                .recorded = true;
            state.release_inputs(&transaction);
        }
        // Bark deliberately hides its initial best-effort broadcast error. Return
        // the result of an explicit attempt with exactly the reviewed signed bytes.
        let broadcast = match self.inner.chain().broadcast_tx(&transaction).await {
            Ok(()) => BroadcastOutcome::Accepted,
            Err(error) => BroadcastOutcome::Failed {
                reason: format!("{error:#}"),
            },
        };
        Ok(types::BoardResult {
            txid,
            broadcast,
            vtxo_ids,
        })
    }

    pub async fn discard_prepared_board(&self, id: String) -> Result<(), BarkError> {
        let onchain = self.inner.onchain().ok_or_else(unsupported)?;
        let _operation = super::prepared_onchain::operation_guard(Some(onchain.clone())).await;
        let mut guard = onchain.write().await;
        let state = (&mut *guard as &mut dyn std::any::Any)
            .downcast_mut::<WalletState>()
            .ok_or_else(unsupported)?;
        let entry = state
            .prepared
            .get(&id)
            .filter(|entry| {
                entry
                    .board
                    .as_ref()
                    .is_some_and(|board| Some(&board.owner) == self.board_owner.as_ref())
            })
            .ok_or_else(|| {
                BarkError::typed(
                    BarkErrorCode::PreparedTransactionNotFound,
                    "unknown prepared board id",
                )
            })?;
        if !entry.recorded
            && entry
                .board
                .as_ref()
                .is_some_and(|board| board.submitted.is_some())
        {
            return Err(BarkError::typed(
                BarkErrorCode::Persistence,
                "board accepted but funding cache write failed; retry submit before discarding",
            ));
        }
        if !entry.recorded {
            let transaction = entry.transaction.clone();
            state.release_inputs(&transaction);
        }
        state.prepared.remove(&id);
        Ok(())
    }
}
