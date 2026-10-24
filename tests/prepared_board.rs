use bark_bolt_ffi::wallet::WalletOpenArgs;
use bark_bolt_ffi::{
    generate_mnemonic, BarkErrorCode, BroadcastOutcome, Config, ExitState, ExitTxStatus, Network,
    OnchainSendAmount, OnchainUtxo, OnchainWallet, OutPoint, TransactionStatus, Wallet,
};
use std::collections::BTreeMap;
use std::path::Path;

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
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn config() -> Config {
    let port = |name: &str, default: &str| std::env::var(name).unwrap_or(default.into());
    serde_json::from_value(serde_json::json!({"server_address":format!("http://127.0.0.1:{}", port("ARK_PORT", "3635")), "bitcoind_address":format!("http://127.0.0.1:{}", port("BITCOIN_PORT", "19443")), "bitcoind_user":"second", "bitcoind_pass":"ark", "vtxo_exit_margin":2})).unwrap()
}
fn args(path: &Path) -> WalletOpenArgs {
    WalletOpenArgs {
        run_daemon: false,
        datadir: path.to_str().unwrap().into(),
        create_if_not_exists: true,
        create_without_server: false,
        skip_recovery: true,
    }
}
fn files(path: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_str().unwrap().into(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect()
}

#[test]
fn live_prepared_board_exact_fees_reservations_status_and_cancel_reopen() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let onchain = OnchainWallet::default(Network::Regtest,mnemonic.clone(),config(),directory.path().to_str().unwrap().into()).await.unwrap();
        let wallet = Wallet::open_with_onchain(Network::Regtest,mnemonic.clone(),config(),args(directory.path()),&onchain).await.unwrap();
        let address = onchain.new_address().await.unwrap();
        for _ in 0..12 { regtest(&["send-to",&address,"0.0001"]); }
        onchain.sync().await.unwrap();
        let before = files(directory.path());
        for rate in [0,u64::MAX,u64::MAX/500] {
            assert_eq!(wallet.prepare_board(OnchainSendAmount::DrainAll,rate,true, None).await.unwrap_err().code,BarkErrorCode::InvalidFeeRate);
        }
        assert_eq!(wallet.prepare_board(OnchainSendAmount::Amount {amount_sats:1},10,true, None).await.unwrap_err().code,BarkErrorCode::BelowMinimumBoardAmount);
        assert_eq!(wallet.prepare_board(OnchainSendAmount::Amount {amount_sats:500_000},10,true, None).await.unwrap_err().code,BarkErrorCode::InsufficientFunds);
        assert_eq!(files(directory.path()),before);
        let preview = wallet.prepare_board(OnchainSendAmount::DrainAll,10,true, None).await.unwrap();
        assert_eq!(preview.inputs.len(),12);
        assert_eq!(preview.gross_sats+preview.onchain_fee_sats,120_000);
        assert_eq!(preview.net_sats+preview.ark_fee_sats,preview.gross_sats);
        assert!(preview.onchain_fee_sats>=preview.vsize*10);
        assert_eq!(files(directory.path()),before);
        assert_eq!(wallet.transaction_status(preview.txid.clone()).await.unwrap(),TransactionStatus::NotFound);
        assert!(wallet.pending_boards().await.unwrap().is_empty());
        assert!(onchain.transactions().await.unwrap().iter().all(|tx|tx.txid!=preview.txid));
        let destination = regtest(&["bcli","getnewaddress"]);
        assert_eq!(onchain.prepare_send(destination,OnchainSendAmount::DrainAll,10,true).await.unwrap_err().code,BarkErrorCode::InsufficientFunds);
        assert_eq!(onchain.broadcast_prepared(preview.id.clone()).await.unwrap_err().code,BarkErrorCode::PreparedTransactionNotFound);
        assert_eq!(onchain.discard_prepared(preview.id.clone()).await.unwrap_err().code,BarkErrorCode::PreparedTransactionNotFound);
        assert!(wallet.board_all().await.is_err());
        wallet.discard_prepared_board(preview.id.clone()).await.unwrap();
        assert_eq!(wallet.submit_prepared_board(preview.id).await.unwrap_err().code,BarkErrorCode::PreparedTransactionNotFound);
        // Legacy board_all above reveals a key; take a fresh no-write snapshot.
        let before = files(directory.path());
        let prepared = wallet.prepare_board(OnchainSendAmount::DrainAll,10,true, None).await.unwrap();
        assert_eq!(files(directory.path()),before);
        let connection = rusqlite::Connection::open(directory.path().join("db.sqlite")).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_board BEFORE INSERT ON bark_movements BEGIN SELECT RAISE(ABORT, 'board movement write failure'); END;").unwrap();
        let not_attempted = wallet.submit_prepared_board(prepared.id.clone()).await.unwrap();
        assert!(matches!(not_attempted.broadcast, BroadcastOutcome::NotAttempted { .. }));
        assert_eq!(wallet.transaction_status(prepared.txid.clone()).await.unwrap(), TransactionStatus::NotFound);
        assert!(wallet.pending_boards().await.unwrap().is_empty());
        connection.execute_batch("DROP TRIGGER fail_board; CREATE TRIGGER fail_board_cache BEFORE INSERT ON bdk_txs BEGIN SELECT RAISE(ABORT, 'board funding cache write failure'); END;").unwrap();
        let cache_failed = wallet.submit_prepared_board(prepared.id.clone()).await.unwrap();
        assert!(matches!(cache_failed.broadcast, BroadcastOutcome::Failed { ref reason } if reason.contains("funding cache write failed")));
        assert_eq!(wallet.discard_prepared_board(prepared.id.clone()).await.unwrap_err().code, BarkErrorCode::Persistence);
        let destination = regtest(&["bcli", "getnewaddress"]);
        assert!(onchain.drain(destination.clone(), Some(1)).await.is_err());
        connection.execute_batch("DROP TRIGGER fail_board_cache").unwrap();
        let result = wallet.submit_prepared_board(prepared.id.clone()).await.unwrap();
        assert!(onchain.transactions().await.unwrap().iter().any(|tx| tx.txid == result.txid));
        // A local eviction must not let legacy builders reuse pending funding.
        onchain.evict_tx(result.txid.clone()).await.unwrap();
        assert!(onchain.drain(destination.clone(), Some(1)).await.is_err());
        assert!(onchain.send(destination, 1000, 100).await.is_err());
        assert!(wallet.board_all().await.is_err());
        onchain.sync().await.unwrap();
        assert_eq!(result.txid,prepared.txid);
        assert!(matches!(result.broadcast,BroadcastOutcome::Accepted));
        assert_eq!(result.vtxo_ids.len(),1);
        assert_eq!(onchain.transaction_status(result.txid.clone()).await.unwrap(),TransactionStatus::InMempool);
        let raw:serde_json::Value = serde_json::from_str(&regtest(&["bcli","getrawtransaction",&result.txid,"true"])).unwrap();
        assert_eq!(raw["vsize"].as_u64(),Some(prepared.vsize));
        assert_eq!(raw["vin"].as_array().unwrap().len(),prepared.inputs.len());
        for (input, expected) in raw["vin"].as_array().unwrap().iter().zip(&prepared.inputs) {
            assert_eq!(input["txid"].as_str(),Some(expected.txid.as_str()));
            assert_eq!(input["vout"].as_u64(),Some(expected.vout as u64));
        }
        let outputs:u64=raw["vout"].as_array().unwrap().iter().map(|output|(output["value"].as_f64().unwrap()*100_000_000.0).round() as u64).sum();
        assert_eq!(120_000-outputs,prepared.onchain_fee_sats);
        let count = wallet.history().await.unwrap().len();
        assert!(matches!(wallet.submit_prepared_board(prepared.id.clone()).await.unwrap().broadcast,BroadcastOutcome::Accepted));
        assert_eq!(wallet.history().await.unwrap().len(),count);
        wallet.discard_prepared_board(prepared.id).await.unwrap();
        regtest(&["generate","6"]);
        assert!(matches!(wallet.transaction_status(result.txid).await.unwrap(),TransactionStatus::Confirmed {..}));
        wallet.sync().await.unwrap();
        assert_eq!(wallet.balance().await.unwrap().spendable_sats,prepared.net_sats);
        let id=result.vtxo_ids[0].clone();
        wallet.start_exit_for_vtxos(vec![id.clone()]).await.unwrap();
        assert!(wallet.cancel_exit(id.clone()).await.unwrap().canceled);
        assert!(wallet.get_exit_vtxos().await.unwrap().is_empty());
        drop(wallet); drop(onchain);
        let onchain = OnchainWallet::default(Network::Regtest,mnemonic.clone(),config(),directory.path().to_str().unwrap().into()).await.unwrap();
        let wallet = Wallet::open_with_onchain(Network::Regtest,mnemonic,config(),args(directory.path()),&onchain).await.unwrap();
        assert!(wallet.get_exit_vtxos().await.unwrap().is_empty());
        let statuses=wallet.list_all_exits(true,true).await.unwrap();
        let status=statuses.iter().find(|status|status.vtxo_id==id).unwrap();
        assert_eq!(status.amount_sats,prepared.net_sats);
        assert!(matches!(status.state,ExitState::Canceled {..}));
        assert!(!status.history.as_ref().unwrap().is_empty());
        assert!(!status.transactions.is_empty());
        let compact=wallet.list_all_exits(false,false).await.unwrap().remove(0);
        assert!(compact.history.is_none()); assert!(compact.transactions.is_empty());
        println!("prepared board {}: {} inputs, {} net sats, {} Ark fee, {} onchain fee, {} vB; cancel/reopen passed",prepared.txid,prepared.inputs.len(),prepared.net_sats,prepared.ark_fee_sats,prepared.onchain_fee_sats,prepared.vsize);
    });
}

