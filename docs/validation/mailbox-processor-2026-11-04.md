# Live mailbox processor: execution evidence (v0.8.0)

Plan: [task 29](../production-migration-todo.md). Bark lib-0.8.0 (`959a489f`)
unchanged; BoltFFI 0.31.1; Rust 1.99.1.

Environment: the repository's existing `bark-bolt-ffi` Compose stack
(Ark 127.0.0.1:3635, Bitcoin 127.0.0.1:19443, captaind `nightly-2026-10-22`).
Other agents' stacks, simulators and emulators were not touched.

## Feasibility (Bark at `959a489f`, paths under `bark/src`)

- `subscribe_process_mailbox_messages` (`mailbox.rs:183`) resumes from the
  stored checkpoint and checks its token between messages and while backing off
  (`mailbox.rs:271`, `:288`). Each message goes through `process_mailbox_message`
  (`mailbox.rs:405`), the handler `sync()` reaches through `sync_mailbox`
  (`lib.rs:2026`, `mailbox.rs:318`).
- It handles arkoor receives (`bark.arkoor`/`receive` movement, dispatched as
  `MovementCreated`), Lightning receive notices (claimed with `wait = false`),
  Lightning send results and delegated round completions (`mailbox.rs:419-455`).
  The round branch finishes this wallet's own delegated participations; it
  skips states with an ongoing attempt (`round/mod.rs:2465`) and never joins a
  round. Interactive joins only happen in the daemon's round-event task
  (`daemon/mod.rs:157`).
- Concurrency: the arkoor check-then-store holds `mailbox.processing`
  (`mailbox.rs:56`, `:486`), the checkpoint only moves forward
  (`persist/sqlite/query.rs:833`), and Lightning actions use a per-action
  `try_lock` that skips when another caller drives the action
  (`actions/mod.rs:544`). Bark's daemon runs the same function next to its sync.
- `subscribe_process_mailbox_messages_weak` is crate-private, so the task holds a
  strong wallet reference and the wallet's `Drop` cancels it.

## Design after review

Review (GPT-6.1 Sol, high) found that Bark's `subscribe_process_mailbox_messages`
doesn't check its token while connecting (the subscribe request deadline is an
hour), and that dropping it mid-message can lose a receive movement: Bark stores
the VTXOs, then awaits the recovery-mailbox post, then records the movement
(`vtxo/mod.rs:310`, `mailbox.rs:521`), and skips known VTXOs on redelivery
(`mailbox.rs:499`). So the processor uses Bark's public pieces instead:
`subscribe_mailbox_messages` (`mailbox.rs:142`) as the wake-up stream, whose
connect and wait are cancelled freely, and `sync_mailbox` (`mailbox.rs:297`), the
mailbox step of `sync()`, for each new message, which always runs to completion.
A message `sync()` already applied is skipped by checkpoint; one that failed is
retried by resubscribing after the backoff. Start, stop and daemon start/stop
hold one async lock until they finish, and a non-waiting `stopDaemon()` keeps it
until Bark's daemon has ended.

## Results (local, 2026-11-04, macOS ARM64)

Final code is `caeb918`; earlier numbers from the first design are kept where
noted.

- No-sync arrival, measured from `sendArkoorPayment` returning to the
  `bark.arkoor` receive on the recipient's `notifications()` (recipient opened
  with `run_daemon: false`, never synced first):
  - Rust `live_arkoor_receive_without_sync` on `caeb918`: 30, 33, 29 ms on three
    runs (117-119 ms from the start of the send). A payment sent while the
    processor was stopped stayed in the mailbox for 3 s, and a restart picked
    it up in 61-74 ms. Two payments sent while `sync()` ran concurrently were
    recorded once each (4 receive movements, 24,000 sats). Dropping the wallet
    while the processor ran released every reference. The first design measured
    13-32 ms.
  - Swift `testLiveMailboxProcessorReceivesWithoutSync` on `caeb918`: 31 ms
    (32 ms on the first design).
  - Kotlin `MailboxProcessorContractTest` on `caeb918`: 30 ms
    (`testPackaged`; 36 and 28 ms on the first design).
- `lifecycle_without_server` (no RegTest, local Esplora stub, unreachable Ark
  server): start twice spawns one task; the task survives failed subscriptions;
  stop waits and restart spawns anew; `runDaemon` stops it and blocks starts
  until `stopDaemonWait`; drop releases the wallet.
