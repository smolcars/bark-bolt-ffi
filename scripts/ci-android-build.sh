#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
export RUSTUP_TOOLCHAIN=1.99.1
export CARGO_PROFILE_DEV_DEBUG=0
test -x /usr/bin/cc
test -x /usr/bin/c++
rustup toolchain install "$RUSTUP_TOOLCHAIN" --profile minimal
targets=(aarch64-linux-android x86_64-linux-android)
if [[ "${BARK_PR_BUILD:-0}" != 1 ]]; then targets+=(armv7-linux-androideabi); fi
rustup target add "${targets[@]}"
export BOLTFFI
BOLTFFI="$(command -v boltffi)"
just build-android --release
"$BOLTFFI" generate swift --deny-skipped
python3 scripts/check-api.py
./gradlew --no-daemon :sdk:android:assembleRelease :sdk:android:generatePomFileForReleasePublication \
    :sdk:jvm:test :sdk:jvm:jar :sdk:jvm:generatePomFileForJvmPublication :sdk:jvm:testPackaged \
    :sdk:jvm:stageCiTests :examples:android:assembleRelease :examples:android:assembleDebug \
    :examples:android:assembleDebugAndroidTest
python3 scripts/check-android-alignment.py
python3 scripts/check-kotlin-artifacts.py
mkdir -p target
cargo test --locked --no-run --message-format=json > target/ci-rust-tests.jsonl
python3 scripts/android-ci.py bundle
