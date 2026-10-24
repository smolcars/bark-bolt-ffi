use bark_bolt_ffi::wallet::WalletOpenArgs;
use bark_bolt_ffi::{
    generate_mnemonic, BarkErrorCode, BroadcastOutcome, Config, Network, OnchainSendAmount,
    OnchainWallet, Wallet,
};

fn regtest(args: &[&str]) -> String {
    let output = std::process::Command::new("bash")
        .arg("scripts/ark-dev.sh")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn port(name: &str, default: u16) -> u16 {
    std::env::var(name).map_or(default, |value| value.parse().unwrap())
}

struct Proxy {
    child: std::process::Child,
    port: u16,
}

impl Proxy {
    fn start(upstream_port: u16) -> Self {
        use std::io::BufRead;
        let mut child = std::process::Command::new("python3")
            .arg("tests/support/ark-proxy.py")
            .env("ARK_PORT", upstream_port.to_string())
            .env(
                "RESPONSE_DELAY_SECONDS",
                if upstream_port == port("BITCOIN_PORT", 19443) {
                    "0"
                } else {
                    "0.25"
                },
            )
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        Self {
            child,
            port: line.trim().parse().unwrap(),
        }
    }
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop();
    }
}

fn config(server_port: u16, bitcoin_port: u16) -> Config {
    serde_json::from_value(serde_json::json!({
        "server_address":format!("http://127.0.0.1:{server_port}"),
        "bitcoind_address":format!("http://127.0.0.1:{bitcoin_port}"),
        "bitcoind_user":"second", "bitcoind_pass":"ark",
    }))
    .unwrap()
}

#[test]
fn failed_broadcast_retry_cannot_spend_newly_reserved_inputs() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut proxy = Proxy::start(port("BITCOIN_PORT", 19443));
        let directory = tempfile::tempdir().unwrap();
        let onchain = OnchainWallet::default(
            Network::Regtest,
            generate_mnemonic().unwrap(),
            config(port("ARK_PORT", 3635), proxy.port),
            directory.path().to_str().unwrap().into(),
        )
        .await
        .unwrap();
        let address = onchain.new_address().await.unwrap();
        regtest(&["send-to", &address, "0.001"]);
        onchain.sync().await.unwrap();
        let destination = regtest(&["bcli", "getnewaddress"]);
        let unverifiable = onchain
            .prepare_send(destination.clone(), OnchainSendAmount::DrainAll, 10, true)
            .await
            .unwrap();
        proxy.stop();
        // Confirmed-only cannot be re-checked without the chain source: refuse, record nothing.
        onchain
            .broadcast_prepared(unverifiable.id.clone())
            .await
            .unwrap_err();
        assert!(onchain
            .transactions()
            .await
            .unwrap()
            .iter()
            .all(|tx| tx.txid != unverifiable.txid));
        onchain.discard_prepared(unverifiable.id).await.unwrap();
        // The reservation checks below need a broadcast that is attempted and fails.
        let old = onchain
            .prepare_send(destination.clone(), OnchainSendAmount::DrainAll, 10, false)
            .await
            .unwrap();
        assert!(matches!(
            onchain
                .broadcast_prepared(old.id.clone())
                .await
                .unwrap()
                .broadcast,
            BroadcastOutcome::Failed { .. }
        ));
        // The proxy was stopped before submission; the node has never received this tx.
        onchain.evict_tx(old.txid.clone()).await.unwrap();
        let new = onchain
            .prepare_send(destination, OnchainSendAmount::DrainAll, 10, false)
            .await
            .unwrap();
        assert_eq!(old.inputs, new.inputs);
        assert_eq!(
            onchain
                .broadcast_prepared(old.id.clone())
                .await
                .unwrap_err()
                .code,
            BarkErrorCode::PreparedInputsUnavailable
        );
        onchain.discard_prepared(new.id).await.unwrap();
        assert!(matches!(
            onchain
                .broadcast_prepared(old.id.clone())
                .await
                .unwrap()
                .broadcast,
            BroadcastOutcome::Failed { .. }
        ));
        assert!(onchain
            .transactions()
            .await
            .unwrap()
            .iter()
            .any(|tx| tx.txid == old.txid));
        assert_eq!(
            onchain
                .prepare_send(address, OnchainSendAmount::DrainAll, 10, true)
                .await
                .unwrap_err()
                .code,
            BarkErrorCode::InsufficientFunds
        );
    });
}

