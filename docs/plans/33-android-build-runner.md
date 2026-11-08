# Linux CI builds

## Plan

Move compilation to the existing Linux pool; keep accelerated tests on Blacksmith.
Move the existing Rust formatting, Clippy, Python/shell checks and offline tests to
the same pool. Add a minimal `.#rust` shell sharing Rust/native tools with Android,
without Android SDK/Java dependencies. Replace the Android-only reusable workflow
with one pinned Linux workflow accepting only `rust` or `android`; retain the same
trust policy and project Cargo caches. Keep every existing Rust check and tag
provenance check. Validate Nix evaluation, workflow syntax and real Rust CI before
marking that migration complete. No runner/service provisioning is part of this PR.
Same-repository PRs authored by owners, members or collaborators build automatically.
Fork PRs and other authors require owner approval, enforced by an immutable reusable
workflow and its runner-group allowlist. Main/tag and explicit trusted dispatches
also run automatically. Verify this PR starts without a deployment approval.

Affected paths: the CI and reusable Linux workflows, `flake.nix`/`flake.lock`, the build
script and compiled-test transfer helper, JVM test staging, three test-only Rust
script paths, existing Python regression tests, the README CI paragraph, and this
execution checklist. Runner provisioning, systemd units, setup history, and other
documentation changes are outside this PR and will be removed from its diff.

Use pinned Nix Android/Java/protobuf/BoltFFI tools and existing Rust 1.99.1. Give
BoltFFI its required Nix runtime libraries; use the host compiler for portable JNI
and Rust tests. Keep each runner's project caches outside checkout cleanup.

Transfer only compiled SDKs/APKs, Rust executables and JVM classes/dependencies
from the same commit. Verify the payload before executing it. Preserve the full
main/tag and lean PR ABI matrices, API/alignment/package checks, existing Rust/JVM/
Android/RegTest coverage, and final `android-sdk-app-reports` artifact name.
Blacksmith runs compiled tests directly rather than compiling again. Keep RegTest
isolated by workflow run and clean up only that run's containers/volumes.

No wallet logic, SDK API, persistence or transaction-validation change. Existing
runner services and wallet containers are not modified by the PR cleanup.

## Validation

- Runtime fix plan: `scripts/ark-dev.sh` must wait for `lncli connect` to succeed;
  LND can report chain sync before its peer server starts. Reuse the existing
  bounded wait, add one startup-race regression in `tests/shell/ark-dev.sh`, and
  rerun real CI. Preserve funding/channel logic and all wallet volumes.
- Passed: actionlint, ShellCheck, 13 Python tests, Swift release-file integrity.
- Passed: required approval for untrusted PRs and automatic trusted-PR routing.
- Passed: manual full ABI build in 37m39s, including portable native libraries and
  the verified runtime bundle. Its temporary service override was removed and
  the normal runner listener restored. No further manual build is needed.
- [CI run 42443649277](https://github.com/smolcars/bark-bolt-ffi/actions/runs/42443649277)
  passed self-hosted Rust checks, self-hosted Android compilation, and Apple.
  Blacksmith verified/unpacked the payload, then hit LND's peer-server startup
  race before acceptance tests. The bounded connection wait now has a regression
  that reproduces a rejected first connection despite successful chain sync.
- [CI run 42445168836](https://github.com/smolcars/bark-bolt-ffi/actions/runs/42445168836)
  passed all four jobs on code commit `a57b3c3`: Rust and Android compilation on
  the Linux pool, Apple on Blacksmith, and Blacksmith runtime acceptance. The
  transferred payload passed provenance checks; 116 Rust tests, two Android
  instrumentation tests, both 16-test JVM runs, and live app RegTest passed.
- Both normal Linux listeners are active with no validation overrides; the third
  stays disabled. Physical devices and additional ABI runtime coverage remain
  outside this migration's acceptance scope.
