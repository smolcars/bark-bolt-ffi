use bark_bolt_ffi::wallet::WalletOpenArgs;
use bark_bolt_ffi::{
    generate_mnemonic, BarkErrorCode, Bolt11InvoiceOptions, Config, Network, OnchainWallet,
    PresignedExitStatus, Wallet,
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

fn decode(tx_hex: &str) -> bitcoin::Transaction {
    bitcoin::consensus::encode::deserialize_hex(tx_hex).unwrap()
}

#[test]
fn live_presigned_exit_plan_broadcasts_and_confirms() {
    if std::env::var("BARK_REGTEST").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mnemonic = generate_mnemonic().unwrap();
        let datadir = directory.path().join("sender");
        let open_onchain = || {
            OnchainWallet::default(
                Network::Regtest,
                mnemonic.clone(),
                config(),
                datadir.to_str().unwrap().into(),
            )
        };
        let onchain = open_onchain().await.unwrap();
        let sender = Wallet::open_with_onchain(
            Network::Regtest,
            mnemonic.clone(),
            config(),
            args(&datadir),
            &onchain,
        )
        .await
        .unwrap();
        regtest(&["send-to", &onchain.new_address().await.unwrap(), "0.01"]);
        onchain.sync().await.unwrap();
        sender.board_amount(300_000).await.unwrap();
        regtest(&["generate", "6"]);
        sender.sync().await.unwrap();

        let recipient = Wallet::open(
            Network::Regtest,
            generate_mnemonic().unwrap(),
            config(),
            args(&directory.path().join("recipient")),
        )
        .await
        .unwrap();
        // The arkoor change VTXO exits over the board and arkoor transactions.
        let address = recipient.new_address().await.unwrap();
        sender.send_arkoor_payment(address, 20_000).await.unwrap();
        recipient.sync().await.unwrap();

        // Reopen: since bdk_wallet 3.2 a reloaded wallet signs only through Bark's signers.
        drop(sender);
        drop(onchain);
        let onchain = open_onchain().await.unwrap();
        let sender = Wallet::open_with_onchain(
            Network::Regtest,
            mnemonic.clone(),
            config(),
            args(&datadir),
            &onchain,
        )
        .await
        .unwrap();
        onchain.sync().await.unwrap();

        let ids: Vec<String> = sender
            .spendable_vtxos()
            .await
            .unwrap()
            .into_iter()
            .map(|vtxo| vtxo.id)
            .collect();
        // Both change VTXOs share the board and arkoor transactions.
        assert_eq!(ids.len(), 2);
        let remaining = sender
            .remaining_exit_transactions(ids.clone())
            .await
            .unwrap();
        let listed: Vec<_> = remaining
            .transactions
            .iter()
            .map(|tx| tx.txid.clone())
            .collect();
        let position = |txid: &String| listed.iter().position(|t| t == txid).unwrap();
        assert_eq!(
            remaining
                .exits
                .iter()
                .map(|e| e.vtxo_id.clone())
                .collect::<Vec<_>>(),
            ids
        );
        for exit in &remaining.exits {
            assert!(exit.txids.len() >= 2, "{remaining:?}");
            // Parents before children, each listed once.
            assert!(exit
                .txids
                .windows(2)
                .all(|w| position(&w[0]) < position(&w[1])));
        }
        assert_eq!(
            listed
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            listed.len()
        );
        assert!(listed.len() < remaining.exits.iter().map(|e| e.txids.len()).sum());
        for tx in &remaining.transactions {
            assert_eq!(decode(&tx.tx_hex).compute_txid().to_string(), tx.txid);
        }

        let before = onchain.balance().await.unwrap();
        let plan = sender
            .presign_emergency_exit(ids.clone(), Some(5))
            .await
            .unwrap();
        // Each remaining transaction gets exactly one package, after its parent's wave.
        let mut wave_of = std::collections::HashMap::new();
        let mut fees = 0;
        for (index, wave) in plan.waves.iter().enumerate() {
            assert!(wave.in_mempool.is_empty());
            for package in &wave.packages {
                assert!(wave_of.insert(package.exit.txid.clone(), index).is_none());
                assert_eq!(
                    decode(&package.exit.tx_hex).compute_txid().to_string(),
                    package.exit.txid
                );
                let child = decode(&package.child.tx_hex);
                assert_eq!(child.compute_txid().to_string(), package.child.txid);
                assert!(child
                    .input
                    .iter()
                    .any(|input| input.previous_output.txid.to_string() == package.exit.txid));
                assert!(package.child.fee_rate_sat_per_vb >= 5);
                fees += package.child.total_fee_sats;
            }
        }
        assert_eq!(wave_of.len(), listed.len());
        let mut signed_fees = 0;
        for (presigned, exit) in plan.exits.iter().zip(&remaining.exits) {
            assert_eq!(presigned.vtxo_id, exit.vtxo_id);
            assert!(presigned.unsigned.is_empty());
            let PresignedExitStatus::FullySigned {
                exited_in_wave,
                total_fee_sats,
                ..
            } = presigned.status
            else {
                panic!("expected a fully signed exit: {plan:?}");
            };
            let waves: Vec<_> = exit.txids.iter().map(|txid| wave_of[txid]).collect();
            assert!(waves.windows(2).all(|w| w[0] < w[1]), "{plan:?}");
            assert_eq!(exited_in_wave, waves.last().map(|wave| *wave as u32));
            signed_fees += total_fee_sats;
        }
        // A shared package counts under the first exit only.
        assert_eq!(signed_fees, fees);
        // Presigning broadcasts nothing and leaves the balance alone.
        let mempool = regtest(&["bcli", "getrawmempool"]);
        assert!(plan
            .waves
            .iter()
            .flat_map(|wave| &wave.packages)
            .all(|package| !mempool.contains(&package.child.txid)));
        assert_eq!(
            onchain.balance().await.unwrap().total_sats,
            before.total_sats
        );

        // Run the plan wave by wave.
        for wave in &plan.waves {
            for package in &wave.packages {
                let package_hex =
                    format!(r#"["{}","{}"]"#, package.exit.tx_hex, package.child.tx_hex);
                let result: serde_json::Value =
                    serde_json::from_str(&regtest(&["bcli", "submitpackage", &package_hex]))
                        .unwrap();
                assert_eq!(result["package_msg"], "success", "{result}");
            }
            regtest(&["generate", "1"]);
        }

        // The children's change addresses were revealed, so one sync sees it all.
        onchain.sync().await.unwrap();
        let after = onchain.balance().await.unwrap();
        // Children also spend the anchors' value, which the package fees include.
        let anchors: u64 = plan
            .waves
            .iter()
            .flat_map(|wave| &wave.packages)
            .flat_map(|package| decode(&package.exit.tx_hex).output)
            .filter(|output| output.script_pubkey.as_bytes() == [0x51, 0x02, 0x4e, 0x73])
            .map(|output| output.value.to_sat())
            .sum();
        assert_eq!(after.total_sats, before.total_sats - fees + anchors);
        sender.sync_exits().await.unwrap();
        let remaining = sender
            .remaining_exit_transactions(ids.clone())
            .await
            .unwrap();
        assert!(remaining.transactions.is_empty(), "{remaining:?}");

        // A wallet without an onchain wallet can't presign; a foreign VTXO is unknown.
        let recipient_ids: Vec<String> = recipient
            .spendable_vtxos()
            .await
            .unwrap()
            .into_iter()
            .map(|vtxo| vtxo.id)
            .collect();
        assert!(recipient
            .presign_emergency_exit(recipient_ids.clone(), None)
            .await
            .is_err());
        let error = sender
            .remaining_exit_transactions(recipient_ids)
            .await
            .unwrap_err();
        assert_eq!(error.code, BarkErrorCode::ExitUnknownVtxo);

        // A custom invoice expiry reaches the invoice.
        let invoice = recipient
            .bolt11_invoice_with(
                10_000,
                Bolt11InvoiceOptions {
                    description: Some("coffee".into()),
                    expiry_secs: Some(600),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let decoded: ark::lightning::Bolt11Invoice = invoice.invoice.parse().unwrap();
        assert_eq!(decoded.expiry_time().as_secs(), 600);
        assert_eq!(decoded.amount_milli_satoshis(), Some(10_000_000));
        // The server caps the expiry at its own 48 hours.
        assert!(recipient
            .bolt11_invoice_with(
                10_000,
                Bolt11InvoiceOptions {
                    expiry_secs: Some(49 * 3600),
                    ..Default::default()
                },
            )
            .await
            .is_err());
    });
}
