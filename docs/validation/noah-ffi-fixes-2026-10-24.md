# Noah FFI fixes: execution evidence (v0.7.0)

Plan: [26 — Noah FFI fixes](../plans/26-noah-ffi-fixes.md). Bark lib-0.8.0
(`959a489f`) unchanged; BoltFFI 0.31.1; Rust 1.99.1.

Environment: the repository's existing `bark-bolt-ffi` Compose stack
(Ark 127.0.0.1:3635, Bitcoin 127.0.0.1:19443, captaind `nightly-2026-10-19`).
Its LND container had stopped tracking the chain (`synced_to_chain: false`
with a four-day-old header timestamp), which failed `wait-lightning-sync` in
`tests/payments.rs`; restarting only that container (volumes kept) fixed it.
Reorg tests use their own disposable bitcoind containers. Other agents'
`noah-bark08-*` and `scripts-*` stacks, simulators and emulators were not
touched. Hosted CI runs the same live tests against the pinned
`nightly-2026-10-22` images.

## Results (local, 2026-10-24)

- `BARK_REGTEST=1 cargo test --locked --no-fail-fast -- --test-threads=1`:
  91 unit, 1 exit-estimate, 1 payments, 7 prepared-board, 3 prepared-onchain
  tests passed.
- Red/green: with the item 4 source reverted, both confirmed-only reorg tests
  fail (the send returns `Accepted`; the board reaches the server).
- Kotlin `:sdk:jvm:test :sdk:jvm:testPackaged` with `BARK_REGTEST=1`: 15 tests,
  0 failures, 0 skipped (includes `V070ContractTest`).
- `swift test --package-path tests/swift` with `BARK_REGTEST=1`: 16 tests,
  0 failures.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `scripts/check-api.py` (165 exports, 62 records, 22 enums),
  `tests/python`, shellcheck: pass.

## Review

GPT-6 Astra (Codex, high effort, priority tier), read-only, two rounds.
Round 1 found three issues, all fixed before push: whole-wallet estimate
omitted locked VTXOs; confirmed-only check blocked rebroadcast of an evicted,
recorded send; the existing reservation test failed once its stopped chain
proxy made the new status query error. Round 2: no findings.

## Limits

Example apps were not rerun locally; the item changes are SDK-internal or
additive for the apps. Hosted CI and release publication are recorded
separately.
