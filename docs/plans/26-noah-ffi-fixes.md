# Noah FFI fixes (v0.7.0)

Five FFI-side changes Noah's native apps need. Bark stays at lib-0.8.0
(`959a489f`); nothing here changes upstream wallet logic or persistence.

## Engineering plan

1. **Typed exit errors.** `src/error.rs`: `From<anyhow::Error>` downcasts
   `bark::exit::ExitError` (found beneath `.context`) through `exit_error`, which
   maps variants to new `Exit*` `BarkErrorCode`s. `BarkError` gains optional
   `vtxo_id`, `needed_sats`, `available_sats`. `ExitProgressStatus.error`
   becomes `BarkError?` through the same mapping (breaking). The estimate's own
   fee-rate overflow check returns `InvalidFeeRate`.
2. **"All" exit estimate eligibility.** `src/core/wallet.rs`: empty `vtxo_ids`
   applies `Vtxo<Full>::check_standard`, the filter bark's
   `start_exit_for_entire_wallet` uses, and reports skipped VTXOs in
   `EmergencyExitFeeEstimate.excluded_vtxos` (breaking record change).
3. **`prepareBoard(excludedOutpoints)`.** Parsed outpoints are merged into the
   `unspendable` list BDK already receives; unparseable or unknown entries are
   ignored.
4. **Confirmed-only at submit.** `Prepared` keeps `confirmed_only`. On first
   submit of a board or prepared send, every parent must be
   `TxStatus::Confirmed` at the chain source, else `PreparedInputsUnavailable`.
5. **Uncertain board outcome.** After a `board_psbt` error, look for a pending
   board or a movement whose `chain_anchor` is this funding txid. If one exists
   (or the lookup fails), return `BroadcastOutcome.Uncertain` and refuse
   resubmission; discard still releases the preparation.

Invariants: amounts are `u64` sats, fee rates sat/vB; no secrets logged;
generated bindings are never hand-edited; `api-baseline.json` tracks new fields.

Validation: Rust unit tests (`cargo test`), live RegTest Rust tests
(`BARK_REGTEST=1 cargo test -- --test-threads=1`), Swift/Kotlin contract tests,
`scripts/check-api.py`, CI.
