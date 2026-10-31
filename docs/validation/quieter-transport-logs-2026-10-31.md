# Quieter transport logs: execution evidence (v0.7.2)

Plan: [task 27](../production-migration-todo.md). Bark lib-0.8.0 (`959a489f`)
unchanged; BoltFFI 0.31.1; Rust 1.99.1.

Environment: the repository's existing `bark-bolt-ffi` Compose stack
(Ark 127.0.0.1:3635, Bitcoin 127.0.0.1:19443, captaind `nightly-2026-10-22`).
Other agents' stacks, simulators and emulators were not touched.

## Results (local, 2026-10-31)

- `cargo test --locked`: 93 unit (including
  `transport_crates_forward_only_info_and_above`), 1 exit-estimate, 1 payments,
  7 prepared-board, 3 prepared-onchain tests passed.
- `BARK_REGTEST=1 cargo test --locked -- --test-threads=1`: the same 93, 1, 1,
  7 and 3 tests passed live.
- `swift test` with `BARK_REGTEST=1` against the staged CI XCFramework: 16 tests,
  0 failures.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `tests/python`, shellcheck and `tests/shell/ark-dev.sh`: pass.

## Review

The change (`7bd2527`) was pushed after `cargo fmt --check` and the logger
tests. No separate code review was run.

## Hosted CI

Main run [41250921361](https://github.com/smolcars/bark-bolt-ffi/actions/runs/41250921361)
at `b40244a` passed `rust`, `apple` (Swift contracts, iOS app tests) and
`android` (Kotlin contracts and packaged JAR, Android app RegTest, live RegTest
Rust tests). Its Apple artifact 13061078515 (sha256 `518e777a…cb4b`) was staged
with [v072-stage-ci.py](v072-stage-ci.py); XCFramework sha256
`3ebde88fa92b1a8d900c67dacb6383844f75b7356cb5c097906697326b1b50c6`. The generated
Swift sources are byte-identical to 0.7.1. The AAR from Android artifact
13062560060 (sha256 `590ec674…6374`) has 0x4000 LOAD alignment on all three ABIs.

## Limits

The filter matches the first path segment of a record's target against a fixed
crate list; a transport crate not on the list still forwards at the requested
level. The example apps were not rerun locally, and the iOS example has no live
RegTest run for this release. Physical and minimum-OS devices were not run.
Publication is recorded separately.