#[test]
fn live_board_rejects_external_spend_and_claimed_exit_survives_reopen() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let onchain = OnchainWallet::default(
            Network::Regtest,
            mnemonic.clone(),
            config(),
            directory.path().to_str().unwrap().into(),
        )
        .await
        .unwrap();
        let wallet = Wallet::open_with_onchain(
            Network::Regtest,
            mnemonic.clone(),
            config(),
            args(directory.path()),
            &onchain,
        )
        .await
        .unwrap();
        let address = onchain.new_address().await.unwrap();
        regtest(&["send-to", &address, "0.005"]);
        onchain.sync().await.unwrap();
        let independent = tempfile::tempdir().unwrap();
        let other = OnchainWallet::default(
            Network::Regtest,
            mnemonic.clone(),
            config(),
            independent.path().to_str().unwrap().into(),
        )
        .await
        .unwrap();
        other.new_address().await.unwrap();
        other.sync().await.unwrap();
        let preview = wallet
            .prepare_board(OnchainSendAmount::DrainAll, 1, true, None)
            .await
            .unwrap();
        let destination = regtest(&["bcli", "getnewaddress"]);
        other.drain(destination, None).await.unwrap();
        assert_eq!(
            wallet
                .submit_prepared_board(preview.id.clone())
                .await
                .unwrap_err()
                .code,
            BarkErrorCode::PreparedInputsUnavailable
        );
        assert!(wallet.pending_boards().await.unwrap().is_empty());
        wallet.discard_prepared_board(preview.id).await.unwrap();
        regtest(&["generate", "1"]);
        onchain.sync().await.unwrap();
        regtest(&["send-to", &address, "0.005"]);
        onchain.sync().await.unwrap();
        let preview = wallet
            .prepare_board(
                OnchainSendAmount::Amount {
                    amount_sats: 100_000,
                },
                1,
                true,
                None,
            )
            .await
            .unwrap();
        let result = wallet
            .submit_prepared_board(preview.id.clone())
            .await
            .unwrap();
        assert!(matches!(result.broadcast, BroadcastOutcome::Accepted));
        wallet.discard_prepared_board(preview.id).await.unwrap();
        regtest(&["generate", "6"]);
        wallet.sync().await.unwrap();
        onchain.sync().await.unwrap();
        let id = result.vtxo_ids[0].clone();
        let reserved = wallet.prepare_board(OnchainSendAmount::DrainAll, 1, true, None).await.unwrap();
        let before_cpfp: Vec<_> = onchain.transactions().await.unwrap().into_iter().map(|tx| tx.txid).collect();
        wallet.start_exit_for_vtxos(vec![id.clone()]).await.unwrap();
        let blocked = wallet.progress_exits(Some(1)).await.unwrap();
        assert!(blocked.iter().any(|status| matches!(&status.state, ExitState::Processing { transactions, .. }
            if transactions.iter().any(|tx| matches!(tx.status, ExitTxStatus::AwaitingCpfpBroadcast)))));
        let after_cpfp: Vec<_> = onchain.transactions().await.unwrap().into_iter().map(|tx| tx.txid).collect();
        assert_eq!(before_cpfp, after_cpfp, "exit CPFP must not spend reserved board inputs");
        wallet.discard_prepared_board(reserved.id).await.unwrap();
        for _ in 0..30 {
            wallet.progress_exits(Some(1)).await.unwrap();
            if !wallet.list_claimable_exits().await.unwrap().is_empty() {
                break;
            }
            regtest(&["generate", "2"]);
            wallet.sync_exits().await.unwrap();
        }
        assert!(!wallet.list_claimable_exits().await.unwrap().is_empty());
        let destination = regtest(&["bcli", "getnewaddress"]);
        let claim = wallet
            .drain_exits(Vec::new(), true, destination, Some(1))
            .await
            .unwrap();
        let signed = wallet
            .sign_exit_claim_inputs(claim.psbt_base64)
            .await
            .unwrap();
        let txid = wallet
            .broadcast_tx(bark_bolt_ffi::extract_tx_from_psbt(signed).unwrap())
            .await
            .unwrap();
        regtest(&["generate", "2"]);
        wallet.sync_exits().await.unwrap();
        drop(wallet);
        drop(onchain);
        drop(other);
        let onchain = OnchainWallet::default(
            Network::Regtest,
            mnemonic.clone(),
            config(),
            directory.path().to_str().unwrap().into(),
        )
        .await
        .unwrap();
        let wallet = Wallet::open_with_onchain(
            Network::Regtest,
            mnemonic,
            config(),
            args(directory.path()),
            &onchain,
        )
        .await
        .unwrap();
        assert!(wallet.get_exit_vtxos().await.unwrap().is_empty());
        let exits = wallet.list_all_exits(true, true).await.unwrap();
        let exit = exits.iter().find(|exit| exit.vtxo_id == id).unwrap();
        assert!(matches!(exit.state, ExitState::Claimed { .. }));
        assert_eq!(exit.amount_sats, preview.net_sats);
        assert!(!exit.history.as_ref().unwrap().is_empty());
        assert!(!exit.transactions.is_empty());
        assert!(matches!(
            wallet.transaction_status(txid).await.unwrap(),
            TransactionStatus::Confirmed { .. }
        ));
        println!("external-spend rejection and real claimed-exit history/reopen passed");
    });
}