- Review fixes, without RegTest:
  `stop_holds_the_lifecycle_until_the_task_ends` (a concurrent stop returns
  only after a slow-stopping task ended, and a concurrent start doesn't overlap
  it; fails with the original take-then-join `stop`);
  `daemon_start_and_stop_are_serialized` (a concurrent daemon start and stop
  apply in order, and a start waits for a daemon stopped without waiting to
  end; fails when that stop clears the flag at once);
  `stop_and_close_interrupt_a_stalled_subscription` (against a server that
  accepts the connection and never answers, stop returns in under 1 s and
  dropping the wallet releases it in under 1 s).
- `BARK_REGTEST=1 cargo test --locked -- --test-threads=1` on `caeb918`: 100
  unit, 1 exit-estimate, 1 payments and 6 of 7 prepared-board tests passed;
  `live_prepared_board_never_spends_excluded_outpoints` failed its
  largest-first coin-selection assertion (the board picked the smaller coin),
  as it did on the first design's full run and once on the v0.7.3 main run.
  Rerun with the 3 prepared-onchain tests: 7 and 3 passed. This change doesn't
  touch boarding or coin selection.
- `BARK_REGTEST=1 swift test --package-path tests/swift` against a fresh
  `just build-ios --overlay boltffi.local.toml` of `caeb918`: 17 tests, 0
  failures (also 17/17 on the first design and on `c69750a`).
- `BARK_REGTEST=1 ./gradlew :sdk:jvm:test :sdk:jvm:testPackaged
  :sdk:android:assembleRelease :examples:android:assembleDebug` against a fresh
  `just build-android --overlay boltffi.local.toml` of `caeb918`: 16 JVM tests
  and 16 packaged tests, 0 failures, 0 skipped; AAR and example APK built.
- `boltffi generate swift --deny-skipped` and `scripts/check-api.py`: 165
  exports, 62 records, 22 enums, 2 callbacks in both hosts. The generated Swift
  and Kotlin carry `startMailboxProcessor()`/`stopMailboxProcessor()` as
  `async throws`/`suspend`, with their doc comments.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `tests/python` (10 tests), shellcheck, `tests/shell/ark-dev.sh` and
  `scripts/swift-artifact.py`: pass.

## Review

GPT-6.1 Sol on high reviewed three rounds: round 1 found S1-S3 (concurrent
stop/start, daemon flag races, connect not cancellable), round 2 found S4
(forced drop could lose a receive movement) and S2 still open for the
non-waiting daemon stop, and round 3 approved `caeb918` with all four resolved.

## Hosted CI

PR [#10](https://github.com/smolcars/bark-bolt-ffi/pull/10) passed `rust`,
`apple` and `android` at `a061c50` (the first `android` attempt failed the
flaky test below; the rerun passed) and merged as `bb14e6a`.

Main run [41887941694](https://github.com/smolcars/bark-bolt-ffi/actions/runs/41887941694)
at `bb14e6a`: `rust` and `apple` (full five-slice Apple build, Swift contracts,
iOS app tests) passed. `android` built the release AAR and JVM JAR, passed the
API parity check, the Gradle build with the JVM and packaged-JAR contract tests,
the 16 KB alignment check and the Kotlin artifact check, then failed both
attempts at `live_prepared_board_never_spends_excluded_outpoints`, so the
emulator step (Android UI tests, live Kotlin tests, Android app RegTest) did not
run for this commit. That test assumes BDK picks the larger of two coins, but
BDK's default selection falls back to a random draw; it failed the same way on
v0.7.3's main run and, run alone locally, passed 5 of 14 times before #10 and
6 of 10 after. The release uses these binaries by decision.

Apple artifact 13353315334 (sha256 `6b7bd38a…41c8`) was staged with
[v080-stage-ci.py](v080-stage-ci.py); XCFramework sha256
`bcadced292d63df92cad7f5e02ba6ce2037b2c0304edb1e680fe7e48559d26c2`. The
generated Swift gains only `startMailboxProcessor()` and `stopMailboxProcessor()`.
The AAR and JVM JAR come from Android artifact 13435622595 (sha256
`7326cbe7…25cf2`); the AAR has 0x4000 LOAD alignment on all three ABIs and both
carry the two new suspend methods.

## Limits

The example apps were not changed or rerun. Noah does not call the processor
yet. A stop waits for a mailbox sync in progress, which, like `sync()`, waits
on the server. Each new message costs one extra mailbox read. Physical devices
were not run. Hosted CI, review and publication are recorded separately.
