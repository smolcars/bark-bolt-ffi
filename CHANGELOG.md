# Changelog

Amounts are `u64` sats and fee rates sat/vB unless stated.
Names use the Rust spelling: Swift lowercases the first letter
(`BarkErrorCode.exitNonStandardVtxo`, `.uncertain`) and Kotlin uses
SCREAMING_SNAKE enum constants (`BarkErrorCode.EXIT_NON_STANDARD_VTXO`) and
class names for variants (`BroadcastOutcome.Uncertain`).

## 0.11.0

Bark moves to lib-0.8.1 (`30368206`). Adds three `Wallet` methods, ten
records and one enum. The ABI of existing types is unchanged; fiat-priced
BOLT12 offers now parse instead of failing (below). Databases stay
compatible: Bark 0.8.1 has no wallet migration. RegTest now runs the 0.8.1
server, which needs CLN v26.06 or later.

### Presigned emergency exits

- `Wallet::presign_emergency_exit(vtxo_ids, fee_rate_sat_per_vb?) -> PresignedExits`
  signs a CPFP child for every exit transaction that still needs broadcasting,
  for as long as confirmed onchain funds last. It broadcasts nothing and
  reserves nothing. `PresignedExits.waves` is the broadcast plan: broadcast
  all of a wave's `(exit, child)` packages together, then wait until they and
  the wave's `in_mempool` transactions confirm before the next wave.
  `PresignedExits.exits` has one entry per VTXO, with
  `PresignedExitStatus::FullySigned { exited_in_wave, total_fee_sats, fee_rate_sat_per_vb }`,
  `PartiallySigned { last_wave }` or `Unsigned`, and the unfunded tail in
  `unsigned`. A package shared by several VTXOs appears once, under the first.
  Omit the fee rate for Bark's default exit fee rate.
- The onchain wallet reveals the change addresses the children pay to, so one
  sync finds the whole chain once it is broadcast. A later spend from the
  onchain wallet can invalidate the remaining children.
- `Wallet::remaining_exit_transactions(vtxo_ids) -> RemainingExitTransactions`
  lists the exit transactions not yet confirmed: per VTXO (`RemainingExit`,
  in request order) the txids in broadcast order, and each transaction once in
  `transactions`, parents before children. It signs nothing and doesn't touch
  the onchain wallet. Statuses are from the last exit sync; call `sync_exits`
  first for current ones.
- Both error with `ExitUnknownVtxo`, `ExitVtxoAlreadyExited` or
  `ExitVtxoAlreadySpent`. Presigning needs the built-in onchain wallet: a
  wallet without one, or with a callback onchain wallet, gets an error.

### Lightning invoice expiry

- `Wallet::bolt11_invoice_with(amount_sats, Bolt11InvoiceOptions)` takes
  `description`, `expiry_secs`, `token` and `claim_destination`, through Bark's
  `Bolt11InvoiceBuilder`. The expiry defaults to, and cannot exceed, the
  server's invoice expiry; a server that ignores it makes the call fail.
  `bolt11_invoice` and `bolt11_invoice_for_address` are unchanged.

### Fixes from Bark 0.8.1

- Lightning sends that stayed pending after being paid now finish from the
  stored preimage.
- `parse_payment_request` now accepts a BOLT12 offer priced in a fiat
  currency, with `amount_sats` unset. It used to fail with
  `InvalidPaymentRequest`.
- The client no longer advertises zstd compression to the server.

### bdk_wallet 3.2

Bark pins bdk_wallet 3.2.0, whose reopened wallet holds no private keys.
Prepared sends and prepared boards signed with it, so on a reopened wallet
they would have failed with "not fully signed". They now sign through Bark's
own signers. Fee, inputs and outputs are unchanged.

## 0.10.0

Bark stays at lib-0.8.0 (`959a489f`). Adds one `Wallet` method and two enums;
existing calls, the ABI of existing types and databases are unchanged.

### Checking VTXOs with the server after a restore

A restored backup can be older than the wallet's real state. It can still list
VTXOs as spendable that were spent after the backup was taken, and `sync()`
never asks the server about VTXOs the wallet thinks are spendable.

