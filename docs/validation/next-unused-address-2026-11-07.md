# Next unused addresses (v0.9.0) — 2026-11-07

Task 30. Local runs on `1eb774d` against an isolated RegTest stack
(`COMPOSE_PROJECT_NAME=bark-bolt-ffi-nua`, `ARK_PORT=3835`, `BITCOIN_PORT=19643`,
bitcoind chain source).

- `core::onchain::tests::next_unused_address_waits_for_a_payment` (Esplora stub):
  a reveal left staged by a failed save is saved by the next call; repeated
  calls return the same address with nothing staged; a synced payment moves it
  to the next index; a reload from SQLite agrees; a prepared send's change lands
  on the shown address, after which the next call returns a new one.
- `BARK_REGTEST=1 cargo test --locked -- --test-threads=1` (on `f5a4dd4`, which
  differs only in the save condition): 101 unit, 1 exit-estimate, 2 payments,
  7 prepared-board and 3 prepared-send tests passed. `live_next_unused_addresses`:
  the onchain address stays put until a sync sees an unconfirmed bitcoind
  payment to it; the Ark address stays put until an arkoor receive on it is
  synced, then a higher index is returned; `newAddress()` still derives.
- `cargo test --locked --lib core::onchain` on `1eb774d`: 3 passed.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`: pass.
- `BARK_REGTEST=1 swift test --package-path tests/swift` against a fresh
  `just build-ios --overlay boltffi.local.toml` of `1eb774d`: 17 tests, 0 failures.
- `BARK_REGTEST=1 ./gradlew :sdk:jvm:test :sdk:jvm:testPackaged` against a fresh
  `just build-android --overlay boltffi.local.toml` of `1eb774d`: 16 JVM tests
  and 16 packaged tests, 0 failures, 0 skipped.

## Review

GPT-6.1 Sol (high), two rounds. Round 1 found S1 (P2: a retry after a failed
save returned an unsaved reveal) and S2 (P3: checklist claimed unrun Swift/Kotlin
tests). Both fixed in `1eb774d`; round 2 approved with no new findings.

## Hosted CI

Pushed to `main` as `9cb803e` (no PR, by decision). Main run
[42381295762](https://github.com/smolcars/bark-bolt-ffi/actions/runs/42381295762):
`rust`, `apple` (full five-slice Apple build, Swift contracts, iOS app tests) and
`android` (AAR/JVM build, API parity, JVM and packaged-JAR tests, 16 KB alignment,
emulator UI and app RegTest) passed.

Apple artifact 13577759118 (sha256 `90624399…a40e`) was staged with
[v090-stage-ci.py](v090-stage-ci.py); XCFramework sha256
`2ba7ed45d5fb6db2983dbe0039611c30f9ac9edb8a31e989c44abf998bfa0e35`. The generated
Swift gains only the two `nextUnusedAddress()` methods. The AAR and JVM JAR come
from Android artifact 13578358372 (sha256 `ce9bf9f2…8488`).

## Limits

"Used" is as of the last onchain sync and mailbox sync. The onchain wallet is
single-descriptor, so a send's change uses the shown address. The Ark address
may reuse a key Lightning, boarding, refresh or change took, and concurrent
first calls may each derive. The example apps don't call the new methods.
