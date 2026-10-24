# Changelog

Amounts are `u64` sats and fee rates sat/vB unless stated.

## 0.7.0 (unreleased)

Bark stays at lib-0.8.0 (`959a489f`). Rebuild against the 0.7.0 bindings and
native binaries together: `BarkError`, `ExitProgressStatus`,
`EmergencyExitFeeEstimate` and `BroadcastOutcome` change shape.

### Typed emergency-exit errors

Exit calls (`estimateEmergencyExitFee`, `startExitForVtxos`,
`startExitForVtxosIncludingNonStandard`, `progressExits`) now classify Bark's
`ExitError` instead of returning `Other`:

| Kind | `BarkErrorCode` |
| --- | --- |
| Never exitable (do not retry) | `ExitNonStandardVtxo`, `ExitDustLimit`, `ExitVtxoAlreadySpent`, `ExitVtxoAlreadyExited`, `ExitVtxoSwept`, `ExitUnknownVtxo`, `ExitCyclicTransactions`, `ExitMissingAnchorOutput` |
| Chain source unavailable (retry later) | `ExitTransactionRetrievalFailure`, `ExitTipRetrievalFailure`, `ExitBlockRetrievalFailure`, `ExitAncestorRetrievalFailure` |
| Not enough confirmed onchain funds for CPFP | `ExitInsufficientConfirmedFunds` |

`BarkError` gains optional `vtxoId` (set when Bark names the VTXO),
`neededSats` and `availableSats` (set for `ExitInsufficientConfirmedFunds`).
Other exit failures stay `Other`. An estimate fee rate that overflows now
returns `InvalidFeeRate`.

**Migration:** `ExitProgressStatus.error` changes from `String?` to
`BarkError?`. Read `error?.message` for the old text and `error?.code` to
decide whether to retry. Exhaustive `switch`/`when` over `BarkErrorCode` needs
the new cases.

### Whole-wallet exit estimate skips VTXOs bark would not exit

`estimateEmergencyExitFee(vtxoIds: [])` now prices what bark's whole-wallet
exit would start: every unspent VTXO, spendable or locked (previously only
spendable), minus non-standard ones. One dust or non-standard VTXO no longer fails the
estimate; it is left out and listed in the new
`EmergencyExitFeeEstimate.excludedVtxos: [ExcludedExitVtxo]`
(`vtxoId`, `amountSats`, `code`, `message`). `code` is
`ExitNonStandardVtxo` (dust is a kind of non-standard), or `Other` when the
VTXO could not be loaded. Explicit ids are still priced as given and fail with
the typed code.

**Migration:** code that constructs `EmergencyExitFeeEstimate` passes
`excludedVtxos`. Show `excludedVtxos` next to an "exit all" estimate; those
funds need a refresh before they can be exited.

### `prepareBoard` excludes caller-chosen coins

`prepareBoard(amount:feeRateSatPerVb:confirmedOnly:excludedOutpoints:)` takes
an optional `excludedOutpoints: [OutPoint]?` (Kotlin `List<OutPoint>?`,
default `nil`/`null`). Coin selection never spends those outpoints; with the
rest of the wallet it fails with the usual `InsufficientFunds`. Malformed
txids and outpoints the wallet does not own are ignored, not errors.

**Migration:** none required; existing calls keep their behaviour. Replace
"discard and retry lower" loops with one call that passes the protected coins.

### Confirmed-only preparations are re-checked at submit

A board or onchain send prepared with `confirmedOnly: true` now requires every
parent transaction to still be confirmed at the chain source on first submit.
If a reorg returned a parent to the mempool, `submitPreparedBoard` and
`broadcastPrepared` throw `PreparedInputsUnavailable` and nothing is recorded
or broadcast. The preparation stays usable: submit again once the parent
re-confirms, or discard it. If the chain source is unreachable, that first
submit throws instead of recording the transaction and returning `failed`.
Retrying a send that was already recorded skips the check, so rebroadcast
keeps working.

**Migration:** none in the API; handle `PreparedInputsUnavailable` from submit
as "inputs changed, prepare again or wait".
