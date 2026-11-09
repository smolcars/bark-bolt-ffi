use bark_bolt_ffi::wallet::WalletOpenArgs;
use bark_bolt_ffi::{
    generate_mnemonic, BarkError, BarkErrorCode, Bip321UriOptions, Config, LightningSendOptions,
    LightningSendStatus, Network, OnchainWallet, PaymentMethodParsingError, PaymentResult,
    ServerStatusAdoption, VtxoState, Wallet,
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

fn json(args: &[&str]) -> serde_json::Value {
    serde_json::from_str(&regtest(args)).unwrap()
}

fn config() -> Config {
    let port = |name: &str, default: &str| std::env::var(name).unwrap_or(default.into());
    serde_json::from_value(serde_json::json!({
        "server_address": format!("http://127.0.0.1:{}", port("ARK_PORT", "3635")),
        "bitcoind_address": format!("http://127.0.0.1:{}", port("BITCOIN_PORT", "19443")),
        "bitcoind_user": "second", "bitcoind_pass": "ark",
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

fn paid(status: &LightningSendStatus) -> bool {
    matches!(status, LightningSendStatus::Paid { .. })
}

#[test]
fn live_payment_requests_lightning_options_and_movement_ids() {
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
        let wallet = Wallet::open_with_onchain(
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
        regtest(&["send-to", &onchain.new_address().await.unwrap(), "0.01"]);
        onchain.sync().await.unwrap();
        wallet.board_amount(500_000).await.unwrap();
        regtest(&["generate", "6"]);
        wallet.sync().await.unwrap();

        let info = wallet.debug_info().await.unwrap();
        assert!(matches!(info.network, Network::Regtest));
        assert_eq!(info.mailbox_id, wallet.mailbox_identifier().unwrap());
        assert!(info.vtxo_xpub.starts_with("tpub"), "{}", info.vtxo_xpub);

        let garbage = wallet
            .parse_payment_request("not a payment".into())
            .await
            .unwrap_err();
        assert_eq!(garbage.code, BarkErrorCode::InvalidPaymentRequest);
        let mainnet = wallet
            .parse_payment_request("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq".into())
            .await
            .unwrap();
        assert_eq!(
            mainnet.options[0].errors,
            [PaymentMethodParsingError::NetworkMismatch]
        );
        assert!(mainnet.default_option.is_none());
        // Since Bark 0.8.1 a fiat-priced BOLT12 offer parses with an unknown amount.
        let usd_offer = "lno1qcp4256ypqpq86q2qe3k7enxv4j3vggz0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vq";
        let offer = wallet.parse_payment_request(usd_offer.into()).await.unwrap();
        assert_eq!(offer.amount_sats, None);
        assert!(offer.options[0].requires_amount);
        let error = wallet
            .parse_payment_request("bitcoin:?sp=x".into())
            .await
            .unwrap_err();
        assert_eq!(error.code, BarkErrorCode::InvalidPaymentRequest);

        // Ark: send_payment and send_arkoor_payment both return their history entry.
        let target = recipient.new_address().await.unwrap();
        let ark = wallet
            .parse_payment_request(format!(" {target}\n"))
            .await
            .unwrap();
        let option = ark.default_option.unwrap();
        assert_eq!(option.method.method_type, "ark");
        assert!(option.errors.is_empty() && option.requires_amount && !option.supports_comment);
        let fee = wallet
            .estimate_payment_fee(option.method.clone(), 10_000)
            .await
            .unwrap();
        assert_eq!(fee.gross_amount_sats, 10_000 + fee.fee_sats);
        let PaymentResult::Ark { movement_id } = wallet
            .send_payment(option.method, Some(10_000), None, None)
            .await
            .unwrap()
        else {
            panic!("expected an Ark payment");
        };
        let direct = wallet
            .send_arkoor_payment(target.clone(), 5_000)
            .await
            .unwrap();
        assert_ne!(direct, movement_id);
        let history = wallet.history().await.unwrap();
        for (id, sats) in [(movement_id, 10_000), (direct, 5_000)] {
            let movement = history.iter().find(|m| m.id == id).unwrap();
            assert_eq!(movement.sent_to_addresses[0].payment_method.value, target);
            assert_eq!(movement.sent_to_addresses[0].amount_sats, sats);
        }

        // BIP 321 from a wallet without onchain wallet: Ark and BOLT11 only.
        let uri = recipient
            .bip321_uri(Some(Bip321UriOptions {
                amount_sats: Some(3_000),
                label: Some("coffee".into()),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(uri.onchain_address.is_none());
        let invoice = uri.invoice.unwrap();
        assert_eq!(invoice.amount_sats, 3_000);
        recipient
            .lightning_receive_state(invoice.payment_hash.clone())
            .await
            .unwrap();
        let request = wallet.parse_payment_request(uri.uri.clone()).await.unwrap();
        assert_eq!(request.amount_sats, Some(3_000));
        assert_eq!(request.label.as_deref(), Some("coffee"));
        let types: Vec<_> = request
            .options
            .iter()
            .map(|o| o.method.method_type.as_str())
            .collect();
        assert_eq!(types, ["invoice", "ark"]);
        assert!(request.options.iter().all(|o| o.errors.is_empty()));
        assert_eq!(
            request.default_option.as_ref().unwrap().method.value,
            uri.ark_address.unwrap()
        );
        let fees = wallet
            .estimate_payment_fees(request.clone(), None)
            .await
            .unwrap();
        assert_eq!(fees.len(), 2);
        assert!(fees[0].fee.gross_amount_sats <= fees[1].fee.gross_amount_sats);

        // Onchain address derived before the invoice's server call, then added.
        let full = wallet
            .bip321_uri(Some(Bip321UriOptions {
                amount_sats: Some(2_500),
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(full.ark_address.is_some() && full.invoice.is_some());
        let address = full.onchain_address.unwrap();
        assert!(full.uri.contains(&format!("tb={address}")), "{}", full.uri);
        let parsed = wallet.parse_payment_request(full.uri).await.unwrap();
        assert!(parsed.options.iter().any(|o| o.method.value == address));
        let only_onchain = wallet
            .bip321_uri(Some(Bip321UriOptions {
                ark: false,
                lightning: false,
                ..Default::default()
            }))
            .await
            .unwrap();
        assert!(only_onchain.ark_address.is_none() && only_onchain.onchain_address.is_some());
        let nothing = Bip321UriOptions {
            ark: false,
            ..Default::default()
        };
        assert!(recipient.bip321_uri(Some(nothing)).await.is_err());

        // BIP 321 with the attached onchain wallet and no amount: no invoice.
        let own = wallet.bip321_uri(None).await.unwrap();
        assert!(own.invoice.is_none() && own.ark_address.is_some());
        let onchain_address = own.onchain_address.unwrap();
        assert!(onchain.is_mine(onchain_address.clone()).await.unwrap());
        let parsed = wallet.parse_payment_request(own.uri).await.unwrap();
        assert!(parsed
            .options
            .iter()
            .any(|o| o.method.value == onchain_address));

        // A URI amount that disagrees with its invoice is a typed per-option error.
        let lnd = json(&["lncli", "addinvoice", "--amt=3000"]);
        let invoice = lnd["payment_request"].as_str().unwrap();
        let mismatch = wallet
            .parse_payment_request(format!("bitcoin:?amount=0.00004&lightning={invoice}"))
            .await
            .unwrap();
        assert_eq!(
            mismatch.options[0].errors,
            [PaymentMethodParsingError::AmountMismatch {
                expected_sats: 3_000,
                got_sats: 4_000
            }]
        );

        // Lightning through send_payment with send options.
        regtest(&["wait-lightning-sync"]);
        let lnd = json(&["lncli", "addinvoice", "--amt=2000"]);
        let request = wallet
            .parse_payment_request(lnd["payment_request"].as_str().unwrap().into())
            .await
            .unwrap();
        assert_eq!(request.amount_sats, Some(2_000));
        let option = request.default_option.unwrap();
        assert!(option.method.method_type == "invoice" && !option.requires_amount);
        let options = LightningSendOptions {
            wait: true,
            retry_for_secs: Some(7),
        };
        let PaymentResult::Lightning { status } = wallet
            .send_payment(option.method, None, None, Some(options))
            .await
            .unwrap()
        else {
            panic!("expected a Lightning payment");
        };
        assert!(paid(&status), "{status:?}");
        assert_eq!(
            json(&["lncli", "lookupinvoice", lnd["r_hash"].as_str().unwrap()])["state"],
            "SETTLED"
        );

        // A retry time above the server maximum is capped, not refused.
        let lnd = json(&["lncli", "addinvoice", "--amt=1500"]);
        let status = wallet
            .pay_lightning_invoice(
                lnd["payment_request"].as_str().unwrap().into(),
                None,
                Some(LightningSendOptions {
                    wait: true,
                    retry_for_secs: Some(100_000),
                }),
            )
            .await
            .unwrap();
        assert!(paid(&status), "{status:?}");

        // Onchain through send_payment keeps the movement id.
        let destination = regtest(&["bcli", "getnewaddress"]);
        let request = wallet.parse_payment_request(destination).await.unwrap();
        let option = request.default_option.unwrap();
        assert_eq!(option.method.method_type, "bitcoin");
        let PaymentResult::Onchain { txid, movement_id } = wallet
            .send_payment(option.method, Some(20_000), None, None)
            .await
            .unwrap()
        else {
            panic!("expected an onchain payment");
        };
        regtest(&["generate", "1"]);
        let tx = json(&["bcli", "getrawtransaction", &txid, "true"]);
        assert!(tx["confirmations"].as_u64().unwrap() > 0);
        let history = wallet.history().await.unwrap();
        assert!(history.iter().any(|m| m.id == movement_id));
    });
}

#[test]
fn live_next_unused_addresses() {
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

        // Onchain: stable until a sync sees a payment, even an unconfirmed one.
        let first = onchain.next_unused_address().await.unwrap();
        assert_eq!(first, onchain.next_unused_address().await.unwrap());
        regtest(&["bcli", "sendtoaddress", &first, "0.01"]);
        assert_eq!(first, onchain.next_unused_address().await.unwrap());
        onchain.sync().await.unwrap();
        let second = onchain.next_unused_address().await.unwrap();
        assert_ne!(first, second);
        assert_eq!(second, onchain.next_unused_address().await.unwrap());
        regtest(&["generate", "1"]);
        onchain.sync().await.unwrap();
        sender.board_amount(300_000).await.unwrap();
        regtest(&["generate", "6"]);
        sender.sync().await.unwrap();

        // Ark: stable until a received payment is synced.
        let recipient = Wallet::open(
            Network::Regtest,
            generate_mnemonic().unwrap(),
            config(),
            args(&directory.path().join("recipient")),
        )
        .await
        .unwrap();
        let ark = recipient.next_unused_address().await.unwrap();
        let again = recipient.next_unused_address().await.unwrap();
        assert_eq!((&ark.address, ark.index), (&again.address, again.index));
        assert_eq!(
            recipient.peek_address(ark.index).await.unwrap(),
            ark.address
        );
        sender
            .send_arkoor_payment(ark.address.clone(), 10_000)
            .await
            .unwrap();
        recipient.sync().await.unwrap();
        let next = recipient.next_unused_address().await.unwrap();
        assert_ne!(next.address, ark.address);
        assert!(next.index > ark.index);
        let again = recipient.next_unused_address().await.unwrap();
        assert_eq!((&next.address, next.index), (&again.address, again.index));
        // new_address still always derives.
        assert_ne!(recipient.new_address().await.unwrap(), next.address);
    });
}

async fn adopt(wallet: &Wallet, id: &str) -> Result<Option<ServerStatusAdoption>, BarkError> {
    wallet.trust_and_adopt_server_vtxo_status(id.into()).await
}

async fn state(wallet: &Wallet, id: &str) -> VtxoState {
    wallet.get_vtxo_by_id(id.into()).await.unwrap().state
}

#[test]
fn live_trust_and_adopt_server_vtxo_status() {
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
        regtest(&["send-to", &onchain.new_address().await.unwrap(), "0.01"]);
        onchain.sync().await.unwrap();
        sender.board_amount(300_000).await.unwrap();
        regtest(&["generate", "6"]);
        sender.sync().await.unwrap();

        let seed = generate_mnemonic().unwrap();
        let recipient = Wallet::open(
            Network::Regtest,
            seed.clone(),
            config(),
            args(&directory.path().join("recipient")),
        )
        .await
        .unwrap();
        let address = recipient.new_address().await.unwrap();
        sender.send_arkoor_payment(address, 20_000).await.unwrap();
        recipient.sync().await.unwrap();
        let [received] = &recipient.spendable_vtxos().await.unwrap()[..] else {
            panic!("expected one received VTXO");
        };
        let id = received.id.clone();
        assert_eq!(
            adopt(&recipient, &id).await.unwrap(),
            Some(ServerStatusAdoption::Spendable)
        );
        assert_eq!(state(&recipient, &id).await, VtxoState::Spendable);

        // Back up, then spend the VTXO after the backup.
        let restored_dir = directory.path().join("restored");
        std::fs::create_dir(&restored_dir).unwrap();
        recipient
            .create_snapshot(restored_dir.join("db.sqlite").to_str().unwrap().into())
            .await
            .unwrap();
        let address = sender.new_address().await.unwrap();
        recipient.send_arkoor_payment(address, 5_000).await.unwrap();
        assert_eq!(state(&recipient, &id).await, VtxoState::Spent);
        // Spent here and on the server: stays spent.
        assert_eq!(
            adopt(&recipient, &id).await.unwrap(),
            Some(ServerStatusAdoption::Spent)
        );
        assert_eq!(state(&recipient, &id).await, VtxoState::Spent);
        drop(recipient);

        // The restored backup still has it spendable, and sync doesn't ask the server.
        let restored = Wallet::open(Network::Regtest, seed, config(), args(&restored_dir))
            .await
            .unwrap();
        restored.sync().await.unwrap();
        assert_eq!(state(&restored, &id).await, VtxoState::Spendable);
        let before = restored.balance().await.unwrap().spendable_sats;
        assert_eq!(
            adopt(&restored, &id).await.unwrap(),
            Some(ServerStatusAdoption::Spent)
        );
        assert_eq!(state(&restored, &id).await, VtxoState::Spent);
        let after = restored.balance().await.unwrap().spendable_sats;
        assert_eq!(before - after, 20_000);

        // A locked VTXO is left alone: None, still locked.
        let own = sender.spendable_vtxos().await.unwrap()[0].id.clone();
        sender.lock_vtxos(vec![own.clone()], None).await.unwrap();
        assert_eq!(adopt(&sender, &own).await.unwrap(), None);
        assert!(matches!(
            state(&sender, &own).await,
            VtxoState::Locked { .. }
        ));
        sender.unlock_vtxos(vec![own.clone()], None).await.unwrap();

        // Another wallet's VTXO and an invalid id are errors.
        assert!(adopt(&restored, &own).await.is_err());
        assert!(adopt(&restored, "not a vtxo").await.is_err());
    });
}
