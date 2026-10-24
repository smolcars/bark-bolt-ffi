# Bark BoltFFI

Native **Swift and Kotlin** bindings for [Bark 0.8.0](https://gitlab.com/ark-bitcoin/bark/-/tree/lib-0.8.0),
using BoltFFI 0.31.1. Includes SwiftUI and Jetpack Compose example wallets,
isolated Bitcoin/Ark/Lightning RegTest infrastructure and GitHub Actions.

See [installation and breaking changes](docs/migration.md), the
[production checklist](docs/production-migration-todo.md) and
[validation evidence](docs/validation/2026-10-19-master.md). The examples perform wallet
operations through the generated SDKs; RegTest helpers supply test coins and blocks.

Targets: iOS 16+, macOS 13+, Android API 24+, JVM 17. Swift and Kotlin APIs may
break from the original UniFFI API. Rust retains upstream wallet logic,
transaction/PSBT validation, SQLite persistence and native datadir locking.

## Install

Swift: add `https://github.com/smolcars/bark-bolt-ffi` in Xcode and link
`BarkBoltFFI`. The root SwiftPM package uses the versioned, checksummed release
XCFramework. Kotlin: build the AAR/JVM JAR below, or download [versioned release artifacts](https://github.com/smolcars/bark-bolt-ffi/releases/tag/v0.7.0);
[the migration guide](docs/migration.md) documents local Maven publication.

## Local build

Install Rust 1.99.1, protobuf (`protoc`), Just, jq, JDK17 and the Android SDK/NDK28.2.13676358.
Apple builds need Xcode; the example project uses XcodeGen. Homebrew tools must
be on PATH. Install the pinned BoltFFI CLI with:

```sh
scripts/install-boltffi.sh
export BOLTFFI="$PWD/target/tools/boltffi"
just build-ios --overlay boltffi.local.toml
just build-android --overlay boltffi.local.toml
```

The local overlay selects Apple silicon/Android arm64. Omit it for the full ABI
matrix and add `--release` for optimized SDKs. PR CI uses `BARK_PR_BUILD=1` to
select Android arm64/x86_64 and ARM64 Apple slices via `boltffi.pr.toml`.
Main/tag builds retain all supported architectures. Generated bindings are never
patched. SwiftPM package: `dist/apple`; Android AAR: `sdk/android/build/outputs/aar`;
JVM JAR: `sdk/jvm/build/libs`. Compile Kotlin artifacts with:

```sh
./gradlew :sdk:android:assembleRelease :sdk:jvm:test :examples:android:assembleDebug
swift test --package-path tests/swift
xcodegen generate --spec examples/ios/project.yml
```

Open `examples/ios/BarkExample.xcodeproj` and build the BarkExample scheme for an
iPhone simulator. Simulator Keychain storage uses local ad hoc signing without
an Apple developer account. Device signing needs your normal Apple team setup.

## RegTest

```sh
just setup-everything
just status
just bcli getblockchaininfo
just aspd rpc wallet
just bark balance
just generate 6
just send-to ADDRESS 0.1
just stop
```

The Justfile delegates to `scripts/ark-dev.sh`, adapted from Nitro Ark's original
setup. `setup-everything` starts/funds Bitcoin, Ark, the reference Bark wallet and
Lightning, reusing existing wallets and channels. The familiar `up`, `down`,
`create-wallet`, `create-bark-wallet`, `fund-aspd`, `setup-lightning`, `aspd`,
`lncli` and `cln` recipes are available through `just --list`.

Docker Compose uses isolated `bark-bolt-ffi` volumes. Normal `just down` preserves
wallet volumes; setup never forcibly replaces a wallet.
Bitcoin RPC is `http://127.0.0.1:19443` (`second` / `ark`, RegTest credentials only),
Ark is `http://127.0.0.1:3635`. Android reaches these through `adb reverse`; iOS
simulators use host loopback. Fund an app's onchain address, mine, sync and board.
The SDK performs wallet operations; the helper only funds/mines test coins.

With both apps installed, run real cross-platform acceptance:

```sh
just test-regtest --ios-device SIMULATOR_UDID --android-device emulator-5554
just test-regtest --extended --ios-device SIMULATOR_UDID
```

Each run uses a new private app wallet account. Recovery phrases stay in Keychain
or Android Keystore-encrypted private storage and never enter test output. The
debug command bridge is excluded/disabled in release apps.
The test driver remains Python for device JSON polling and assertions; every
backend setup/funding/mining/RPC operation goes through the same Bash script.

## CI

[SDK and native apps](.github/workflows/ci.yml) builds Rust, SwiftPM/XCFramework,
Android AAR/JVM JAR and both apps on Blacksmith Linux/macOS runners. It executes
SDK contracts, iOS UI tests and Android UI/RegTest tests.

[Full cross-platform acceptance](.github/workflows/integration.yml) is manually
dispatched on a macOS ARM64 runner with Xcode,
Docker, JDK17, Android SDK/NDK, a booted simulator/emulator, `IOS_DEVICE` and
`ANDROID_SERIAL`. This explicitly avoids assuming Linux containers work on a
hosted macOS runner. Tests use a separate Compose project/ports. The helper
registers a unique per-job label and dispatches the trusted main workflow.
The validation record links actual completed runs. Current-master integration passed in
[Actions39260224145](https://github.com/smolcars/bark-bolt-ffi/actions/runs/39260224145),
and [all hosted SDK/app jobs passed](https://github.com/smolcars/bark-bolt-ffi/actions/runs/39260131486);
see [device/runner setup](docs/devices.md) to repeat it.
The [Just/Bash validation record](docs/validation/just-and-actions-2026-10-19.md)
documents the restored commands, latest action pins and preservation checks.

## Upstream

Forked from [ark-bitcoin/bark-ffi](https://gitlab.com/ark-bitcoin/bark-ffi).
Original history and [MIT license](LICENSE) are retained. [Nitro Ark](https://github.com/smolcars/react-native-nitro-ark)'s RegTest
layout informed the isolated backend; its snapshot and state-observer code also
informs the Noah compatibility APIs. Its [MIT notice](regtest/LICENSE) is retained.
Its reference checkout/volumes are untouched.
