# Check VTXOs with the server after a restore (v0.10.0) — 2026-11-07

Task 31. Local runs on `ddb1fcf` against an isolated RegTest stack
(`COMPOSE_PROJECT_NAME=bark-bolt-ffi-tas`, `ARK_PORT=3935`, `BITCOIN_PORT=19743`,
bitcoind chain source), started new for this task. No existing stack or volume
was reset.

- `types::server_status_tests::every_adoption_keeps_its_outcome_and_in_flight_state`:
  Bark's `Spendable`, `Spent` and all three in-flight states convert to the
  matching variant; `Unspecified`, `Spendable` or `Spent` inside `InFlight` is an
  error.
- `BARK_REGTEST=1 cargo test --locked -- --test-threads=1`: 102 unit,
  1 exit-estimate, 3 payments, 7 prepared-board and 3 prepared-send tests passed.
  `live_trust_and_adopt_server_vtxo_status`: a received arkoor VTXO is
  `Spendable`; the recipient takes a `createSnapshot` backup, then spends the
  VTXO, after which it is `Spent` and stays spent. A wallet opened from the
  backup still has the VTXO spendable after `sync()`; the call returns `Spent`,
  the VTXO is then spent and the spendable balance drops by its 20,000 sats. A
  locked VTXO returns `None` and stays locked; another wallet's VTXO and an
  invalid id are errors. With the core method stubbed to always return `Spent`,
  the test failed at the first `Spendable` assertion.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `python3 scripts/swift-artifact.py` and `python3 -m unittest discover -s tests/python`
  (10 tests): pass.
- `BARK_REGTEST=1 swift test --package-path tests/swift` against a fresh
  `just build-ios --overlay boltffi.local.toml` of `ddb1fcf`: 17 tests,
  0 failures, 0 skipped. `testLiveNoahFeatureParity` checks `.spendable`, `nil`
  for a locked VTXO, `.spent` for the board VTXO the arkoor send spent, and
  errors for another wallet's VTXO and an invalid id.
- `BARK_REGTEST=1 ./gradlew :sdk:jvm:test :sdk:jvm:testPackaged` against a fresh
  `just build-android --overlay boltffi.local.toml` of `ddb1fcf`: 16 JVM tests
  and 16 packaged tests, 0 failures, 0 skipped. `liveFeatureParity` makes the
  same checks as Swift.
- `boltffi generate swift --deny-skipped` and `python3 scripts/check-api.py`:
  166 exports, 62 records, 24 enums, 2 callbacks in both hosts.

## Review

GPT-6.1 Sol (high), one round: approved `ddb1fcf` with no P1 or P2. S1 (P3:
the checklist linked this file before it existed) is fixed by adding it.

## Hosted CI

Pushed to `main` as `a84271e` (no PR, by decision). Main run
[42398073728](https://github.com/smolcars/bark-bolt-ffi/actions/runs/42398073728):
`rust`, `apple` (full five-slice Apple build, Swift contracts, iOS app tests) and
`android` (AAR/JVM build, API parity, JVM and packaged-JAR tests, 16 KB alignment,
emulator UI and app RegTest) passed. Attempt 1's `android` job failed in
`just setup-everything`, before any test, because LND was still starting
(`lncli`: "server is still in the process of starting"); rerunning that job
passed.

Apple artifact 13584034558 (sha256 `4b5abdd5…f2cc9`) was staged with
[v0100-stage-ci.py](v0100-stage-ci.py); XCFramework sha256
`bee0e937a0042957dda06909c97dd2646954c60dd5fcb9f4fc6dfecbdb21d1ac`. The generated
Swift gains only `VtxoInFlightState`, `ServerStatusAdoption` and
`trustAndAdoptServerVtxoStatus`. The AAR and JVM JAR come from Android artifact
13585201850 (sha256 `9e47c76f…4932`) of the passing attempt; their POMs differ
from v0.9.0's only in version.

## Limits

- No in-flight state was produced live; the unit test covers their conversion.
- Two upstream paths weren't run live: a VTXO spent here that the server
  reports spendable stays spent, and a `Spent` answer for an exited VTXO is an
  error.
- Marking a VTXO spent adds no movement, so history doesn't show it.
- Example apps were not changed; they don't call the new method.
