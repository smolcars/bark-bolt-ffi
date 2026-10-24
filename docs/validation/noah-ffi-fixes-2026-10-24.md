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

## Apps

`just test-regtest --extended` on a task-created simulator (`Bark v070
iPhone`, iPhone 18 Pro, iOS 27.0) and AVD (`bark-v070`, API 35 arm64,
`emulator-5580`): `"passed": true`, 32 steps per platform, including the exit
estimate, start, progress and drain. The booted `iPhone 18 Pro` and
`Noah Link iPhone` simulators were not touched.

## Hosted CI

Main run [40132677901](https://github.com/smolcars/bark-bolt-ffi/actions/runs/40132677901)
at `6961e82` passed `rust`, `apple` (Swift contracts, iOS app tests) and
`android` (Kotlin contracts and packaged JAR, apps, live RegTest Rust tests
on the pinned `nightly-2026-10-22` images). Its Apple artifact 12575299367
(sha256 `c095818a…f0e8`) was staged with
[v070-stage-ci.py](v070-stage-ci.py); XCFramework sha256
`fe1b17f7308f8fa473e4e55040e4bbfb5bad2120b117947ace4ae5af649fa435`.

## Limits

Physical and minimum-OS devices were not run. Publication is recorded
separately.
