# Production migration checklist

Status: task 22 moves to Bark 0.8.0 and adds Lightning send options,
payment-request parsing, BIP 321 receive URIs and movement ids (v0.6.0).
Original tasks 00–20 are complete. Noah follow-up 21 ships its supported
scope in [v0.5.0](https://github.com/smolcars/bark-bolt-ffi/releases/tag/v0.5.0):
exact-fee prepared boarding, shared onchain reservations, direct chain transaction
status and persisted exit history. Prepared offboards and atomic cross-operation
keys remain upstream dependencies; see the unchecked requirements below.
The full **main** Rust/Apple/Android matrix, live Rust/Swift/Kotlin tests and
GPT-6 Astra high source review passed. [Execution evidence](validation/prepared-boarding-2026-10-20.md).
v0.4.0 introduced prepared onchain sends; v0.3.1 gated 16 KB Android alignment.
All nine fresh assets and all 12 live public Swift consumer contracts passed.
Noah was notified. [Release verification](validation/v050-release.json) and
[cleanup](validation/v050-cleanup.json) preserve the final evidence.

## Agreed decisions

- GitHub repository: https://github.com/smolcars/bark-bolt-ffi (now public after
  an external transfer from the initially private niteshbalusu11 repository);
  local checkout: bark-bolt-ffi.
- One repository for the Rust FFI core, Swift SDK, Kotlin Android/JVM SDKs,
  native example apps, RegTest infrastructure and GitHub Actions.
- Consumer languages: Swift and Kotlin only. Use SwiftUI and Jetpack Compose.
  Breaking SDK API changes are allowed; preserve existing wallet persistence compatibility.
- Targets: iOS 16+, macOS 13+, Android API 24+, JVM 17. Retain the existing
  native ABI matrix unless a documented platform decision changes it.
- Pin Bark master bfbe6b7c3088a09dd4e81c4ac24cdc763a627a8c and BoltFFI 0.31.1.
  Matching October 19 Bark/captaind user images are verified and digest-pinned.
- Preserve the complete API: callbacks, logging, notifications, Lightning,
  recovery, boarding, refresh, offboarding, exits and onchain operations.
- Actually run both apps on simulator/emulator against RegTest, and create
  GitHub Actions that build SDKs/apps and execute automated checks.

## Rule for every task

Before implementation, refine its engineering plan with exact affected paths,
API/ownership/concurrency decisions, invariants and validation commands. Implement,
run acceptance checks and record evidence before checking it off. If the scope
changes, revise the plan before the next code change. Routine tasks do not need
another approval pause.

Each task's completion record must contain:

```text
Engineering plan and affected paths:
Implemented:
Validation commands and results:
Artifacts / device or CI run evidence:
Remaining limits:
```

Compilation, simulator execution, RegTest success and hosted CI success are
different claims. Record them separately; do not mark one from evidence of another.

## Reference inputs

- Core source: ark-bitcoin/bark-ffi at
  d3c0f38f29cca1e79f6c81492725acece1f5b8ef. Upstream history and MIT license
  are retained in this checkout.
- Companion packaging reference: ark-bitcoin/bark-ffi-bindings at
  157b0fcda0a0a5782ad2904ed3002a7080293047. Import only needed Swift/Kotlin
  code and shared tooling.
- Nitro Ark exists at
  /Users/niteshchowdharybalusu/Documents/react-native-nitro-ark, local commit
  19b047e16b09bbeabac6ac1bd284e6bb14f4f76f. Inspected remote master:
  988b4f54fb642a42951e685247085a9aff90c4fc.
- Its scripts/docker-compose.yml, scripts/ark-dev.sh and justfile describe
  Bitcoin Core, Postgres, CLN, LND, an Ark server and a Bark CLI. Reuse the
  infrastructure approach, not its React Native binding architecture.
- Nitro uses September nightly server/CLI images and a different Bark revision;
  Compose references a missing captaind.toml. Its bootstrap resets a fixed
  scripts_bark volume. Supply compatible configuration, pin versions, and use
  isolated project names/ports/volumes instead of that reset path.
- Docker/OrbStack, Xcode, Android SDK/NDK and both booted phone runtimes are
  installed. The API 35 ARM64 emulator was provisioned during task09.
- The feasibility prototype was superseded by real SDK contracts/apps and removed
  during final cleanup. Its history remains in earlier commits.

## Tasks

### 25 — Noah exit, board and preparation fixes (v0.7.0)

