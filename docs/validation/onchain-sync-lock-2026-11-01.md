# Onchain reads no longer wait for a sync: execution evidence (v0.7.3)

Plan: [task 28](../production-migration-todo.md). Bark lib-0.8.0 (`959a489f`)
unchanged; BoltFFI 0.31.1; Rust 1.99.1.

Environment: the repository's existing `bark-bolt-ffi` Compose stack
(Ark 127.0.0.1:3635, Bitcoin 127.0.0.1:19443, captaind `nightly-2026-10-22`).
Other agents' stacks, simulators and emulators were not touched.

## Results (local, 2026-11-01)

- `cargo test --locked`: 95 unit (including `reads_do_not_wait_for_a_sync_fetch`
  and `scan_overlapped_by_a_bark_sync_keeps_old_payments_confirmed`), 1
  exit-estimate, 1 payments, 7 prepared-board, 3 prepared-onchain tests passed.
- `BARK_REGTEST=1 cargo test --locked -- --test-threads=1`: the same 95, 1, 1,
  7 and 3 tests passed live.
- `swift test` with `BARK_REGTEST=1` against the staged CI XCFramework: 16 tests,
  0 failures.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `tests/python`, shellcheck and `tests/shell/ark-dev.sh`: pass.

## Review

The change (#7, merged as `30be335`) was reviewed in two rounds by GPT-6.1 Sol
on high and approved.

## Hosted CI

Main run [41279004359](https://github.com/smolcars/bark-bolt-ffi/actions/runs/41279004359)
at `69f89ee` passed `rust`, `apple` (Swift contracts, iOS app tests) and
`android` (Kotlin contracts and packaged JAR, Android app RegTest, live RegTest
Rust tests). Its first `android` attempt failed the bitcoind-backed live test
`live_prepared_board_never_spends_excluded_outpoints`, which this change doesn't
touch; the rerun passed. Its Apple artifact 13072049751 (sha256 `30a1591d…0747`)
was staged with [v073-stage-ci.py](v073-stage-ci.py); XCFramework sha256
`8a26e489576881762b2567a6641f04421cef20522a5998137a1104e881694c84`. The generated
Swift sources and the AAR's `classes.jar` are byte-identical to 0.7.2. The AAR
from Android artifact 13081387969 (sha256 `ca7c6aad…f3c4`) has 0x4000 LOAD
alignment on all three ABIs.

## Limits

The live RegTest tests use a bitcoind chain source; the Esplora path is covered
by the unit tests against a local stub Esplora server. The example apps were not
rerun locally, and the iOS example has no live RegTest run for this release.
Physical and minimum-OS devices were not run. Publication is recorded separately.