/// A reorg must never rewind the Ark/Lightning stack used by other contracts.
struct ReorgNode {
    name: String,
    port: u16,
}
impl ReorgNode {
    fn start() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let name = format!(
            "bark-bolt-ffi-reorg-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let output=std::process::Command::new("docker").args(["run","--rm","-d","--name",&name,"-p","127.0.0.1::18443",
            "bitcoin/bitcoin@sha256:da25cedc66b1daefff9f412ee196c901a899c3fa68a33b20849c3e08b5c40d63",
            "-regtest","-server","-txindex","-blockfilterindex","-rpcbind=0.0.0.0","-rpcallowip=0.0.0.0/0","-rpcuser=second","-rpcpassword=ark","-fallbackfee=0.00001"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Own cleanup before any readiness assertion can fail.
        let mut node = Self { name, port: 0 };
        let port = std::process::Command::new("docker")
            .args(["port", &node.name, "18443/tcp"])
            .output()
            .unwrap();
        node.port = String::from_utf8(port.stdout)
            .unwrap()
            .trim()
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        for _ in 0..100 {
            if node.raw(&["getblockcount"]).status.success() {
                node.rpc(&["createwallet", "miner"]);
                node.mine(101);
                return node;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        panic!("disposable Bitcoin node did not become ready");
    }
    fn raw(&self, args: &[&str]) -> std::process::Output {
        std::process::Command::new("docker")
            .args([
                "exec",
                &self.name,
                "bitcoin-cli",
                "-regtest",
                "-rpcuser=second",
                "-rpcpassword=ark",
                "-rpcwallet=miner",
            ])
            .args(args)
            .output()
            .unwrap()
    }
    fn rpc(&self, args: &[&str]) -> String {
        let result = self.raw(args);
        assert!(
            result.status.success(),
            "Bitcoin {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().into()
    }
    fn mine(&self, blocks: u32) {
        self.rpc(&[
            "generatetoaddress",
            &blocks.to_string(),
            &self.rpc(&["getnewaddress"]),
        ]);
    }
}
impl Drop for ReorgNode {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

#[test]
fn live_prepared_board_rejects_reorg_spend_below_cached_tip() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    let node = ReorgNode::start();
    let mut config = config();
    config.bitcoind_address = Some(format!("http://127.0.0.1:{}", node.port));
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let onchain = OnchainWallet::default(
            Network::Regtest,
            mnemonic.clone(),
            config.clone(),
            directory.path().to_str().unwrap().into(),
        )
        .await
        .unwrap();
        let wallet = Wallet::open_with_onchain(
            Network::Regtest,
            mnemonic.clone(),
            config.clone(),
            args(directory.path()),
            &onchain,
        )
        .await
        .unwrap();
        let address = onchain.new_address().await.unwrap();
        node.rpc(&["sendtoaddress", &address, "0.005"]);
        node.mine(1);
        let funding_height: u32 = node.rpc(&["getblockcount"]).parse().unwrap();
        node.mine(5);
        onchain.sync().await.unwrap();
        let preview = wallet
            .prepare_board(OnchainSendAmount::DrainAll, 1, true, None)
            .await
            .unwrap();
        let invalidated = node.rpc(&["getblockhash", &(funding_height + 1).to_string()]);
        node.rpc(&["invalidateblock", &invalidated]);
        let other_dir = tempfile::tempdir().unwrap();
        let other = OnchainWallet::default(
            Network::Regtest,
            mnemonic,
            config,
            other_dir.path().to_str().unwrap().into(),
        )
        .await
        .unwrap();
        other.new_address().await.unwrap();
        other.sync().await.unwrap();
        let destination = node.rpc(&["getnewaddress"]);
        let competing = other.drain(destination, Some(2)).await.unwrap();
        assert_eq!(
            other.transaction_status(competing.clone()).await.unwrap(),
            TransactionStatus::InMempool
        );
        node.mine(7);
        let raw: serde_json::Value =
            serde_json::from_str(&node.rpc(&["getrawtransaction", &competing, "true"])).unwrap();
        assert!(raw["confirmations"].as_u64().unwrap() >= 7);
        assert_eq!(
            wallet
                .submit_prepared_board(preview.id.clone())
                .await
                .unwrap_err()
                .code,
            BarkErrorCode::PreparedInputsUnavailable
        );
        assert!(wallet.pending_boards().await.unwrap().is_empty());
        wallet.discard_prepared_board(preview.id).await.unwrap();
        println!(
            "reorg spend below the preparation's cached tip was rejected before board creation"
        );
        let mut recipients = serde_json::Map::new();
        for _ in 0..101 {
            recipients.insert(
                onchain.new_address().await.unwrap(),
                serde_json::json!(0.00001),
            );
        }
        node.rpc(&[
            "sendmany",
            "",
            &serde_json::Value::Object(recipients).to_string(),
        ]);
        node.mine(1);
        onchain.sync().await.unwrap();
        let before = files(directory.path());
        assert_eq!(
            wallet
                .prepare_board(OnchainSendAmount::DrainAll, 1, true, None)
                .await
                .unwrap_err()
                .code,
            BarkErrorCode::TooManyInputs
        );
        assert_eq!(
            files(directory.path()),
            before,
            "101-input failure must leave SQLite unchanged"
        );
        let spend = onchain
            .prepare_send(
                node.rpc(&["getnewaddress"]),
                OnchainSendAmount::Amount { amount_sats: 1000 },
                1,
                true,
            )
            .await
            .unwrap();
        onchain.discard_prepared(spend.id).await.unwrap();
        println!("101-input board rejection left no write or leaked reservation");
    });
}

#[test]
fn live_prepared_board_never_spends_excluded_outpoints() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let onchain = OnchainWallet::default(
            Network::Regtest,
            mnemonic.clone(),
            config(),
            directory.path().to_str().unwrap().into(),
        )
        .await
        .unwrap();
        let wallet = Wallet::open_with_onchain(
            Network::Regtest,
            mnemonic,
            config(),
            args(directory.path()),
            &onchain,
        )
        .await
        .unwrap();
        let address = onchain.new_address().await.unwrap();
        regtest(&["send-to", &address, "0.002"]);
        regtest(&["send-to", &address, "0.0006"]);
        regtest(&["generate", "1"]);
        onchain.sync().await.unwrap();
        let utxos = onchain.utxos().await.unwrap();
        let utxo = |sats: u64| {
            utxos
                .iter()
                .find_map(|utxo| match utxo {
                    OnchainUtxo::Local {
                        outpoint,
                        amount_sats,
                        ..
                    } if *amount_sats == sats => Some(outpoint.clone()),
                    _ => None,
                })
                .unwrap()
        };
        let (big, small) = (utxo(200_000), utxo(60_000));
        let amount = OnchainSendAmount::Amount {
            amount_sats: 50_000,
        };
        // Largest-first selection is what an unconstrained board would spend.
        let unconstrained = wallet
            .prepare_board(amount.clone(), 1, true, None)
            .await
            .unwrap();
        assert!(
            unconstrained.inputs.contains(&big),
            "{:?}",
            unconstrained.inputs
        );
        wallet
            .discard_prepared_board(unconstrained.id)
            .await
            .unwrap();
        let ignored = vec![
            big.clone(),
            OutPoint {
                txid: "not a txid".into(),
                vout: 0,
            },
            OutPoint {
                txid: "11".repeat(32),
                vout: 3,
            },
        ];
        let excluded = wallet
            .prepare_board(amount, 1, true, Some(ignored.clone()))
            .await
            .unwrap();
        assert_eq!(excluded.inputs, vec![small.clone()]);
        wallet.discard_prepared_board(excluded.id).await.unwrap();
        let drained = wallet
            .prepare_board(OnchainSendAmount::DrainAll, 1, true, Some(ignored.clone()))
            .await
            .unwrap();
        assert_eq!(drained.inputs, vec![small]);
        wallet.discard_prepared_board(drained.id).await.unwrap();
        let too_much = OnchainSendAmount::Amount {
            amount_sats: 100_000,
        };
        assert_eq!(
            wallet
                .prepare_board(too_much, 1, true, Some(ignored))
                .await
                .unwrap_err()
                .code,
            BarkErrorCode::InsufficientFunds
        );
    });
}

