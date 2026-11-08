#!/usr/bin/env bash
# Adapted from Nitro Ark's ark-dev.sh (MIT; see regtest/LICENSE).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE_FILE="$SCRIPT_DIR/../regtest/compose.yaml"
WALLET_NAME=miner

dcr() { docker compose -f "$COMPOSE_FILE" "$@"; }
bitcoin() { dcr exec -T bitcoind bitcoin-cli -regtest -rpcuser=second -rpcpassword=ark "$@"; }
bcli() { bitcoin "-rpcwallet=$WALLET_NAME" "$@"; }
aspd() { dcr exec -T captaind captaind "$@"; }
lncli() { dcr exec -T lnd lncli --network=regtest "$@"; }
cln() { dcr exec -T cln lightning-cli --regtest "$@"; }
bark() { dcr run --rm -T bark --datadir /wallets/reference --no-logfile "$@"; }

die() { printf '%s\n' "$*" >&2; exit 1; }

wait_for() {
    local label="$1" output='' deadline=$((SECONDS + 120))
    shift
    while (( SECONDS < deadline )); do
        if output=$("$@" 2>&1); then
            printf '%s\n' "$output"
            return
        fi
        sleep 1
    done
    die "$label not ready after 120s: $output"
}

create_wallet() {
    if bitcoin listwallets | jq -e --arg name "$WALLET_NAME" 'index($name) != null' >/dev/null; then
        return
    fi
    if bitcoin listwalletdir | jq -e --arg name "$WALLET_NAME" 'any(.wallets[]; .name == $name)' >/dev/null; then
        bitcoin loadwallet "$WALLET_NAME" >/dev/null
    else
        bitcoin createwallet "$WALLET_NAME" >/dev/null
    fi
}

generate_blocks() {
    local blocks="${1:-101}" address
    [[ "$blocks" =~ ^[0-9]+$ ]] || die 'Provide a nonnegative block count'
    create_wallet
    address=$(bcli getnewaddress)
    bcli generatetoaddress "$blocks" "$address"
}