#[test]
fn preparation_waits_for_board_cosign_and_excludes_its_funding() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let proxy = Proxy::start(port("ARK_PORT", 3635));
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let settings = config(proxy.port, port("BITCOIN_PORT", 19443));
        let onchain = std::sync::Arc::new(
            OnchainWallet::default(
                Network::Regtest,
                mnemonic.clone(),
                settings.clone(),
                directory.path().to_str().unwrap().into(),
            )
            .await
            .unwrap(),
        );
        // The Ark database deliberately differs from the onchain database.
        let wallet = std::sync::Arc::new(
            Wallet::open_with_onchain(
                Network::Regtest,
                mnemonic,
                settings,
                WalletOpenArgs {
                    run_daemon: false,
                    datadir: directory.path().join("ark").to_str().unwrap().into(),
                    create_if_not_exists: true,
                    create_without_server: false,
                    skip_recovery: true,
                },
                &onchain,
            )
            .await
            .unwrap(),
        );
        regtest(&["send-to", &onchain.new_address().await.unwrap(), "0.005"]);
        onchain.sync().await.unwrap();
        let destination = regtest(&["bcli", "getnewaddress"]);
        let board = {
            let wallet = wallet.clone();
            tokio::spawn(async move { wallet.board_amount(100_000).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let mut prepare = {
            let onchain = onchain.clone();
            tokio::spawn(async move {
                onchain
                    .prepare_send(destination, OnchainSendAmount::DrainAll, 10, false)
                    .await
            })
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut prepare)
                .await
                .is_err()
        );
        let board = board.await.unwrap().unwrap();
        let prepared = prepare.await.unwrap().unwrap();
        let raw = regtest(&["bcli", "getrawtransaction", &board.txid]);
        let funding: bitcoin::Transaction =
            bitcoin::consensus::deserialize(&hex::decode(raw).unwrap()).unwrap();
        assert!(prepared
            .inputs
            .iter()
            .all(|input| funding
                .input
                .iter()
                .all(|i| i.previous_output.txid.to_string() != input.txid
                    || i.previous_output.vout != input.vout)));
        onchain.discard_prepared(prepared.id).await.unwrap();
    });
}