/// Fund one coin on a disposable node, then let `check` prepare, reorg the
/// funding block back into the mempool and submit.
fn with_unconfirmed_funding_after_reorg<F>(check: F)
where
    F: for<'a> FnOnce(
        &'a ReorgNode,
        &'a Wallet,
        &'a OnchainWallet,
        Box<dyn FnOnce() + 'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>,
{
    let node = ReorgNode::start();
    let mut config = config();
    config.bitcoind_address = Some(format!("http://127.0.0.1:{}", node.port));
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let onchain = OnchainWallet::default(
            Network::Regtest,
            mnemonic.clone(),
            config.clone(),
            directory.path().to_str().unwrap().into(),
        )
        .await
        .unwrap();
        let wallet = Wallet::open_with_onchain(
            Network::Regtest,
            mnemonic,
            config,
            args(directory.path()),
            &onchain,
        )
        .await
        .unwrap();
        let address = onchain.new_address().await.unwrap();
        node.rpc(&["sendtoaddress", &address, "0.005"]);
        node.mine(1);
        let funding_height: u32 = node.rpc(&["getblockcount"]).parse().unwrap();
        node.mine(5);
        onchain.sync().await.unwrap();
        let node_ref = &node;
        let reorg = Box::new(move || {
            // Disconnecting the funding block returns its transaction to the mempool.
            let hash = node_ref.rpc(&["getblockhash", &funding_height.to_string()]);
            node_ref.rpc(&["invalidateblock", &hash]);
            // A longer chain that leaves the funding transaction unconfirmed.
            let miner = node_ref.rpc(&["getnewaddress"]);
            for _ in 0..7 {
                node_ref.rpc(&["generateblock", &miner, "[]"]);
            }
            let raw: serde_json::Value =
                serde_json::from_str(&node_ref.rpc(&["getrawmempool"])).unwrap();
            assert_eq!(raw.as_array().unwrap().len(), 1);
        });
        check(&node, &wallet, &onchain, reorg).await;
    });
}

