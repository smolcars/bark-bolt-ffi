use bark_bolt_ffi::wallet::WalletOpenArgs;
use bark_bolt_ffi::{generate_mnemonic, BarkErrorCode, Config, Network, OnchainWallet, Wallet};

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
    serde_json::from_value(serde_json::json!({
        "server_address": format!("http://127.0.0.1:{}", port("ARK_PORT", "3635")),
        "bitcoind_address": format!("http://127.0.0.1:{}", port("BITCOIN_PORT", "19443")),
        "bitcoind_user": "second", "bitcoind_pass": "ark", "vtxo_exit_margin": 2,
    }))
    .unwrap()
}

fn args(datadir: &std::path::Path) -> WalletOpenArgs {
    WalletOpenArgs {
        run_daemon: false,
        datadir: datadir.to_str().unwrap().into(),
        create_if_not_exists: true,
        create_without_server: false,
        skip_recovery: true,
    }
}

#[test]
fn live_whole_wallet_exit_estimate_skips_dust_vtxo() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let datadir = directory.path().join("sender");
        let onchain = OnchainWallet::default(
            Network::Regtest,
            mnemonic.clone(),
            config(),
            datadir.to_str().unwrap().into(),
        )
        .await
        .unwrap();
        let sender = Wallet::open_with_onchain(
            Network::Regtest,
            mnemonic,
            config(),
            args(&datadir),
            &onchain,
        )
        .await
        .unwrap();
        let recipient = Wallet::open(
            Network::Regtest,
            generate_mnemonic().unwrap(),
            config(),
            args(&directory.path().join("recipient")),
        )
        .await
        .unwrap();
        regtest(&["send-to", &onchain.new_address().await.unwrap(), "0.002"]);
        onchain.sync().await.unwrap();
        sender.board_amount(100_000).await.unwrap();
        regtest(&["generate", "6"]);
        sender.sync().await.unwrap();
        let address = recipient.new_address().await.unwrap();
        sender
            .send_arkoor_payment(address.clone(), 20_000)
            .await
            .unwrap();
        // Below the 330-sat P2TR dust limit: relay would refuse its exit.
        sender.send_arkoor_payment(address, 200).await.unwrap();
        recipient.sync().await.unwrap();
        let vtxos = recipient.spendable_vtxos().await.unwrap();
        assert_eq!(vtxos.len(), 2);
        let normal = vtxos.iter().find(|v| v.amount_sats == 20_000).unwrap();
        let dust = vtxos.iter().find(|v| v.amount_sats == 200).unwrap();

        let normal_only = recipient
            .estimate_emergency_exit_fee(vec![normal.id.clone()], Some(2), None, None)
            .await
            .unwrap();
        // Bark's whole-wallet exit also takes locked VTXOs, so the estimate must too.
        recipient
            .lock_vtxos(vec![normal.id.clone()], None)
            .await
            .unwrap();
        assert_eq!(recipient.spendable_vtxos().await.unwrap().len(), 1);
        let all = recipient
            .estimate_emergency_exit_fee(Vec::new(), Some(2), None, None)
            .await
            .unwrap();
        assert_eq!(all.total_fee_sats, normal_only.total_fee_sats);
        assert_eq!(all.txs_to_broadcast, normal_only.txs_to_broadcast);
        assert!(normal_only.excluded_vtxos.is_empty());
        assert_eq!(all.excluded_vtxos.len(), 1);
        let excluded = &all.excluded_vtxos[0];
        assert_eq!(excluded.vtxo_id, dust.id);
        assert_eq!(excluded.amount_sats, 200);
        assert_eq!(excluded.code, BarkErrorCode::ExitNonStandardVtxo);

        // Asking for the dust VTXO by id still fails, now with a typed code.
        let error = recipient
            .estimate_emergency_exit_fee(vec![dust.id.clone()], Some(2), None, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, BarkErrorCode::ExitNonStandardVtxo);
        assert_eq!(error.vtxo_id.as_deref(), Some(dust.id.as_str()));
        let error = recipient
            .start_exit_for_vtxos(vec![dust.id.clone()])
            .await
            .unwrap_err();
        assert!(
            matches!(
                error.code,
                BarkErrorCode::ExitDustLimit | BarkErrorCode::ExitNonStandardVtxo
            ),
            "{error:?}"
        );
        assert_eq!(error.vtxo_id.as_deref(), Some(dust.id.as_str()));
    });
}