- [x] Typed emergency-exit errors; `ExitProgressStatus.error` as `BarkError?`.
- [x] Whole-wallet exit estimate applies bark's eligibility filter and reports excluded VTXOs.
- [x] `prepareBoard` excluded outpoints.
- [x] Confirmed-only parents re-checked at first submit of boards and prepared sends.
- [x] `BroadcastOutcome.uncertain` when bark may have recorded a failed board.
- [x] Rust/Swift/Kotlin live RegTest and two-round GPT-6 Astra review.
- [ ] Main CI full matrix.
- [ ] Stage CI artifacts, move install docs to 0.7.0, publish v0.7.0 and notify the
      Noah rewrite thread (waits for approval).

Engineering plan: [Noah FFI fixes](plans/26-noah-ffi-fixes.md).
[Execution evidence](validation/noah-ffi-fixes-2026-10-24.md). Migration notes: [CHANGELOG](../CHANGELOG.md).

### 24 — Rust 1.99.1 toolchain

- [x] Install exactly Rust 1.99.1 and align all active project/CI/packaging pins.
- [x] Run the documented Rust and tooling checks and record actual outcomes.

Engineering plan and affected paths (2026-10-24):

- Verify availability against the official Rust release announcement and
  distribution manifest, then install `1.99.1` with `rustfmt`, `clippy` and the
  existing supported Apple/Android targets. If installation is unavailable,
  retain the working 1.90.0 pins and record the blocker; do not substitute a version.
- Align `rust-toolchain.toml`, `.github/workflows/{ci,integration}.yml`,
  `scripts/build-sdk.sh`, `scripts/swift-artifact.py`, the toolchain fixture in
  `tests/python/test_tooling.py` and `README.md`. This checklist owns the plan
  and validation record. Preserve historical validation/planning records and
  `sdk/swift/provenance.json`, which describes the already-published artifact.
- Interfaces/ownership: retain the existing toolchain selection and packaging
  interfaces; only change their version values. Keep Rust edition 2021,
  Cargo.lock, dependency pins, public APIs, wallet logic, transaction validation,
  persistence, platform minimums, ABI matrix, licenses and attribution unchanged.
  Do not touch reference checkouts, existing wallet/database volumes or unrelated
  edits. Add no tests for trivial pin changes. After local validation, the user
  requested a commit/push to the default branch (`main`; origin has no `master`)
  on 2026-10-24. Push only the scoped changes without a PR or force-push.
- Dependencies: official rustup downloads; existing protobuf/native build tools
  and locked Cargo dependencies. Install the eight supported targets already
  present under 1.90.0; its extra i686 Android target is outside the SDK matrix.
- Validation: capture `rustc --version --verbose`, `cargo --version`, installed
  components/targets and selected toolchain; run `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`,
  `cargo build --locked`, `python3 scripts/swift-artifact.py`,
  `python3 -m unittest discover -s tests/python -v`, script syntax checks and
  `git diff --check`. Review remaining version references and generated/unrelated
  changes. Record compatibility failures before making any further source edits.
  Native app execution, live RegTest, full cross-platform SDK packaging and
  hosted CI require separate execution evidence; local checks do not imply them.

Implemented: all seven planned configuration/script/documentation/fixture paths
now use 1.99.1; the checklist is the eighth changed file. No dependency, lockfile,
Rust source, generated SDK or published artifact metadata changes.

Validation commands and results (2026-10-24, macOS ARM64):