#[test]
fn live_confirmed_only_board_rejects_parent_reorged_to_mempool() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    with_unconfirmed_funding_after_reorg(|_, wallet, _, reorg| {
        Box::pin(async move {
            let preview = wallet
                .prepare_board(OnchainSendAmount::DrainAll, 1, true, None)
                .await
                .unwrap();
            reorg();
            let error = wallet
                .submit_prepared_board(preview.id.clone())
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                BarkErrorCode::PreparedInputsUnavailable,
                "{error:?}"
            );
            assert!(wallet.pending_boards().await.unwrap().is_empty());
            assert!(wallet.history().await.unwrap().is_empty());
            wallet.discard_prepared_board(preview.id).await.unwrap();
        })
    });
}

#[test]
fn live_confirmed_only_send_rejects_parent_reorged_to_mempool() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    with_unconfirmed_funding_after_reorg(|node, _, onchain, reorg| {
        Box::pin(async move {
            let destination = node.rpc(&["getnewaddress"]);
            let prepared = onchain
                .prepare_send(destination, OnchainSendAmount::DrainAll, 1, true)
                .await
                .unwrap();
            reorg();
            let error = onchain
                .broadcast_prepared(prepared.id.clone())
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                BarkErrorCode::PreparedInputsUnavailable,
                "{error:?}"
            );
            assert!(onchain
                .transactions()
                .await
                .unwrap()
                .iter()
                .all(|tx| tx.txid != prepared.txid));
            // The refusal is a re-check, not a consumed preparation: re-confirming the parent lets it through.
            node.mine(1);
            let sent = onchain
                .broadcast_prepared(prepared.id.clone())
                .await
                .unwrap();
            assert!(
                matches!(sent.broadcast, BroadcastOutcome::Accepted),
                "{:?}",
                sent.broadcast
            );
            // A second reorg after submission plus a local eviction must not block rebroadcast.
            node.rpc(&["invalidateblock", &node.rpc(&["getbestblockhash"])]);
            onchain.evict_tx(prepared.txid.clone()).await.unwrap();
            onchain
                .broadcast_prepared(prepared.id.clone())
                .await
                .unwrap();
            assert!(onchain
                .transactions()
                .await
                .unwrap()
                .iter()
                .any(|tx| tx.txid == prepared.txid));
            onchain.discard_prepared(prepared.id).await.unwrap();
        })
    });
}

