use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[boltffi::data]
pub struct Config {
    pub server_address: String,

    #[boltffi::default(None)]
    pub server_access_token: Option<String>,

    #[boltffi::default(None)]
    pub esplora_address: Option<String>,

    #[boltffi::default(None)]
    pub bitcoind_address: Option<String>,

    #[boltffi::default(None)]
    pub bitcoind_cookiefile: Option<String>,

    #[boltffi::default(None)]
    pub bitcoind_user: Option<String>,

    #[boltffi::default(None)]
    pub bitcoind_pass: Option<String>,

    #[boltffi::default(None)]
    pub vtxo_refresh_expiry_threshold: Option<u16>,

    #[boltffi::default(None)]
    pub vtxo_exit_margin: Option<u16>,

    #[boltffi::default(None)]
    pub htlc_recv_claim_delta: Option<u16>,

    /// Blocks from the tip until a new board's VTXO expires, capped at the
    /// server's VTXO lifetime. Default: 432 (3 days) on mainnet, 4320 elsewhere.
    #[boltffi::default(None)]
    pub board_lifetime: Option<u16>,

    #[boltffi::default(None)]
    pub fallback_fee_rate: Option<u64>,

    #[boltffi::default(None)]
    pub round_tx_required_confirmations: Option<u32>,

    #[boltffi::default(None)]
    pub daemon_sync_interval_secs: Option<u64>,

    #[boltffi::default(None)]
    pub offboard_required_confirmations: Option<u32>,

    #[boltffi::default(None)]
    pub daemon_manual_sync: Option<bool>,

    #[boltffi::default(None)]
    pub lightning_receive_claim_retries: Option<u8>,

    /// Optional custom user-agent. No host-language user-agent is inferred.
    #[boltffi::default(None)]
    pub user_agent: Option<String>,
    /// How many consecutive unused seed-derived VTXO key indices a scan
    /// crosses before concluding a VTXO isn't ours.
    ///
    /// Used by `Wallet::recover_vtxos` and `Wallet::import_vtxo(s)`, and by
    /// the seed-recovery scan that runs at wallet creation. Every match
    /// extends the window, so this bounds the run of unused indices, not the
    /// total keys derived. Raise it for a wallet that handed out many
    /// addresses without receiving into them.
    ///
    /// Default: 250. Capped at 100_000; a higher value is rejected when the
    /// wallet is opened or created.
    #[boltffi::default(None)]
    pub vtxo_key_gap_limit: Option<u32>,
}

