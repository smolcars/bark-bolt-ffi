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