#[test]
fn live_board_failing_after_movement_write_is_uncertain_and_not_resubmitted() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let onchain = OnchainWallet::default(Network::Regtest, mnemonic.clone(), config(), directory.path().to_str().unwrap().into()).await.unwrap();
        let wallet = Wallet::open_with_onchain(Network::Regtest, mnemonic, config(), args(directory.path()), &onchain).await.unwrap();
        regtest(&["send-to", &onchain.new_address().await.unwrap(), "0.001"]);
        regtest(&["generate", "1"]);
        onchain.sync().await.unwrap();
        let prepared = wallet.prepare_board(OnchainSendAmount::DrainAll, 1, true, None).await.unwrap();
        let connection = rusqlite::Connection::open(directory.path().join("db.sqlite")).unwrap();
        // Bark stores the BOARD movement, then fails to store its checkpoint.
        connection.execute_batch("CREATE TRIGGER fail_checkpoint BEFORE INSERT ON bark_wallet_action_checkpoint BEGIN SELECT RAISE(ABORT, 'board checkpoint write failure'); END;").unwrap();
        let result = wallet.submit_prepared_board(prepared.id.clone()).await.unwrap();
        assert!(matches!(result.broadcast, BroadcastOutcome::Uncertain { ref reason } if reason.contains("board checkpoint write failure")), "{:?}", result.broadcast);
        assert!(result.vtxo_ids.is_empty());
        let boards = || async {
            wallet.history().await.unwrap().into_iter().filter(|movement| movement.subsystem_name == "bark.board").count()
        };
        assert_eq!(boards().await, 1);
        connection.execute_batch("DROP TRIGGER fail_checkpoint").unwrap();
        // Even once the database works again, a second cosign would create a second board.
        let error = wallet.submit_prepared_board(prepared.id.clone()).await.unwrap_err();
        assert_eq!(error.code, BarkErrorCode::PreparedInputsUnavailable);
        assert_eq!(boards().await, 1);
        assert_eq!(wallet.transaction_status(prepared.txid).await.unwrap(), TransactionStatus::NotFound);
        wallet.discard_prepared_board(prepared.id).await.unwrap();
    });
}