- `Wallet::trust_and_adopt_server_vtxo_status(vtxo_id) -> Option<ServerStatusAdoption>`
  exposes Bark's method of the same name. It asks the server for the VTXO's
  spend state and moves the wallet's VTXO along, only ever toward spent:
  - `Spent`: the wallet now has the VTXO as spent.
  - `Spendable`: nothing changes. A VTXO the wallet already has as spent stays
    spent.
  - `InFlight { state }`: the server is waiting on a preimage or on the VTXO's
    transaction chain. `VtxoInFlightState` is `Unclaimed`, `Unregistered` or
    `HtlcRecvUnclaimed`. Nothing changes.
  - `None`: the VTXO is locked. It isn't checked and the server isn't asked;
    unlock it first to check it.

  It errors for an invalid id, a VTXO that isn't in this wallet or has no key
  here, an unreachable server, or a server answer with no usable state.

Limits:

- It trusts the server. A dishonest server can make the wallet stop using a
  VTXO it still owns.
- Marking a VTXO spent adds no movement, so history doesn't show the spend.
  Balance does.
- Bark doesn't move an exited VTXO to spent, so a `Spent` answer for one is an
  error.
- One server request per VTXO. It doesn't add VTXOs the backup is missing.

## 0.9.0

Bark stays at lib-0.8.0 (`959a489f`). Adds one `Wallet` and one `OnchainWallet`
method; existing calls, the ABI of existing types and databases are unchanged.

### Next unused addresses

Showing a receive address no longer has to create one. `new_address()` stays
as it was and always derives a new address.

- `OnchainWallet::next_unused_address() -> String` returns the lowest revealed
  address that no wallet transaction pays, unconfirmed ones included. It reveals
  and saves a new address only when every revealed one has been paid, so syncs
  stop growing with each Receive visit. A repeated call reads memory only.
  Callback-backed onchain wallets return an error, like `new_address()`.
- `Wallet::next_unused_address() -> AddressWithIndex` returns the address of the
  newest stored VTXO key while no movement has received on it, otherwise a new
  address. A repeated call does a few indexed reads and no write.

Limits:

- "Used" is as of the last onchain `sync()` and the last mailbox sync. A
  payment the wallet hasn't seen yet doesn't count, so the same address is
  shown again until it has.
- Bark's onchain wallet has a single descriptor, so a send puts its change on
  the lowest unused address, which is the one this method shows. The funds stay
  in the wallet; the next call returns a new address.
- Lightning receives, boards, refreshes and change take keys from the same
  sequence as Ark addresses. The Ark address can therefore reuse such a key.
- After restoring an older backup, run `initialScan()` and `sync()` before
  showing addresses, so payments made after the backup are seen.

## 0.8.0

Bark stays at lib-0.8.0 (`959a489f`). Adds two `Wallet` methods; existing calls,
the ABI of existing types and databases are unchanged.

### Live mailbox processor

`startMailboxProcessor()` keeps the wallet subscribed to its Ark server mailbox
and applies messages as they arrive. A wallet opened with `runDaemon: false`
now adds an incoming arkoor payment, and emits its `bark.arkoor` `receive`
movement on `notifications()`, as soon as the payment lands. Before, that
waited for the next `sync()`. On RegTest the movement arrives about 30 ms after
the sender's `sendArkoorPayment` returns.

- For each new mailbox message it runs Bark's `sync_mailbox`, the mailbox step
  of `sync()`. That handles arkoor receives, Lightning receive notices (the
  payment is claimed), Lightning send results and the completion of this
  wallet's own delegated round participations. It never joins a round.
- It is safe next to `sync()`, maintenance and payment calls. Bark serializes
  the receive check and only moves the mailbox checkpoint forward, so each
  payment is recorded once.
- After the stream drops, or a message fails, it resubscribes after 1 s,
  backing off to 30 s while that keeps happening, and resumes from the stored
  checkpoint.
- Calling `startMailboxProcessor()` again while it runs does nothing. While
  Bark's daemon runs (`runDaemon: true` or `runDaemon()`) it does nothing either,
  because the daemon processes the mailbox itself; `runDaemon()` stops a
  running processor first.
- `stopMailboxProcessor()` returns once it has stopped. Connecting and
  waiting for messages are interrupted at once; a mailbox sync in progress
  finishes first, because Bark can't resume a message cut short after storing
  its VTXOs. Closing or dropping the wallet stops it the same way without
  waiting. Start, stop and the daemon calls run one at a time, and the
  processor can't start until a stopped daemon has ended, so there is never
  more than one of them.
- Keep calling `sync()`: it still does everything else (fees, pending rounds,
  boards, offboards, Lightning sends and exits).

Its own log records use the `[MAILBOX]` prefix and cover start, stop and
resubscribe reasons, never message contents, keys or addresses.

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