send_to_address() {
    [[ $# == 2 ]] || die 'Usage: send-to ADDRESS BTC_AMOUNT'
    create_wallet
    bcli sendtoaddress "$1" "$2"
    generate_blocks 1 >/dev/null
}

fund_aspd() {
    [[ $# == 1 ]] || die 'Usage: fund-aspd BTC_AMOUNT'
    local address
    address=$(aspd rpc wallet | jq -er '.rounds.address')
    send_to_address "$address" "$1"
}

create_bark_wallet() {
    local exists
    # Retain existing seeds; never copy the original script's forced reset.
    exists=$(dcr run --rm -T --entrypoint sh bark -c \
        'if test -f /wallets/reference/mnemonic; then echo yes; else echo no; fi')
    case "$exists" in
        yes) return ;;
        no) bark create --regtest --ark http://captaind:3535 \
            --bitcoind http://bitcoind:18443 --bitcoind-user second \
            --bitcoind-pass ark >/dev/null ;;
        *) die 'Could not check the reference wallet; refusing to create it' ;;
    esac
}

lnd_synced() { lncli getinfo | jq -e '.synced_to_chain == true'; }

lnd_at_tip() {
    local tip
    tip=$(bitcoin getblockcount)
    lncli getinfo | jq -e --argjson tip "$tip" '.synced_to_chain and .block_height == $tip'
}

# CLN offers HTLCs from its own height; paying before it reaches the tip after a
# large mine can give an HTLC an already-passed deadline and force-close the channel.
lightning_at_tip() {
    local tip
    tip=$(bitcoin getblockcount)
    lncli getinfo | jq -e --argjson tip "$tip" '.synced_to_chain and .block_height == $tip' >/dev/null &&
        cln getinfo | jq -e --argjson tip "$tip" '.blockheight == $tip'
}

lnd_funded() { lncli walletbalance | jq -e '.confirmed_balance | tonumber >= 2000000'; }

channel_active() {
    lncli listchannels | jq -e --arg peer "$1" 'any(.channels[]; .active and .remote_pubkey == $peer)'
}

setup_lightning_channels() {
    local peer_info peer port
    # A reused, funded stack still needs a recent tip for LND readiness.
    generate_blocks 1 >/dev/null
    wait_for 'LND chain sync' lnd_synced >/dev/null
    peer_info=$(wait_for CLN cln getinfo)
    peer=$(jq -er '.id' <<<"$peer_info")
    port=$(jq -er '.binding[0].port' <<<"$peer_info")
    if ! lncli listchannels | jq -e --arg peer "$peer" 'any(.channels[]; .remote_pubkey == $peer)' >/dev/null; then
        if lncli pendingchannels | jq -e '.pending_open_channels | length == 0' >/dev/null; then
            send_to_address "$(lncli newaddress p2wkh | jq -er '.address')" 1 >/dev/null
            send_to_address "$(cln newaddr | jq -er '.bech32')" 1 >/dev/null
            generate_blocks 6 >/dev/null
            wait_for 'LND funding' lnd_funded >/dev/null
            wait_for 'LND chain sync after mining' lnd_at_tip >/dev/null
            if ! lncli listpeers | jq -e --arg peer "$peer" 'any(.peers[]; .pub_key == $peer)' >/dev/null; then
                wait_for 'LND peer connection' lncli connect "$peer@cln:$port" >/dev/null
            fi
            lncli openchannel "--node_key=$peer" --local_amt=10000000 --push_amt=5000000 >/dev/null
        fi
        generate_blocks 6 >/dev/null
    fi
    wait_for 'Lightning channel' channel_active "$peer" >/dev/null
}

bark_funded() { bark balance | jq -e '.spendable_sat >= 100000'; }

setup_everything() {
    dcr up -d
    wait_for Bitcoin bitcoin getblockchaininfo >/dev/null
    create_wallet
    if bcli getbalance | jq -e '. < 50' >/dev/null; then
        generate_blocks 110 >/dev/null
    fi
    local wallet
    wallet=$(wait_for 'Ark server' aspd rpc wallet)
    if jq -e '.rounds.trusted_balance < 100000000' <<<"$wallet" >/dev/null; then
        fund_aspd 25 >/dev/null
        generate_blocks 6 >/dev/null
    fi
    setup_lightning_channels
    create_bark_wallet
    if ! bark_funded >/dev/null; then
        send_to_address "$(bark onchain address | jq -er '.address')" 0.05 >/dev/null
        generate_blocks 6 >/dev/null
        bark board 1000000sat >/dev/null
        generate_blocks 6 >/dev/null
    fi
    wait_for 'Reference Bark balance' bark_funded >/dev/null
    jq -n --argjson height "$(bitcoin getblockcount)" \
        --argjson channels "$(lncli listchannels | jq '.channels | length')" \
        --argjson balance "$(bark balance)" \
        '{bitcoin_height: $height, lightning_channels: $channels, reference_balance: $balance}'
}

usage() {
    cat <<'EOF'
Usage: scripts/ark-dev.sh COMMAND [ARGS...]

  setup-everything             Start/fund Bitcoin, Ark, Bark and Lightning
  up | stop | down | status    Manage this repository's Compose project
  logs [ARGS...]               Show Compose logs
  create-wallet               Create/load the existing miner wallet
  create-bark-wallet          Create the reference wallet only if absent
  generate [BLOCKS=101]        Mine blocks
  send-to ADDRESS BTC_AMOUNT  Fund an address and mine one block
  fund-aspd BTC_AMOUNT        Fund the Ark server and mine one block
  setup-lightning-channels    Reuse/create the LND-to-CLN channel
  wait-lightning-sync         Wait until LND and CLN reach the Bitcoin tip
  bcli | aspd | bark | lncli | cln [ARGS...]  Run the corresponding CLI

Ordinary down preserves volumes. COMPOSE_PROJECT_NAME, BITCOIN_PORT and ARK_PORT
select an isolated CI stack; the default is the existing bark-bolt-ffi stack.
EOF
}

command="${1:---help}"
[[ $# == 0 ]] || shift
case "$command" in
    --help|-h|help) usage ;;
    setup-everything) setup_everything ;;
    up|stop) dcr "$command" "$@" ;;
    down) dcr --profile cli down "$@" ;;
    status) dcr ps "$@" ;;
    logs) dcr logs --tail=100 "$@" ;;
    create-wallet) create_wallet ;;
    create-bark-wallet) create_bark_wallet ;;
    generate) generate_blocks "$@" ;;
    send-to) send_to_address "$@" ;;
    fund-aspd) fund_aspd "$@" ;;
    setup-lightning-channels) setup_lightning_channels ;;
    wait-lightning-sync) wait_for 'Lightning chain sync' lightning_at_tip >/dev/null ;;
    bcli|aspd|bark|lncli|cln) "$command" "$@" ;;
    *) usage >&2; die "Unknown command: $command" ;;
esac