impl Config {
    pub(crate) fn into_bark(
        self,
        network: bitcoin::Network,
    ) -> Result<bark::Config, crate::error::BarkError> {
        let mut ret = bark::Config::network_default(network);

        ret.server_address = self.server_address;
        // upstream deprecated this field; the FFI keeps exposing it until it's removed
        #[allow(deprecated)]
        {
            ret.server_access_token = self.server_access_token;
        }
        ret.user_agent = self.user_agent;
        ret.esplora_address = self.esplora_address;
        ret.bitcoind_address = self.bitcoind_address;
        ret.bitcoind_cookiefile = self.bitcoind_cookiefile.map(std::path::PathBuf::from);
        ret.bitcoind_user = self.bitcoind_user;
        // Upstream wraps the password so it can't leak into `Debug` output.
        ret.bitcoind_pass = self.bitcoind_pass.map(bark::secret::Secret::new);

        if let Some(threshold) = self.vtxo_refresh_expiry_threshold {
            ret.vtxo_refresh_expiry_threshold = threshold.into();
        }
        if let Some(margin) = self.vtxo_exit_margin {
            ret.vtxo_exit_margin = margin.into();
        }
        if let Some(delta) = self.htlc_recv_claim_delta {
            ret.htlc_recv_claim_delta = delta.into();
        }
        if let Some(lifetime) = self.board_lifetime {
            ret.board_lifetime = lifetime.into();
        }
        if let Some(rate) = self.fallback_fee_rate {
            ret.fallback_fee_rate = Some(bitcoin::FeeRate::from_sat_per_kwu(rate));
        }
        if let Some(confs) = self.round_tx_required_confirmations {
            ret.round_tx_required_confirmations = confs.try_into().map_err(|_| {
                crate::error::BarkError::from("round_tx_required_confirmations exceeds 65535")
            })?;
        }
        if let Some(secs) = self.daemon_sync_interval_secs {
            ret.daemon_sync_interval_secs = secs;
        }
        if let Some(confs) = self.offboard_required_confirmations {
            ret.offboard_required_confirmations = confs.try_into().map_err(|_| {
                crate::error::BarkError::from("offboard_required_confirmations exceeds 65535")
            })?;
        }
        if let Some(manual) = self.daemon_manual_sync {
            ret.daemon_manual_sync = manual;
        }
        if let Some(retries) = self.lightning_receive_claim_retries {
            ret.lightning_receive_claim_retries = retries;
        }
        if let Some(limit) = self.vtxo_key_gap_limit {
            // Refused up front rather than by the first key scan that reads it,
            // which would only surface on a recovery or import much later.
            if limit > bark::MAX_VTXO_KEY_GAP_LIMIT {
                return Err(format!(
                    "vtxo_key_gap_limit {} is above the maximum of {}",
                    limit,
                    bark::MAX_VTXO_KEY_GAP_LIMIT,
                )
                .into());
            }
            ret.vtxo_key_gap_limit = limit;
        }

        Ok(ret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config with nothing set, so each test only varies the field it cares
    /// about and everything else falls back to bark's network default.
    fn minimal() -> Config {
        Config {
            server_address: "http://127.0.0.1:3535".to_owned(),
            server_access_token: None,
            esplora_address: None,
            bitcoind_address: None,
            bitcoind_cookiefile: None,
            bitcoind_user: None,
            bitcoind_pass: None,
            vtxo_refresh_expiry_threshold: None,
            vtxo_exit_margin: None,
            htlc_recv_claim_delta: None,
            board_lifetime: None,
            fallback_fee_rate: None,
            round_tx_required_confirmations: None,
            daemon_sync_interval_secs: None,
            offboard_required_confirmations: None,
            daemon_manual_sync: None,
            lightning_receive_claim_retries: None,
            user_agent: None,
            vtxo_key_gap_limit: None,
        }
    }

    #[test]
    fn unset_gap_limit_keeps_the_upstream_default() {
        let cfg = minimal().into_bark(bitcoin::Network::Regtest).unwrap();
        assert_eq!(cfg.vtxo_key_gap_limit, bark::DEFAULT_VTXO_KEY_GAP_LIMIT);
    }

    #[test]
    fn board_lifetime_is_forwarded_or_defaults() {
        let cfg = minimal().into_bark(bitcoin::Network::Bitcoin).unwrap();
        assert_eq!(u32::from(cfg.board_lifetime), 432);
        let mut config = minimal();
        config.board_lifetime = Some(144);
        let cfg = config.into_bark(bitcoin::Network::Bitcoin).unwrap();
        assert_eq!(u32::from(cfg.board_lifetime), 144);
    }

    #[test]
    fn gap_limit_is_forwarded() {
        let mut config = minimal();
        config.vtxo_key_gap_limit = Some(1_000);
        let cfg = config.into_bark(bitcoin::Network::Regtest).unwrap();
        assert_eq!(cfg.vtxo_key_gap_limit, 1_000);
    }

    /// Rejected here rather than by the first key scan that reads it, which
    /// would only surface on a recovery or import long after wallet open.
    #[test]
    fn gap_limit_above_the_maximum_is_refused() {
        let mut config = minimal();
        config.vtxo_key_gap_limit = Some(bark::MAX_VTXO_KEY_GAP_LIMIT + 1);
        let err = config
            .into_bark(bitcoin::Network::Regtest)
            .expect_err("a gap limit above the maximum should be refused");
        assert!(
            err.message().contains("above the maximum"),
            "{}",
            err.message()
        );
    }
    #[test]
    fn confirmation_deltas_reject_overflow() {
        for offboard in [false, true] {
            let mut config = minimal();
            if offboard {
                config.offboard_required_confirmations = Some(65536);
            } else {
                config.round_tx_required_confirmations = Some(65536);
            }
            assert!(config
                .into_bark(bitcoin::Network::Regtest)
                .unwrap_err()
                .message
                .contains("exceeds 65535"));
        }
        let mut config = minimal();
        config.round_tx_required_confirmations = Some(65535);
        let converted = config.into_bark(bitcoin::Network::Regtest).unwrap();
        assert_eq!(u32::from(converted.round_tx_required_confirmations), 65535);
    }
}
