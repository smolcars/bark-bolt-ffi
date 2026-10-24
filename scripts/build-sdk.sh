#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
export PATH="/opt/homebrew/bin:$PATH"
export RUSTUP_TOOLCHAIN=1.99.1
export CARGO_PROFILE_DEV_DEBUG=0
if [[ "$(uname -s)" == Darwin ]]; then
    export MACOSX_DEPLOYMENT_TARGET=13.0
fi
BOLTFFI="${BOLTFFI:-boltffi}"
[[ "$("$BOLTFFI" --version)" == "boltffi 0.31.1" ]] || { echo 'Use BoltFFI CLI 0.31.1' >&2; exit 1; }
platform="${1:?Choose apple or android}"
shift
if [[ "${BARK_PR_BUILD:-0}" == 1 ]]; then
    set -- --overlay boltffi.pr.toml "$@"
fi
if [[ "$platform" == apple ]]; then
    export IPHONEOS_DEPLOYMENT_TARGET=16.0
    "$BOLTFFI" pack apple "$@" --deny-skipped --layout ffi-only
    cp sdk/swift-support/*.swift dist/apple/Sources/BoltFFI/
    "$BOLTFFI" generate swift --deny-skipped
elif [[ "$platform" == android ]]; then
    "$BOLTFFI" pack android "$@" --deny-skipped
else
    echo "Unsupported SDK: $platform" >&2; exit 1
fi
