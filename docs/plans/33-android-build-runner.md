# Android compilation on the Linux runner pool

## Engineering plan

The user wants Android compilation moved off paid Blacksmith machines while
retaining accelerated emulator tests there. Approved PR builds should also use
the shared Linux host. This change builds on task 32's two bounded runner services.

### Paths and interfaces

- Add `flake.nix` / `flake.lock`: one pinned Linux Android development shell with
  Java 17, Rustup (Rust itself remains pinned by `rust-toolchain.toml`), protobuf,
  Android API 35/build-tools 35.0.0/NDK 28.2.13676358, and existing build utilities.
  Use `mkShellNoCC` and the Linux host's C compiler deliberately: the desktop JNI
  library and Rust test executables must link the host glibc, rather than acquire
  Nix-store runtime dependencies that Blacksmith does not have. Supported build
  baselines are Debian 12 and Ubuntu 24.04; verify the actual output dependencies.
- Add `.github/workflows/android-build.yml`, a reusable workflow called at an
  immutable commit. Only this exact workflow/ref may access runner group 12.
  Its fixed definition requires the `android-pr-build` protected environment for
  every PR invocation. Configure the repository owner as required reviewer;
  approve the concrete PR commit before executing its scripts on the host.
  Allow only `pull_request`, trusted main/tag pushes, and explicit dispatch events.
  No arbitrary checkout-ref input or inherited repository secrets.
- Change `.github/workflows/ci.yml`: call the pinned build workflow, then run the
  existing `android` acceptance gate on Blacksmith using compiled artifacts from
  that build in the same workflow run. Retain Rust and Apple jobs unchanged.
- Add `scripts/ci-android-build.sh` and `scripts/android-ci.py`: compile all current
  SDK/app outputs and Rust test executables, bundle a narrow runtime payload with
  source commit/file hashes, verify/extract it on Blacksmith, and execute tests
  without Cargo, Gradle, SDK generation, or Kotlin compilation on Blacksmith.
- Update `sdk/jvm/build.gradle.kts`: stage compiled main/test classes and runtime
  dependencies, reusing the current contract-class list. On Blacksmith run both
  regular-classpath and packaged-JAR live contracts directly with Java.
- Replace three test-only absolute `CARGO_MANIFEST_DIR` script paths in Rust unit
  tests with the repository-relative path already used by integration tests.
  The prebuilt tests run from the checked-out repository root on Blacksmith.
- Add focused Python tests for payload provenance and instrumentation failure
  detection. Update this record, task 33, and CI operating instructions.

### Ownership, caching, and access

Each runner retains its own project Cargo target/download cache, Rust toolchains,
and Gradle cache outside checkout cleanup. No Docker access on the build host.
The original six-CPU/16-GiB runner and shared Nix resource limits stay unchanged.
The Nix lockfile is committed and CI must not update it implicitly.

Configure `android-pr-build` approval before granting runner access. Scope the
runner group to this repository and the exact pinned reusable workflow, with
public repository access enabled only for that restricted definition. A PR cannot
remove approval by changing its caller workflow. Updating the reusable workflow
requires a new immutable pin and matching group allowlist update, recorded here.
The current PR's approval is a user decision; do not approve it on the user's behalf.

### Invariants

Preserve the full main/tag ABI matrix, lean PR matrix, API generation checks,
16-KiB alignment gate, packaged AAR/JAR checks, Rust/Java contracts, Android
instrumentation, and live app RegTest acceptance. Apps use SDKs built from the
same source commit. Upload artifacts under the existing final
`android-sdk-app-reports` name so release staging keeps its contract.

Only explicit runtime payload paths cross jobs; executable modes survive inside
the archive. Reject a mismatched source commit or modified payload before tests.
Verify instrumentation reports a positive test count and fails on runner errors.
RegTest remains isolated on Blacksmith, with per-run Compose project names and
explicit cleanup/log capture. Never alter existing host wallet containers/volumes.
No upstream wallet behavior, persistence, transaction validation, or SDK API change.

### Dependencies and validation

Start from current `origin/main` (the initial local checkout was stale). Existing
task 28 is now a released wallet change, so record runner provisioning as task 32
and this migration as task 33.

Validate workflow syntax with actionlint and scripts with ShellCheck/Python tests.
Evaluate and lock the Nix shell, then execute the real build on the Linux host as
the unprivileged runner user under the service resource limits. Check native ELF
dependencies and exercise the runtime payload. Open a real PR with the plan and
changes. Record actual CI evidence separately: an environment approval request,
local build, or YAML check does not establish hosted runtime acceptance. Do not
mark the migration complete until the real split CI passes.

## Outcomes

Pending implementation and real build/CI validation.
