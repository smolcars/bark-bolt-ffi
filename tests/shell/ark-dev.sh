#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
mkdir "$temporary/bin"
export ARK_TEST_LOG="$temporary/arguments" ARK_TEST_CALLS="$temporary/calls"
export PATH="$temporary/bin:$PATH"
cat > "$temporary/bin/docker" <<'EOF'
#!/usr/bin/env bash
printf '%s\0' "$@" >> "$ARK_TEST_LOG"
printf 'call\n' >> "$ARK_TEST_CALLS"
case "${ARK_TEST_MODE:-}" in
    lightning-startup)
        case " $* " in
            *' listwallets '*) printf '["miner"]\n' ;;
            *' getnewaddress '*) printf 'miner-address\n' ;;
            *' getblockcount '*) printf '100\n' ;;
            *' getinfo '*) printf '{"id":"peer","binding":[{"port":9735}],"synced_to_chain":true,"block_height":100,"blockheight":100}\n' ;;
            *' pendingchannels '*) printf '{"pending_open_channels":[]}\n' ;;
            *' newaddress '*) printf '{"address":"lnd-address"}\n' ;;
            *' newaddr '*) printf '{"bech32":"cln-address"}\n' ;;
            *' walletbalance '*) printf '{"confirmed_balance":"2000000"}\n' ;;
            *' listpeers '*) printf '{"peers":[]}\n' ;;
            *' connect '*)
                printf 'attempt\n' >> "$ARK_TEST_CONNECT_CALLS"
                if [[ $(wc -l < "$ARK_TEST_CONNECT_CALLS") -eq 1 ]]; then
                    echo '[lncli] server is still in the process of starting' >&2
                    exit 1
                fi
                printf '{}\n'
                ;;
            *' openchannel '*) touch "$ARK_TEST_CHANNEL"; printf '{}\n' ;;
            *' listchannels '*)
                if [[ -f "$ARK_TEST_CHANNEL" ]]; then
                    printf '{"channels":[{"active":true,"remote_pubkey":"peer"}]}\n'
                else
                    printf '{"channels":[]}\n'
                fi
                ;;
            *) printf '{}\n' ;;
        esac
        ;;
    existing) printf 'yes\n' ;;
    fail) exit 42 ;;
    *) printf '[]\n' ;;
esac
EOF
chmod +x "$temporary/bin/docker"
script="$ROOT/scripts/ark-dev.sh"
compose="$ROOT/scripts/../regtest/compose.yaml"

# Just and Bash must preserve whitespace, quotes, empty arguments and newlines.
# shellcheck disable=SC2016
json='{"two words":0.1,"literal":"$(never execute this)"}
'
printf '%s\0' compose -f "$compose" exec -T bitcoind bitcoin-cli -regtest \
    -rpcuser=second -rpcpassword=ark -rpcwallet=miner sendmany '' "$json" > "$temporary/expected"
"$script" bcli sendmany '' "$json" >/dev/null
cmp "$temporary/expected" "$ARK_TEST_LOG"
: > "$ARK_TEST_LOG"
just --justfile "$ROOT/justfile" bcli sendmany '' "$json" >/dev/null
cmp "$temporary/expected" "$ARK_TEST_LOG"

# aspd retains the original raw captaind interface, including explicit rpc.
for argument in --version rpc; do
    : > "$ARK_TEST_LOG"
    just --justfile "$ROOT/justfile" aspd "$argument" >/dev/null
    printf '%s\0' compose -f "$compose" exec -T captaind captaind "$argument" > "$temporary/expected"
    cmp "$temporary/expected" "$ARK_TEST_LOG"
done

# Ordinary down must preserve every existing wallet volume.
: > "$ARK_TEST_LOG"
"$script" down >/dev/null
printf '%s\0' compose -f "$compose" --profile cli down > "$temporary/expected"
cmp "$temporary/expected" "$ARK_TEST_LOG"

# An existing mnemonic must cause exactly one read, never creation or a reset.
: > "$ARK_TEST_LOG"; : > "$ARK_TEST_CALLS"
ARK_TEST_MODE=existing "$script" create-bark-wallet
[[ $(wc -l < "$ARK_TEST_CALLS") -eq 1 ]]

# A failed existence check must fail closed instead of attempting creation.
: > "$ARK_TEST_LOG"; : > "$ARK_TEST_CALLS"
if ARK_TEST_MODE=fail "$script" create-bark-wallet >/dev/null 2>&1; then
    echo 'Expected wallet lookup failure' >&2; exit 1
else
    [[ $? -eq 42 ]]
fi
[[ $(wc -l < "$ARK_TEST_CALLS") -eq 1 ]]

# CLI errors survive both entry points.
if ARK_TEST_MODE=fail just --justfile "$ROOT/justfile" bark balance >/dev/null 2>&1; then
    echo 'Expected CLI failure' >&2; exit 1
fi

# Invalid block counts must not reach Docker.
: > "$ARK_TEST_CALLS"
if "$script" generate invalid >/dev/null 2>&1; then
    echo 'Expected invalid block count failure' >&2; exit 1
fi
[[ ! -s "$ARK_TEST_CALLS" ]]

# Chain sync can finish while LND still rejects peer connections during startup.
export ARK_TEST_CONNECT_CALLS="$temporary/connect-calls" ARK_TEST_CHANNEL="$temporary/channel"
ARK_TEST_MODE=lightning-startup "$script" setup-lightning-channels
[[ $(wc -l < "$ARK_TEST_CONNECT_CALLS") -eq 2 ]]
[[ -f "$ARK_TEST_CHANNEL" ]]
echo 'Just/Bash argument forwarding, wallet preservation and failure checks pass'
