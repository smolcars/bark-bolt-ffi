# Android CI build split

## Plan

Move compilation to the existing Linux pool; keep accelerated tests on Blacksmith.
Same-repository PRs authored by owners, members or collaborators build automatically.
Fork PRs and other authors require owner approval, enforced by an immutable reusable
workflow and its runner-group allowlist. Main/tag and explicit trusted dispatches
also run automatically. Verify this PR starts without a deployment approval.

Affected paths: the two Android CI workflows, `flake.nix`/`flake.lock`, the build
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

## Validation and remaining work

- Passed: actionlint, ShellCheck, 13 Python tests, Swift release-file integrity.
- Configured and exercised: immutable runner access and the approval environment.
  Narrow approval to fork/untrusted PRs so trusted pushes do not repeatedly stop.
- Real Linux release-matrix build and split [PR #12 CI](https://github.com/smolcars/bark-bolt-ffi/pull/12)
  are running. The first live check found the upstream CLI's glibc requirement;
  packaging that build tool in Nix resolved it. Compilation is now in progress.
- Pending: completed native build, payload portability and live Blacksmith tests.
  Do not mark runtime acceptance complete from syntax checks or an approval gate.
