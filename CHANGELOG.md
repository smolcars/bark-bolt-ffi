# Changelog

Amounts are `u64` sats and fee rates sat/vB unless stated.
Names use the Rust spelling: Swift lowercases the first letter
(`BarkErrorCode.exitNonStandardVtxo`, `.uncertain`) and Kotlin uses
SCREAMING_SNAKE enum constants (`BarkErrorCode.EXIT_NON_STANDARD_VTXO`) and
class names for variants (`BroadcastOutcome.Uncertain`).

## 0.7.3

Bark stays at lib-0.8.0 (`959a489f`). No API change; databases open unchanged.

### Onchain reads no longer wait for a sync

On Esplora, `OnchainWallet.sync` and `initialScan` now fetch without holding
the wallet lock and take it only to apply and save the result. `balance`,
`transactions`, `utxos` and other calls no longer wait seconds for a sync's
network round trip. Syncs and scans still run one at a time, and the result
matches what Bark's own sync produces. `balance` now takes the read lock.
Bitcoind syncs still hold the lock, as does Bark's daemon (`runDaemon`)
onchain sync.

## 0.7.2

Bark stays at lib-0.8.0 (`959a489f`). No API change; databases open unchanged.

### Quieter transport logs

`setLogger` no longer forwards Debug and Trace records from the HTTP and gRPC
transport crates (`h2`, `hyper`, `hyper_util`, `reqwest`, `rustls`, `tonic`,
`tower` and a few more). They logged every request and pooled connection.
Their Info and above still arrive, and Bark's own records are unchanged.

## 0.7.1

Bark stays at lib-0.8.0 (`959a489f`). No API change; databases open unchanged.

### Exit reads survive a dropped VTXO

`dangerousDropVtxo` leaves the VTXO's exit row behind, and Bark can neither
load, progress nor cancel that row. One such row made `listAllExits` fail for
the whole wallet and `getExitStatus` fail for that ID. `listAllExits` now skips
the row, with or without transactions, and `getExitStatus` returns `nil`. A
failed database read still throws, so it can't hide a live exit.

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

### Uncertain board outcome

Bark writes the BOARD movement before its checkpoint, so `submitPreparedBoard`
can fail after Bark has partly recorded the board. When Bark returns an error,
the SDK now checks for a pending board or a movement whose `chain_anchor` is
this funding transaction. If it finds one, or the check itself fails, the
result is the new `BroadcastOutcome.uncertain(reason:)`
(Kotlin `BroadcastOutcome.Uncertain`) and the preparation is consumed: later
submits throw `PreparedInputsUnavailable`, and `discardPreparedBoard` still
releases it. Without a record, the outcome stays `notAttempted` and a retry
is safe.

**Migration:** handle the new case in exhaustive `switch`/`when` over
`BroadcastOutcome`. On `uncertain`, do not resubmit. Sync, inspect
`pendingBoards()` and history for the funding txid, then discard the
preparation and prepare again only if nothing was boarded.
