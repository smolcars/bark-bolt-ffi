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

## Results (local, 2026-11-04, macOS ARM64)

- No-sync arrival, measured from `sendArkoorPayment` returning to the
  `bark.arkoor` receive on the recipient's `notifications()` (recipient opened
  with `run_daemon: false`, never synced first):
  - Rust `live_arkoor_receive_without_sync`: 13, 27, 32 and 27 ms on four runs
    (108-116 ms from the start of the send). A payment sent while the processor
    was stopped stayed in the mailbox for 3 s, and a restart picked it up in
    54-74 ms. Two payments sent while `sync()` ran concurrently were recorded
    once each (4 receive movements, 24,000 sats). Dropping the wallet while the
    processor ran released every reference within the 10 s limit.
  - Swift `testLiveMailboxProcessorReceivesWithoutSync`: 32 ms.
  - Kotlin `MailboxProcessorContractTest`: 36 ms (`:sdk:jvm:test`) and 28 ms
    (`testPackaged`).
- `lifecycle_without_server` (no RegTest, local Esplora stub, unreachable Ark
  server): start twice spawns one task; the task survives failed subscriptions;
  stop waits and restart spawns anew; `runDaemon` stops it and blocks starts
  until `stopDaemonWait`; drop releases the wallet.
- `BARK_REGTEST=1 cargo test --locked -- --test-threads=1`: 97 unit, 1
  exit-estimate, 1 payments and 6 of 7 prepared-board tests passed;
  `live_prepared_board_never_spends_excluded_outpoints` failed its
  largest-first coin-selection assertion (the board picked the 60k coin), the
  same test that failed once on the v0.7.3 main run. Rerun with the 3
  prepared-onchain tests: 7 and 3 passed. This change doesn't touch boarding.
- `BARK_REGTEST=1 swift test --package-path tests/swift` against a fresh
  `just build-ios --overlay boltffi.local.toml`: 17 tests, 0 failures.
- `BARK_REGTEST=1 ./gradlew :sdk:jvm:test :sdk:jvm:testPackaged
  :sdk:android:assembleRelease :examples:android:assembleDebug` against a fresh
  `just build-android --overlay boltffi.local.toml`: 16 JVM tests and 16
  packaged tests, 0 failures, 0 skipped; AAR and example APK built.
- `boltffi generate swift --deny-skipped` and `scripts/check-api.py`: 165
  exports, 62 records, 22 enums, 2 callbacks in both hosts. The generated Swift
  and Kotlin carry `startMailboxProcessor()`/`stopMailboxProcessor()` as
  `async throws`/`suspend`, with their doc comments.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `tests/python` (10 tests), shellcheck, `tests/shell/ark-dev.sh` and
  `scripts/swift-artifact.py`: pass.

## Limits

The example apps were not changed or rerun. Noah does not call the processor
yet. `stopMailboxProcessor()` waits for a message in flight and, while the
subscription is still connecting to an unresponsive server, for that attempt to
fail; Bark doesn't check the token during connect. Physical devices were not
run. Hosted CI, review and publication are recorded separately.