#[test]
fn live_exact_prepared_send_shared_handles_daemon_and_confirmed_drain() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let config = config(port("ARK_PORT", 3635), port("BITCOIN_PORT", 19443));
        let datadir = directory.path().to_str().unwrap().to_owned();
        let onchain = OnchainWallet::default(Network::Regtest, mnemonic.clone(), config.clone(), datadir.clone()).await.unwrap();
        let second = OnchainWallet::default(Network::Regtest, mnemonic.clone(), config.clone(), datadir.clone()).await.unwrap();
        let wallet = Wallet::open_with_onchain(Network::Regtest, mnemonic, config, WalletOpenArgs {
            run_daemon: true, datadir, create_if_not_exists: true, create_without_server: false, skip_recovery: true,
        }, &onchain).await.unwrap();
        let destination = regtest(&["bcli", "getnewaddress"]);
        for rate in [0, u64::MAX, u64::MAX / 250] {
            assert_eq!(onchain.prepare_send(destination.clone(), OnchainSendAmount::DrainAll, rate, true).await.unwrap_err().code, BarkErrorCode::InvalidFeeRate);
        }
        assert_eq!(onchain.prepare_send("invalid".into(), OnchainSendAmount::DrainAll, 10, true).await.unwrap_err().code, BarkErrorCode::InvalidAddress);
        assert_eq!(onchain.broadcast_prepared("unknown".into()).await.unwrap_err().code, BarkErrorCode::PreparedTransactionNotFound);
        let mut outputs = serde_json::Map::new();
        for _ in 0..12 {
            outputs.insert(onchain.new_address().await.unwrap(), serde_json::json!(0.0001));
        }
        regtest(&["bcli", "sendmany", "", &serde_json::Value::Object(outputs).to_string()]);
        regtest(&["generate", "1"]);
        onchain.sync().await.unwrap();
        let prepared = onchain.prepare_send(destination.clone(), OnchainSendAmount::Amount { amount_sats: 105_000 }, 10, true).await.unwrap();
        assert_eq!(prepared.inputs.len(), 12);
        assert!(prepared.fee_sats > 7_000);
        assert_eq!(second.prepare_send(destination.clone(), OnchainSendAmount::DrainAll, 10, true).await.unwrap_err().code, BarkErrorCode::InsufficientFunds);
        assert!(second.send(destination.clone(), 5_000, 10).await.is_err());
        assert!(wallet.board_all().await.is_err());
        let board_info = wallet.board_funding_address().await.unwrap();
        let unsigned = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO, lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn { previous_output: bitcoin::OutPoint {
                txid: prepared.inputs[0].txid.parse().unwrap(), vout: prepared.inputs[0].vout,
            }, ..Default::default() }],
            output: vec![bitcoin::TxOut { value: bitcoin::Amount::from_sat(105_000), script_pubkey: board_info.address.parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>().unwrap().assume_checked().script_pubkey() }],
        };
        let psbt = bitcoin::Psbt::from_unsigned_tx(unsigned).unwrap();
        use base64::Engine;
        assert_eq!(wallet.board_psbt(base64::engine::general_purpose::STANDARD.encode(psbt.serialize()), board_info.keypair_index, board_info.expiry_height).await.unwrap_err().code, BarkErrorCode::PreparedInputsUnavailable);
        onchain.sync().await.unwrap();
        assert!(!onchain.transactions().await.unwrap().iter().any(|t| t.txid == prepared.txid));

        // A real SQLite failure must neither mutate the live graph nor reach the node.
        let connection = rusqlite::Connection::open(directory.path().join("db.sqlite")).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_prepared BEFORE INSERT ON bdk_txs BEGIN SELECT RAISE(ABORT, 'prepared test write failure'); END;").unwrap();
        let failed = second.broadcast_prepared(prepared.id.clone()).await.unwrap();
        assert!(matches!(failed.broadcast, BroadcastOutcome::NotAttempted { .. }));
        connection.execute_batch("DROP TRIGGER fail_prepared;").unwrap();
        onchain.sync().await.unwrap();
        assert!(!onchain.transactions().await.unwrap().iter().any(|t| t.txid == prepared.txid));
        let result = second.broadcast_prepared(prepared.id.clone()).await.unwrap();
        assert!(matches!(result.broadcast, BroadcastOutcome::Accepted), "{:?}", result.broadcast);
        assert_eq!(result.txid, prepared.txid);
        let raw = regtest(&["bcli", "getrawtransaction", &result.txid]);
        let transaction: bitcoin::Transaction = bitcoin::consensus::deserialize(&hex::decode(raw).unwrap()).unwrap();
        assert_eq!(transaction.compute_txid().to_string(), prepared.txid);
        assert_eq!(transaction.vsize() as u64, prepared.vsize);
        assert_eq!(transaction.input.iter().map(|i| (i.previous_output.txid.to_string(), i.previous_output.vout)).collect::<Vec<_>>(),
            prepared.inputs.iter().map(|i| (i.txid.clone(), i.vout)).collect::<Vec<_>>());
        assert_eq!(prepared.fee_sats, 120_000 - transaction.output.iter().map(|o| o.value.to_sat()).sum::<u64>());
        assert_eq!(prepared.amount_sats, 105_000);
        assert_eq!(second.broadcast_prepared(prepared.id.clone()).await.unwrap().txid, prepared.txid);
        onchain.discard_prepared(prepared.id).await.unwrap();
        regtest(&["generate", "1"]);
        onchain.sync().await.unwrap();

        let receive = onchain.new_address().await.unwrap();
        let unconfirmed = regtest(&["bcli", "sendtoaddress", &receive, "0.0002"]);
        onchain.sync().await.unwrap();
        let confirmed = onchain.prepare_send(destination.clone(), OnchainSendAmount::DrainAll, 10, true).await.unwrap();
        assert!(confirmed.inputs.iter().all(|point| point.txid != unconfirmed));
        onchain.discard_prepared(confirmed.id).await.unwrap();
        let total = onchain.balance().await.unwrap().total_sats;
        let all = onchain.prepare_send(destination, OnchainSendAmount::DrainAll, 10, false).await.unwrap();
        assert!(all.inputs.iter().any(|point| point.txid == unconfirmed));
        assert_eq!(all.amount_sats + all.fee_sats, total);
        onchain.discard_prepared(all.id.clone()).await.unwrap();
        assert_eq!(onchain.discard_prepared(all.id).await.unwrap_err().code, BarkErrorCode::PreparedTransactionNotFound);
        wallet.stop_daemon().await.unwrap();
        println!("prepared live tx {}: {} inputs, {} sat fee, {} vB; persistence failure, shared handles, daemon, confirmed-only and discard passed", result.txid, prepared.inputs.len(), prepared.fee_sats, prepared.vsize);
    });
}