- Availability: [official release announcement](https://blog.rust-lang.org/2026/10/15/Rust-1.99.1/)
  and [distribution manifest](https://static.rust-lang.org/dist/channel-rust-1.99.1.toml)
  confirm the October 15 release and an available aarch64-apple-darwin package.
- `rustup toolchain install 1.99.1 --profile minimal --component rustfmt,clippy
  --target aarch64-apple-darwin,aarch64-apple-ios,aarch64-apple-ios-sim,x86_64-apple-darwin,x86_64-apple-ios,aarch64-linux-android,armv7-linux-androideabi,x86_64-linux-android`
  succeeded. Installed components and targets were verified with rustup; the
  repository override selects `1.99.1-aarch64-apple-darwin`. Global default and
  the existing 1.90.0 installation were preserved.
- Exact toolchain output:

  ```text
  rustc 1.99.1 (58cf8f9c3 2026-10-13)
  binary: rustc
  commit-hash: 58cf8f9c35b355199727627d07ae34a4e2846d67
  commit-date: 2026-10-13
  host: aarch64-apple-darwin
  release: 1.99.1
  LLVM version: 23.1.1
  cargo 1.99.1 (5f94df478 2026-08-27)
  ```

- `cargo fmt --check`: passed.
- `cargo clippy --locked --all-targets -- -D warnings`: passed in 1m 55s.
- `cargo test --locked`: passed (90 unit-test entries, seven integration-test
  entries, zero doctests; compilation took 2m 33s). `BARK_REGTEST` was disabled:
  five unit and all seven integration entries returned at their environment
  guards. This validates 85 offline unit tests, not those 12 live scenarios.
- `cargo build --locked`: passed after the test build (0.21s).
- `python3 scripts/swift-artifact.py`: passed published file/manifest integrity.
- `python3 -m unittest discover -s tests/python -v`: all 10 tests passed.
- `bash -n scripts/build-sdk.sh scripts/install-boltffi.sh`, ShellCheck on those
  scripts plus `scripts/ark-dev.sh` and `tests/shell/ark-dev.sh`, and
  `bash tests/shell/ark-dev.sh`: passed.
- `git diff --check`: passed. Search of active scripts/config/docs/fixtures found
  no remaining 1.90.0 pin; the old value remains in historical evidence and
  published Swift provenance only. All eight changed paths match the plan.
- Rust checks used the CI settings `CARGO_BUILD_JOBS=2`,
  `CARGO_PROFILE_DEV_DEBUG=0` and the SDK's `MACOSX_DEPLOYMENT_TARGET=13.0`,
  with Homebrew tools on PATH.

Artifacts / device or CI run evidence: local command output above; no device or
hosted CI run. Build logs are outside the repository.

Remaining limits: native app execution, live RegTest, full cross-platform SDK
packaging and hosted CI were not validated for 1.99.1 by these local checks.
Published v0.6.0 Swift artifacts retain their truthful 1.90.0
provenance; a future release must rebuild/stage artifacts before the tag's source
freshness gate can pass. Reference repositories and wallet/database volumes were
not touched.

### 23 — CI caches shared from main

- [x] Rust dependency caches via rust-cache and Gradle caches, saved only from main.
- [x] PR CI green; main saves; a warm main rerun shows the restore and build-time effect.

Engineering plan and findings: [CI caches](plans/25-ci-caches.md).
Warm main: android 33.1 → 14.2 min, apple 28.8 → 9.5 min, rust 3.3 → 1.0 min; about 4.7 GB cached.

### 22 — Bark 0.8.0, Lightning send options, payment requests and BIP 321

- [x] Pin Bark lib-0.8.0 and the matching RegTest server/CLI images.
- [x] Movement ids from arkoor sends, onchain sends and offboards.
- [x] LightningSendOptions (wait, retryForSecs) on every Lightning pay method.
- [x] parsePaymentRequest with typed option errors, payment fee estimates and sendPayment.
- [x] bip321Uri and debugInfo.
- [x] Rust/Swift/Kotlin live RegTest, both example apps, hosted CI.
- [x] Publish v0.6.0 and notify the Noah rewrite thread.

Engineering plan and affected paths: [Bark 0.8.0 plan](plans/24-bark-0.8.0-payments.md).
Live Rust/Swift/Kotlin and extended app RegTest passed locally on an isolated stack;
PR and full-matrix main CI passed. [Execution evidence](validation/bark-080-2026-10-22.md).
[v0.6.0](https://github.com/smolcars/bark-bolt-ffi/releases/tag/v0.6.0) is published from the
main CI artifacts; fresh assets and a public Swift consumer passed; Noah was notified.
[Release record](validation/v060-release.json), [cleanup](validation/v060-cleanup.json).

### 21 — Noah prepared boarding, chain status and durable exit listings

- [x] Prepared boarding with exact signed fees and shared onchain reservations.
- [x] Read-only chain transaction status without wallet synchronization.
- [x] listAllExits with amounts, terminal rows, history and transaction flags.
- [x] Rust/native/live validation, GPT-6 Astra high review and v0.5.0 SDK release.
- [x] Remove disposable builds and task-created devices after evidence is saved.
- [ ] Upstream dependency: prepared offboards/sendOnchain with fixed negotiated
  fees and shared VTXO reservations (request 2).
- [ ] Upstream dependency: atomic durable operation keys for boarding, delegated
  refresh, exit progression and claim (request 3).

Engineering plan and feasibility: [Noah follow-up](plans/23-noah-prepared-boarding.md).
The final two requirements remain open; do not treat a wrapper estimate or an
unattributed pre-submit journal as their implementation. Implemented scope passed
the full main matrix, live native contracts and final Astra review; v0.5.0 is
published and Noah received the handoff. Removed 17.69 GB of allocated build and
download output; available disk increased by 15.91 GB. No task-created device
remained; all shared devices and existing wallet containers/volumes were preserved.

### 20 — Noah priorities: items 13–16

- [x] Item 13: build all three Android ABIs with 16 KB LOAD alignment and gate every AAR `.so` in CI.
- [x] Publish and verify the v0.3.1 alignment patch release.
- [x] Item 14: exact-fee prepared onchain send/drain, reserved inputs, broadcast/discard.
- [x] Items 15/16: typed errors/outcomes and pending Lightning hash/amount/fees.
- [x] Publish/verify v0.4.0 prepared-send SDKs and notify Noah.

Engineering plan and affected paths: [Noah priorities](plans/20-noah-release-priorities.md).
Item 13 built/CI passed: [alignment evidence](validation/2026-10-19-v031.md).
v0.3.1 fresh assets and 12 live public Swift consumer contracts passed; Noah was notified.
Prepared-send native builds, live Swift/Kotlin/Rust and all source CI jobs passed.
[Prepared-send evidence](validation/prepared-onchain-2026-10-19.md),
[release plan](plans/22-prepared-sdk-release.md). v0.4.0 publication, fresh downloads and the public consumer passed; Noah was notified.

### 19 — Post-release resource cleanup

- [x] Close Bark devices and remove the dedicated Android AVD/test app data.
- [x] Remove owned Docker containers, volumes and networks; preserve Noah/Nitro.
- [x] Remove disposable build output and measure recovered disk space.

Engineering plan: [cleanup](plans/19-release-cleanup.md). Recovered 52.96 GB;
[ownership and validation results](validation/v030-cleanup.json).

### 18 — v0.3.0 full balance and no-write existing-wallet open

- [x] Expose all nine balance buckets and Bark-computed pending/total.
- [x] Validate existing-wallet identity before SQLite migrations or BDK writes.
- [x] Pass Rust, compiled Swift/Kotlin and live RegTest regression contracts.
- [x] Fable 5.1 high reviews all changes; verify, plan and fix confirmed findings.
- [x] Publish v0.3.0 XCFramework, AAR, SHA256SUMS and SPM manifest; verify CI/consumers.
- [x] Notify the active Noah native rewrite thread after publication.

Engineering plan and affected paths: [balance and existing open](plans/18-balance-and-existing-open.md).
Local contracts, Fable review, hosted source CI, publication, fresh checksums,
clean public consumer and Noah handoff passed.
[Execution evidence](validation/2026-10-19-v030.md),
[release hashes](validation/v030-release.json).

### 17 — Noah native API parity

- [x] P0 keys/signatures, v2 snapshots and committed-state subscriptions.
- [x] P1 feature parity, structured amounts and transaction details.
- [x] Fresh tagged release with verified SDK artifacts and consumer contracts.

Engineering plan and affected paths: [Noah native parity](plans/17-noah-native-parity.md).
Nitro v0.0.144 compatibility is required for existing Noah app updates; this
supersedes the earlier decision that Nitro schema compatibility was unnecessary.
P2 remains deferred. Acceptance and evidence must be recorded per priority group.
P0/P1 PRs passed all three hosted checks and are merged. v0.2.0 is published at
b8ca615c6bd0faf45ef9b9ea8661fa9f5a99eb45 with eleven assets. Fresh downloads
match SHA256SUMS; a clean public Git consumer passes all nine live Swift
contracts. [Release verification](validation/noah-release-v0.2.0.json).

### 00 — Bootstrap the new project

- [x] Publish the initial source and plan commit.

**Engineering plan:** create the requested repository; preserve upstream history,
license and compatibility evidence; establish main, GitHub origin and GitLab
upstream; document the migration's current incomplete state.

**Implement:** add README, this checklist and AGENTS.md; commit/push the baseline
and verify the remote commit. Keep original source/Nitro checkouts unchanged.

**Done when:** the GitHub repo contains the baseline, license and plan,
and local/remote commit IDs agree.

### 01 — Establish the baseline and complete API inventory

- [x] Reproduce baseline native checks and map every public API.

**Engineering plan:** inspect src/, Cargo.toml and companion SDK contracts.
Confirm the initial inventory: 128 callable exports, 40 records, 14 data enums,
3 objects and 2 callback traits. Map defaults, errors, ownership and old/new names.

**Implement:** capture Rust build/test/Clippy outcomes, pin toolchains and record
an API parity/test checklist grouped by wallet feature.

**Done when:** every old export has a planned Swift/Kotlin counterpart and
validation strategy; baseline failures are separated from migration regressions.

### 02 — Build a reproducible RegTest backend

- [x] Boot compatible, isolated Bitcoin/Ark/Lightning services.

**Engineering plan:** adapt Nitro's setup into regtest/ and lifecycle scripts.
Choose server/CLI versions compatible with the pinned Bark revision, supply config and
pin images or source revisions. Define readiness probes, funding, mining,
Lightning liquidity, endpoint configuration and cleanup before coding.

**Implement:** provide bounded start/wait/status/fund/mine/logs/stop commands;
bootstrap mature coins, server funding and CLN/LND channels. Use separate project
names/ports/test volumes. Parameterize iOS simulator, Android emulator and CI
endpoints; keep local HTTP/RPC settings in RegTest/debug configuration.

**Done when:** a fresh stack boots, a reference Bark wallet performs an actual
Ark operation, Lightning channels are funded, repeated setup works, and teardown
does not touch existing Nitro or unrelated wallet/database volumes.

### 03 — Port the full Rust binding layer

- [x] Replace production UniFFI exports with BoltFFI across the complete API.

**Engineering plan:** retain the core/native adapter seam and pin Bark as task14 specifies.
Specify concrete object returns, separate onchain factory arguments, message
errors, callback error conversion, data/default attributes and required Copy
derives before changing src/ and the manifest.

**Implement:** migrate all DTOs/enums, free functions, Wallet/OnchainWallet APIs,
all original custom-wallet callbacks plus master signPsbt, logging and notifications. Preserve SQLite,
TLS/BDK, datadir/locking and transaction/PSBT/network validation. Pin crate/CLI
versions and Cargo.lock.

**Done when:** the real crate builds and relevant Rust tests/Clippy pass.
Strict Swift/Kotlin generation has no skipped declarations; every inventory
entry has an implemented counterpart.

### 04 — Verify persistence and async/object lifecycle

- [x] Test the real SDK's persistence, callbacks, cancellation and daemon shutdown.

**Engineering plan:** define callback/handle retention and release, in-flight
object lifetime, Tokio behavior, notification reuse and shutdown. Foreign task
cancellation must not imply rollback of an already-started payment.

**Implement:** move cancel-slot cleanup into a Drop-safe path; test reopen/restore,
SQLite filename compatibility, daemon stop-and-wait, callback errors/cause
messages, logger replacement/removal, concurrent receives, explicit cancellation,
foreign cancellation and reuse. Remove the React Native logger leak workaround
once native lifetime tests establish safe release.

**Done when:** Rust and both language tests cover these paths on the real SDK,
including in-flight release and failures; no persistence is silently reset.

### 05 — Package usable Swift and Kotlin SDKs

- [x] Produce SwiftPM/XCFramework, Android AAR and Kotlin/JVM artifacts.

**Engineering plan:** settle layout, package names, deployment targets, ABI matrix,
JNI loading and debug/release artifact flow. Root SwiftPM metadata must work from
the Git URL, and apps must consume newly built local artifacts during development.

**Implement:** build iOS device/simulator and macOS slices, SwiftPM, Android JNI/AAR,
and JVM JAR with desktop natives. Replace JNA with JNI. Add contract tests for
data/enums/defaults/errors and API parity in both languages.

**Done when:** clean consumers compile and run from actual packaged artifacts;
release libraries load, full-range u64/nested values round-trip, and examples
cannot silently resolve an old UniFFI binary.

### 06 — Remove other languages and old infrastructure

- [x] Leave a Swift/Kotlin-only product with no active UniFFI or browser layer.

**Engineering plan:** identify direct features/consumers to remove; retain Rust
dependencies still needed for metadata or wallet behavior. Import only required
Swift/Kotlin companion tooling and discard its Go-dependent release graph.

**Implement:** remove WASM code, UniFFI scaffolding/features/version-tag conventions,
old GitLab CI and unused build targets. Omit Dart, Go, React Native, browser, npm
and pub tooling. Update supported targets and build commands.

**Done when:** only Swift/Kotlin consumer products remain and native checks pass.
Transitive Cargo dependency names alone do not imply an unsupported product.

### 07 — Build the native iOS example

- [x] Create and run a SwiftUI app with the real SDK.

**Engineering plan:** design small screens/view-model states for RegTest config,
create/restore/open, addresses/balances, boarding, Ark transfers, Lightning,
history, refresh and exits. Plan loading/errors/retries, lifecycle subscriptions,
task cleanup and Keychain-backed seed storage before writing UI.

**Implement:** add an iOS 16 Xcode project, local SDK dependency, debug networking,
automation identifiers and UI tests. Perform wallet logic through the SDK and
use isolated test app storage; do not log seeds.

**Done when:** the app builds, installs and launches on a simulator, persists and
reopens a real wallet and displays real backend results. Record build/test logs
and screenshots.

### 08 — Build the native Android example

- [x] Create and run a Kotlin/Compose app with the real SDK.

**Engineering plan:** mirror the iOS workflows with ViewModel/coroutine lifecycle
handling, Keystore-protected seed storage, JNI cleanup and debug host networking.

**Implement:** add the minSdk 24 Gradle app, pinned tooling, generated AAR dependency,
loading/error states, automation identifiers and instrumentation tests. Use host
mapping or adb forwarding, not a developer-specific LAN IP.

**Done when:** the APK builds, installs and launches on an emulator, loads JNI,
and creates/reopens a real wallet connected to RegTest; preserve native/app logs
and test reports.

### 09 — Prepare repeatable simulator/emulator execution

- [x] Resolve device prerequisites and automate device readiness.

**Engineering plan:** select local/CI OS images; install the missing Android
emulator/system image; define boot readiness, forwarding and test data isolation.

**Implement:** provide bounded setup/test commands. Open local devices through
T3's Device panel and drive them with its returned agent-device launcher; use
platform build/log tools as needed.

**Done when:** both apps execute on actual simulator/emulator processes with
reachable endpoints. Missing runtimes/devices are resolved, not counted as passes.

### 10 — Run end-to-end RegTest acceptance on both apps

- [x] Demonstrate actual wallet workflows and assertions on iOS and Android.

**Engineering plan:** define deterministic balances/state transitions, block
mining and bounded confirmation waits. Separate complete API contract coverage
from important UI workflows. Identify additional LNURL/BOLT12 fixtures explicitly.

**Implement:** create a wallet on each platform; fund/sync/board; send Ark in both
directions; assert balances/history/notifications; pay and receive Lightning via
CLN/LND; exercise refresh, offboarding and unilateral exit/claim with mined
timelocks. Test restart, restore/rescan, unavailable server, invalid input and
cancellation/reconnect.

**Done when:** real SDK/app outcomes agree with node/server state; restart
preserves data; save transaction IDs, test summaries, screenshots and sanitized
logs. Resolve or explicitly document uncovered scenarios before claiming parity.

### 11 — Add GitHub Actions for Rust and SDK builds

- [x] Build and test Rust plus generated SDK artifacts on GitHub.

**Engineering plan:** define PR/push triggers, pinned toolchains, Linux/macOS
matrices, caches, API/generation checks and artifact provenance from the same
commit. Separate release/signing permissions from normal tests.

**Implement:** add format/Clippy/tests/audit, strict generation, Apple packaging,
Android AAR/JVM JAR builds, native contract tests and artifact uploads.

**Done when:** actual GitHub workflow runs pass and expose downloadable artifacts
from the migrated commit. YAML validation alone is insufficient.

### 12 — Add GitHub Actions for the apps and runtime tests

- [x] Build both apps and execute automated simulator/emulator tests in CI.

**Engineering plan:** use macOS/Xcode for iOS and Linux with accelerated Android
emulation. Define RegTest startup and networking explicitly: hosted macOS cannot
be assumed to support Linux Docker service containers. Validate a supported
backend connection/container runtime or provision a suitable macOS Actions runner
before promising an iOS RegTest gate.

**Implement:** build against SDK artifacts from the same workflow, run native app
tests, upload APKs/xcresult/test reports/logs, and execute real RegTest smoke tests.
Run longer Lightning/exit/recovery cases in a defined integration job.

**Done when:** actual Actions runs build both apps and execute tests; required
RegTest jobs have passed, rather than being skipped or mocked. Simulator builds
need no App Store signing credentials.

### 13 — Validate release outputs and finish documentation

- [x] Deliver reproducible SDKs, apps, RegTest setup, CI and migration guidance.

**Engineering plan:** review parity, platform/test matrices, packaged contents,
API changes, install/build commands and artifact version/checksum metadata.
Distinguish a release-ready build from publishing a release.

**Implement:** document installation, breaking changes, RegTest, app execution
and CI; exercise release-mode packaged consumers and clean builds. Replace or
archive the proof when real tests supersede it; preserve license/attribution.

**Done when:** another developer can reproduce both apps and the backend from
documented commands; every API is accounted for, required runtime/CI checks pass,
packaged consumers work and remaining limitations are accurately recorded.

### 14 — Align current master, user Docker images and cleanup

- [x] Revalidate the final migration against current Bark master and dated user images.

**Engineering plan:** docs/plans/14-master-and-cleanup.md records source/image
revision inspection, ownership/PSBT/typed-height adaptations, cleanup and checks.

**Implement:** pin bfbe6b7c, retain all original exports, add master sign-only
callback/swept state/fee margin, use dated multiarch images and delete superseded
prototype/unused dependencies/assets. Preserve prior wallet volumes.

**Done when:** strict generation/parity,57 Rust tests/Clippy/audit, full release
SDKs, clean package consumers, both rebuilt apps, extended RegTest and actual
hosted Actions pass for the updated implementation. Prior 0.7.1 evidence remains
historical and is not substituted for this acceptance.

### 15 — Validate and fix Opus 5.5 review findings

- [x] Fix confirmed review issues and record validation evidence.

**Engineering plan:** [review fixes](plans/15-opus-review-fixes.md) records the
finding dispositions, affected interfaces, seed/data invariants and checks.

**Implement:** safe release staging/development CI, runner hardening, shared fresh
recovery paths/backup exclusions, public API documentation and focused Rust cleanup.

**Done when:** regression checks, contracts, rebuilt native apps, fresh paired
RegTest and actual hosted CI pass, fixes are pushed and remaining limits are explicit.

**Results:** fixes pushed at 95d90fd. [Review dispositions and evidence](validation/opus55-review-2026-10-19.md)
record failing/passing regressions, two independent fresh restores per platform,
[green hosted SDK/app CI](https://github.com/smolcars/bark-bolt-ffi/actions/runs/39249620824)
and [green paired RegTest CI](https://github.com/smolcars/bark-bolt-ffi/actions/runs/39249668162).
The ephemeral runner and its per-run containers/volumes were removed. Published
v0.1.0 artifacts remain unchanged; these source fixes are on main.

### 16 — Restore Just/Bash RegTest tooling and update GitHub Actions

- [x] Use Nitro Ark's familiar Just recipes/Bash commands and latest stable actions.

**Engineering plan:** [Just/Bash and Actions](plans/16-just-and-current-actions.md)
records the retained interfaces, volume/seed invariants, exact upstream action
releases and checks.

**Implement:** restore the root Justfile and scripts/ark-dev.sh; delete the Python
stack manager; route mobile automation through Bash; update all action pins and
active instructions. Keep Python only for structured mobile/runner helpers.

**Done when:** shell safety/lint checks, real repeated setup with preserved seed,
full paired app acceptance and both updated CI workflows pass; changes are pushed
and evidence is recorded.

**Results:** restored the original command family, including raw captaind
passthrough, and pushed at f66a75d. Real repeated/fresh setup, seed preservation,
ordinary down's six retained volumes and explicit CI cleanup pass.
[Hosted SDK/app CI](https://github.com/smolcars/bark-bolt-ffi/actions/runs/39260131486)
and [paired RegTest CI](https://github.com/smolcars/bark-bolt-ffi/actions/runs/39260224145)
pass with the latest stable action pins. [Validation and evidence](validation/just-and-actions-2026-10-19.md)
record commands, exact versions, transaction summaries and scope limits.

## Dependency order

00 -> 01 -> 02 -> 03 -> 04 -> 05 -> 06 -> 07/08 -> 09 -> 10 -> 11/12 -> 13.
CI scaffolding and device setup may start earlier after their engineering plans
are written; dependencies and completion evidence still apply.

## Completion records and evidence

### 00–01: repository and baseline

Plan/paths: task00 above, [baseline plan](plans/01-baseline-and-api.md), root
metadata and [API inventory](api-baseline.json). Preserved upstream history/MIT
license, created the requested repository and mapped128 exports, 40 records,
14 enums and 2 callback protocols. All52 original Rust tests and baseline Clippy
passed. The repo is now public under smolcars after an external transfer.
The original Bark/Nitro reference checkouts remain unchanged.

### 02–04 and06: backend, port, lifecycle and removal

Plan/paths: [RegTest](plans/02-regtest.md), [native port](plans/03-native-port.md)
and [master adaptation](plans/14-master-and-cleanup.md); Cargo/src, regtest and
scripts. All original names are accounted for by strict generation/API parity;
master adds signPsbt, VtxoSwept and fee margin.57 Rust tests, warnings-denied
Clippy and audit with no ignored advisories pass. Tests cover unsigned PSBT
immutability, delta bounds, callbacks, cancellation/reuse and persistence.
The reproduced Kotlin reentrant cancellation deadlock was fixed in shared Rust.

The six-service isolated backend bootstraps idempotently, boards real sats and
provides Lightning liquidity. Matched master images recover funded seeds; the
older server's missing recovery RPCs were proved with a red/green upstream CLI
probe. Existing wallet volumes were preserved. UniFFI/WASM source, GitLab CI,
prototype, obsolete stubs, unused dependencies/helper and image were deleted.
Required Rust/build/test helpers remain. Evidence: [master validation](validation/2026-10-19-master.md).

### 07–10: apps, devices and complete local transaction acceptance

Plan/paths: [native apps](plans/07-08-native-apps.md); examples, SDK contracts,
app-regtest.py, wait-devices.py, PRODUCT and DESIGN. Implemented native workflows,
protected seed storage, persistent SQLite, async lifecycle/errors/accessibility
and debug-only test transport. Both apps build/install/run; local UI tests,
Android lint/minified build and independent Maven consumers pass. Device setup
uses real iOS27 ARM64 simulator/API 35 ARM64 emulator processes.

The optimized native SDK app run passes on both platforms: actual notifications,
two-way Ark transfers, settled BOLT11 send/receive, funded seed recovery,
refresh/offboard, fee estimation, exit progress and confirmed claims, reopen,
invalid input and unavailable-server retry. Evidence: [release-native summary](validation/master-release-20261019-r1.json),
[native UI review](validation/native-ui-review.md) and twelve retained screenshots.
Physical/minimum-OS devices and external LNURL/LNAddress/BOLT12 providers were not run.

### 05 and11–14: final packaging, Actions, release and master gates

Plan/paths: [SDK packaging](plans/05-sdk-packaging.md), [Actions](plans/11-12-actions.md),
[release artifacts](plans/13-release-artifacts.md), [master cleanup](plans/14-master-and-cleanup.md).
Full optimized Apple five-pair and Android three-ABI builds pass. Swift contracts
pass from a clean ZIP consumer without newer-macOS linker warnings. The deployment
gate reproduced and corrected a macOS 27 native-object minimum to macOS 13.
Kotlin AAR/JAR content checks, three regular and three actual-JAR contracts,
MavenLocal publication and independent minified Android/JVM consumers pass.

Paired master [integration 39201337865](https://github.com/smolcars/bark-bolt-ffi/actions/runs/39201337865)
passed. Its post-job volume check exposed a profile-excluded CLI fixture volume;
the corrected teardown removes it. Full integration 39202007139 passed with
the correction; post-job checks find no fixture volumes or registered runner.
SDK source is unchanged by teardown/documentation edits.

Hosted [run39201332568](https://github.com/smolcars/bark-bolt-ffi/actions/runs/39201332568)
passed Rust, Android and Apple jobs, including UI/SDK contracts and deployment
gates. [Release v0.1.0](https://github.com/smolcars/bark-bolt-ffi/releases/tag/v0.1.0)
is published at 6dae2db with ten checksum-verified assets. The clean public Git-URL
Swift consumer downloads the actual archive, links and passes all 3 contracts.
Its harness disables ambient Keychain/netrc credentials; SDK source and published
asset checksums are unchanged by that isolation setting.

Final commands: `python3 scripts/swift-artifact.py`, `python3 scripts/check-api.py`,
packaged contract/artifact checks, documented live acceptance commands and actual
Actions runs all pass. Documentation relative links and `git diff --check` pass.
The final follow-up contains documentation, retained Nitro attribution and the
release-check credential isolation only; the tested SDK source is unchanged.

Remaining limits: physical/minimum-OS execution and external LNURL/LNAddress/BOLT12
providers are untested. App stores and Maven Central publication are outside scope.
The separate [master validation record](validation/2026-10-19-master.md) contains
the commands, transaction summaries, source/CI/release links and exact checksum. Prior 0.7.1
run39195200829 is retained as historical evidence, not master acceptance.
