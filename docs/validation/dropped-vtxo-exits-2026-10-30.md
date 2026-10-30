# Dropped-VTXO exit reads: execution evidence (v0.7.1)

Plan: [task 26](../production-migration-todo.md). Bark lib-0.8.0 (`959a489f`)
unchanged; BoltFFI 0.31.1; Rust 1.99.1.

Environment: the repository's existing `bark-bolt-ffi` Compose stack
(Ark 127.0.0.1:3635, Bitcoin 127.0.0.1:19443, captaind `nightly-2026-10-22`).
Other agents' stacks, simulators and emulators were not touched.

## Results (local, 2026-10-30)

- `BARK_REGTEST=1 cargo test --locked -- --test-threads=1`: 92 unit,
  1 exit-estimate, 1 payments, 7 prepared-board, 3 prepared-onchain tests passed.
  `cargo test --locked --lib`: 92 passed.
- Red/green for `listing_exits_survives_a_dropped_vtxo`:
  - v0.7.0 source: fails listing exits with `The VTXO with id … cannot be found`.
  - Only the amount lookup skipping absent VTXOs: `list_all_exits(true, true)`
    still fails inside Bark with `failed to retrieve VTXO for exit …`.
  - Helper treating every read error as absent: fails its failed-read assertion.
  - Shared helper as released: passes.
- `swift test` with `BARK_REGTEST=1` against the staged CI XCFramework: 16 tests,
  0 failures.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `tests/python`, shellcheck and `tests/shell/ark-dev.sh`: pass.

## `getExitStatus` callers

Noah's onchain journal keeps VTXO IDs across launches and re-checks them with
`getExitStatus` (iOS `WalletSession+ExitProgress.swift`, Android
`OnchainOperations.kt`), and its Debug screen can drop a VTXO, so a dropped ID
can reach this call. The other callers take IDs from the exit lists. Returning
`nil` leaves that journal attempt unresolved instead of failing every check.

## Review

GPT-6.1 Sol (Codex, high effort), read-only, one round: approved, no findings.

## Hosted CI

Main run [41139900213](https://github.com/smolcars/bark-bolt-ffi/actions/runs/41139900213)
at `be7aeb1` passed `rust`, `apple` (Swift contracts, iOS app tests) and
`android` (Kotlin contracts and packaged JAR, Android app RegTest, live RegTest
Rust tests including the regression test). Its Apple artifact 13018578027
(sha256 `5ed5533b…7e65`) was staged with [v071-stage-ci.py](v071-stage-ci.py);
XCFramework sha256
`fd411e82ccb43c1cf2fcb9b0feb3bc7cb2cc0c8cc0ff4518165c6a2ebe70ede8`. The generated
Swift sources are byte-identical to 0.7.0.

## Limits

An exit row whose VTXO is gone stays in the database, hidden from both reads; a
finished exit in that state also disappears from the list. The example apps were
not rerun locally, and the iOS example has no live RegTest run for this release.
Physical and minimum-OS devices were not run. Publication is recorded separately.
