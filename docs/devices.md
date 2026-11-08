# Native device setup

Use Xcode's Simulator app and Android Studio's Device Manager to select and boot
an iPhone and an Android API35 phone. Local Apple silicon uses an arm64 Android
system image; GitHub-hosted Linux uses x86_64 with KVM.

For command-line Android setup, with ANDROID_HOME and JDK17 configured:

```sh
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" 'emulator' \
  'system-images;android-35;google_apis;arm64-v8a' 'platforms;android-35' \
  'build-tools;35.0.0' 'ndk;28.2.13676358'
"$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager" create avd \
  --name BarkRegtest --package 'system-images;android-35;google_apis;arm64-v8a' --device pixel_7
"$ANDROID_HOME/emulator/emulator" -avd BarkRegtest -gpu auto
```

Create the AVD once; reuse it subsequently. Find identifiers with
`xcrun simctl list devices available` and `adb devices`. Readiness is bounded:

```sh
just wait-devices --ios-device IOS_UDID --android-device emulator-5554
```

Install Just and jq, build/install apps from README, then run
`just test-regtest` with those IDs.
The driver configures adb reverse for the selected RegTest ports. T3 users can
open both devices in the Device panel; verification in this migration used the
agent-device launcher returned by device_open for interactive navigation.

Full Actions integration needs these tools on an isolated macOS ARM64 account
or machine that runs only trusted repository code. Start one bounded job with:

```sh
just ci-regtest --ios-device IOS_UDID --android-device emulator-5554
```

The helper requires authenticated GitHub CLI repository-admin access. Before
registering, it checks that fork workflows require approval for **all external
contributors**. It registers an ephemeral runner with a random per-job label and
no default labels, then dispatches `integration.yml` on `main` with that label.
The workflow rejects other events/refs. No separate `gh workflow run` is needed.
After one job the runner unregisters and removes its temporary files. CI uses a
unique Compose project; teardown removes only that run's containers and volumes.
Local `just stop` and ordinary `just down` preserve developer wallet volumes.

The Just recipes retain Python internally for mobile JSON polling/assertions,
device readiness and the runner's archive/environment validation. RegTest service
setup, wallet funding, mining and CLI access live only in `scripts/ark-dev.sh`.

The listener receives an explicit build/device environment without inherited
GH tokens, SSH agent variables or unrelated credentials. This is **not a sandbox**:
a job still runs as its OS user and can access that user's files, Keychain and
Docker. Never approve untrusted fork code for this machine. For managed runners,
use a dedicated isolated host/account and workflow-restricted runner groups
where available; a public-repository runner should not be a permanent process
on a personal developer account. See [GitHub's runner access guidance](https://docs.github.com/en/actions/how-tos/manage-runners/self-hosted-runners/manage-access).

## Linux Android build runners

`linux-blixt-europe` and `linux-blixt-europe-2` run the Android compilation job,
with six CPUs and a 16 GiB memory cap each. The third registration is retained but
disabled. Both runner processes and the Nix daemon share a 12-CPU/32-GiB ceiling.
Their systemd definitions are tracked in `ops/`; each account has a private home,
project caches, and workspace, with no sudo or host Docker socket access.

The `linux-blixt-europe` organization group allows only this repository and the
exact immutable `.github/workflows/android-build.yml` revision called in `ci.yml`.
That workflow requires `android-pr-build` approval from the repository owner for
every PR build. Review the concrete source commit before approving; approval
grants that code access to the runner account and its persistent caches. Main/tag
pushes and explicit trusted dispatches use `android-trusted-build` instead.

When changing the reusable build workflow, commit its reviewed definition first,
then update both the caller's SHA pin and the runner group's selected workflow
entry. A PR cannot bypass approval by replacing the caller or its environment.
No administrator token is installed on the build host.

Nix build commands use the committed lockfile and each project's pinned shell.
The Android SDK/NDK, BoltFFI CLI and Java/protobuf/build utilities come from Nix; Rust 1.99.1 is
selected through Rustup and `rust-toolchain.toml`. Host-linked JNI/Rust binaries
use `/usr/bin/cc` rather than the Nix compiler so Blacksmith can load them without
installing Nix. The build checks their dynamic dependencies before upload.

The runtime payload includes compiled APKs, JVM main/test classes and dependencies,
Rust test executables, release SDK files, and a source-commit/checksum manifest.
Blacksmith verifies that payload against its checkout and runs the existing
acceptance tests directly, without Gradle or Cargo compilation. Emulator/KVM and
the per-run Docker RegTest stack remain on Blacksmith; cleanup affects only that
run's Compose project. The final artifact remains `android-sdk-app-reports`.
