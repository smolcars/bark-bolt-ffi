//! Callback-based onchain wallet adapter
//!
//! This module allows foreign languages (Swift, Kotlin) to provide their own
//! onchain wallet implementations via BoltFFI callbacks. The adapter implements the
//! Bark onchain wallet trait by forwarding calls to the callback interface.

use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use async_trait::async_trait;
use bitcoin::{Address, Amount, FeeRate, Psbt, Script, ScriptBuf, Transaction};
use log::{error, warn};

use bark::chain::ChainSource;
use bark::onchain::{CpfpError, MakeCpfpFees, OnchainWalletTrait};

use crate::error::BarkError;
use crate::types::{CpfpParams, Destination};

/// Callback interface for custom onchain wallet implementations
///
/// Foreign languages implement this trait to provide their own wallet functionality.
#[boltffi::export]
pub trait CustomOnchainWalletCallbacks: Send + Sync {
    /// Get the wallet balance in satoshis
    ///
    /// An error falls back to the last value this returned, or 0 if none yet.
    fn get_balance(&self) -> Result<u64, BarkError>;

    /// Prepare a transaction to send to given destinations
    ///
    /// # Arguments
    /// * `destinations` - List of destinations with addresses and amounts
    /// * `fee_rate_sat_per_vb` - Fee rate in sats per vbyte
    ///
    /// # Returns
    /// Base64-encoded PSBT. Rejected unless it pays every destination the
    /// exact amount asked for; extra outputs (change) are fine.
    fn prepare_tx(
        &self,
        destinations: Vec<Destination>,
        fee_rate_sat_per_vb: u64,
    ) -> Result<String, BarkError>;

    /// Prepare a transaction that drains the wallet to a single address
    ///
    /// # Arguments
    /// * `address` - Bitcoin address to drain to
    /// * `fee_rate_sat_per_vb` - Fee rate in sats per vbyte
    ///
    /// # Returns
    /// Base64-encoded PSBT. Rejected unless every output pays `address`.
    fn prepare_drain_tx(
        &self,
        address: String,
        fee_rate_sat_per_vb: u64,
    ) -> Result<String, BarkError>;

    /// Sign/finalize without storing or registering the transaction.
    /// The returned PSBT must preserve its unsigned transaction.
    fn sign_psbt(&self, psbt_base64: String) -> Result<String, BarkError>;

    /// Sign, finalize and store a PSBT
    ///
    /// # Arguments
    /// * `psbt_base64` - Base64-encoded PSBT
    ///
    /// # Returns
    /// Base64-encoded fully signed PSBT (all witnesses filled in). Rejected
    /// unless the unsigned transaction is unchanged.
    fn finish_psbt(&self, psbt_base64: String) -> Result<String, BarkError>;

    /// Whether a script pubkey belongs to the wallet's keychains
    ///
    /// # Arguments
    /// * `script_pubkey_hex` - Hex-encoded script pubkey
    fn is_mine(&self, script_pubkey_hex: String) -> Result<bool, BarkError>;

    /// Register an unconfirmed transaction relevant to the wallet
    ///
    /// # Arguments
    /// * `tx_hex` - Hex-encoded transaction
    fn register_tx(&self, tx_hex: String) -> Result<(), BarkError>;

    /// Mark a wallet-known transaction as evicted from the mempool
    ///
    /// Bark calls this when a CPFP it broadcast was RBF-replaced by a competing
    /// party, so the tx's inputs should return to coin selection immediately
    /// instead of waiting for the sync eviction grace period. Only ever called
    /// for a tx that has definitively been superseded on-chain.
    ///
    /// # Arguments
    /// * `txid` - Hex-encoded transaction id
    fn evict_tx(&self, txid: String) -> Result<(), BarkError>;

    /// Create a signed P2A CPFP transaction
    ///
    /// # Arguments
    /// * `params` - CPFP transaction parameters
    ///
    /// # Returns
    /// Hex-encoded signed CPFP transaction. Rejected unless it spends the
    /// parent it was given.
    fn make_signed_p2a_cpfp(&self, params: CpfpParams) -> Result<String, BarkError>;

    /// Store a signed P2A CPFP transaction in the wallet
    ///
    /// # Arguments
    /// * `tx_hex` - Hex-encoded transaction
    fn store_signed_p2a_cpfp(&self, tx_hex: String) -> Result<(), BarkError>;

    /// Sync the wallet with the chain.
    ///
    /// Called by Bark (e.g. the background daemon) to ask the wallet to refresh
    /// its view of the chain before reading balances or building transactions.
    /// Implementations bring their own chain backend up to date — Bark's chain
    /// source is intentionally not passed across the FFI boundary.
    fn sync(&self) -> Result<(), BarkError>;
}

/// Rust adapter that implements Bark's onchain wallet trait using the callback
/// interface. What the foreign callbacks return is checked before it is used.
pub struct CallbackWalletAdapter {
    callbacks: Arc<dyn CustomOnchainWalletCallbacks>,
    /// `balance()` is infallible upstream, so on error it returns this rather
    /// than a zero that reads as "no funds".
    last_balance: Mutex<Option<Amount>>,
}

impl CallbackWalletAdapter {
    pub fn new(callbacks: Arc<dyn CustomOnchainWalletCallbacks>) -> Self {
        Self {
            callbacks,
            last_balance: Mutex::new(None),
        }
    }
}

fn validate_signed_psbt(original: &Psbt, signed_base64: &str) -> anyhow::Result<Psbt> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(signed_base64)?;
    let signed = Psbt::deserialize(&bytes)?;
    if signed.unsigned_tx != original.unsigned_tx {
        bail!("PSBT signer returned a different transaction than it was given");
    }
    Ok(signed)
}

/// Check that `tx` pays every `(script, amount)` in `required`, counted as a
/// multiset. Extra outputs are allowed: change is legitimate.
fn check_pays(tx: &Transaction, required: &[(ScriptBuf, Amount)]) -> anyhow::Result<()> {
    let mut available: Vec<_> = tx.output.iter().collect();

    for (script, amount) in required {
        match available
            .iter()
            .position(|o| &o.script_pubkey == script && o.value == *amount)
        {
            Some(i) => {
                available.swap_remove(i);
            }
            None => bail!(
                "wallet returned a transaction that does not pay {} sats to {}",
                amount.to_sat(),
                script.to_hex_string(),
            ),
        }
    }

    Ok(())
}

#[async_trait]
impl OnchainWalletTrait for CallbackWalletAdapter {
    async fn balance(&self) -> Amount {
        match self.callbacks.get_balance() {
            Ok(sats) => {
                let balance = Amount::from_sat(sats);
                *self.last_balance.lock().unwrap() = Some(balance);
                balance
            }
            Err(e) => {
                error!(
                    "CustomOnchainWalletCallbacks::get_balance failed: {}",
                    e.message()
                );
                match *self.last_balance.lock().unwrap() {
                    Some(stale) => {
                        warn!("Reusing the last known balance of {} sats", stale.to_sat());
                        stale
                    }
                    None => {
                        error!(
                            "No balance has ever been read; reporting 0, which will \
                                look like an empty wallet and may block boarding or exits"
                        );
                        Amount::ZERO
                    }
                }
            }
        }
    }

    async fn address(&mut self) -> anyhow::Result<Address> {
        // Callback wallets have no address callback; foreign wallets generate
        // addresses on their own side.
        Err(anyhow::anyhow!(
            "address() not supported for callback wallets"
        ))
    }

    async fn sync(&mut self, _chain: &ChainSource) -> anyhow::Result<()> {
        // Forwards to the foreign wallet's own `sync`. Bark's `ChainSource`
        // (esplora/bitcoind) is not used by the foreign implementation — it
        // owns its chain backend — so it is not passed across the FFI.
        self.callbacks.sync().context("sync failed")?;
        Ok(())
    }

    async fn is_mine(&self, spk: &Script) -> anyhow::Result<bool> {
        let spk_hex = hex::encode(spk.as_bytes());
        self.callbacks
            .is_mine(spk_hex)
            .map_err(|e| anyhow::anyhow!("is_mine failed: {}", e.message()))
    }

    async fn register_tx(&mut self, tx: &Transaction) -> anyhow::Result<()> {
        let tx_hex = hex::encode(bitcoin::consensus::serialize(tx));
        self.callbacks
            .register_tx(tx_hex)
            .map_err(|e| anyhow::anyhow!("register_tx failed: {}", e.message()))?;
        Ok(())
    }

    async fn evict_tx(&mut self, txid: bitcoin::Txid) -> anyhow::Result<()> {
        self.callbacks
            .evict_tx(txid.to_string())
            .map_err(|e| anyhow::anyhow!("evict_tx failed: {}", e.message()))?;
        Ok(())
    }

    async fn prepare_tx(
        &mut self,
        destinations: &[(Address, Amount)],
        fee_rate: FeeRate,
    ) -> anyhow::Result<Psbt> {
        // Convert destinations to FFI-friendly type
        let dests: Vec<Destination> = destinations
            .iter()
            .map(|(addr, amt)| Destination {
                address: addr.to_string(),
                amount_sats: amt.to_sat(),
            })
            .collect();

        // Get fee rate in sat/vB
        let fee_rate_sat_per_vb = fee_rate.to_sat_per_vb_ceil();

        // Call callback
        let psbt_base64 = self
            .callbacks
            .prepare_tx(dests, fee_rate_sat_per_vb)
            .map_err(|e| anyhow::anyhow!("prepare_tx failed: {}", e.message()))?;

        // Decode PSBT from base64
        use base64::Engine;
        let psbt_bytes = base64::engine::general_purpose::STANDARD.decode(&psbt_base64)?;
        let psbt = Psbt::deserialize(&psbt_bytes)?;

        let required: Vec<_> = destinations
            .iter()
            .map(|(addr, amt)| (addr.script_pubkey(), *amt))
            .collect();
        check_pays(&psbt.unsigned_tx, &required)
            .context("prepare_tx returned a transaction paying the wrong outputs")?;

        Ok(psbt)
    }

    async fn prepare_drain_tx(
        &mut self,
        destination: Address,
        fee_rate: FeeRate,
    ) -> anyhow::Result<Psbt> {
        let fee_rate_sat_per_vb = fee_rate.to_sat_per_vb_ceil();

        let psbt_base64 = self
            .callbacks
            .prepare_drain_tx(destination.to_string(), fee_rate_sat_per_vb)
            .map_err(|e| anyhow::anyhow!("prepare_drain_tx failed: {}", e.message()))?;

        use base64::Engine;
        let psbt_bytes = base64::engine::general_purpose::STANDARD.decode(&psbt_base64)?;
        let psbt = Psbt::deserialize(&psbt_bytes)?;

        // The amount is the wallet's to decide, the recipient is not.
        let spk = destination.script_pubkey();
        if psbt.unsigned_tx.output.is_empty()
            || psbt
                .unsigned_tx
                .output
                .iter()
                .any(|o| o.script_pubkey != spk)
        {
            bail!(
                "prepare_drain_tx returned a transaction paying somewhere other than {destination}"
            );
        }

        Ok(psbt)
    }

    async fn sign_psbt(&mut self, psbt: Psbt) -> anyhow::Result<Psbt> {
        let signed = self
            .callbacks
            .sign_psbt(psbt.to_string())
            .context("sign_psbt failed")?;
        validate_signed_psbt(&psbt, &signed)
    }

    async fn finish_psbt(&mut self, psbt: Psbt) -> anyhow::Result<Psbt> {
        let signed = self
            .callbacks
            .finish_psbt(psbt.to_string())
            .context("finish_psbt failed")?;
        validate_signed_psbt(&psbt, &signed)
    }

    async fn make_signed_p2a_cpfp(
        &mut self,
        tx: &Transaction,
        fees: MakeCpfpFees,
    ) -> Result<Transaction, CpfpError> {
        // Convert to FFI-friendly CPFP params
        let tx_hex = hex::encode(bitcoin::consensus::serialize(tx));

        let mut params = CpfpParams {
            tx_hex,
            fees_type: "Effective".to_string(),
            effective_fee_rate_sat_per_vb: fees.effective().to_sat_per_vb_ceil(),
            replaced_fee_sats: None,
            replaced_fee_rate_sat_per_vb: None,
            incremental_relay_fee_sat_per_vb: None,
            parent_in_mempool: None,
        };
        if let MakeCpfpFees::Rbf {
            replaced_fee,
            replaced_fee_rate,
            incremental_relay_fee,
            parent_in_mempool,
            ..
        } = fees
        {
            params.fees_type = "Rbf".to_string();
            params.replaced_fee_sats = Some(replaced_fee.to_sat());
            params.replaced_fee_rate_sat_per_vb = Some(replaced_fee_rate.to_sat_per_vb_ceil());
            params.incremental_relay_fee_sat_per_vb =
                Some(incremental_relay_fee.to_sat_per_vb_ceil());
            params.parent_in_mempool = Some(parent_in_mempool);
        }

        // Call callback
        let cpfp_tx_hex = self
            .callbacks
            .make_signed_p2a_cpfp(params)
            .map_err(|e| CpfpError::CreateError(e.message()))?;

        // Decode transaction
        let cpfp_tx_bytes = hex::decode(&cpfp_tx_hex)
            .map_err(|e| CpfpError::InternalError(format!("Invalid hex: {}", e)))?;
        let cpfp_tx: Transaction = bitcoin::consensus::deserialize(&cpfp_tx_bytes)
            .map_err(|e| CpfpError::InternalError(format!("Invalid transaction: {}", e)))?;

        // A CPFP child that does not spend its parent bumps nothing.
        let parent = tx.compute_txid();
        if !cpfp_tx
            .input
            .iter()
            .any(|i| i.previous_output.txid == parent)
        {
            return Err(CpfpError::CreateError(format!(
                "returned CPFP transaction does not spend its parent {}",
                parent,
            )));
        }

        Ok(cpfp_tx)
    }

    async fn store_signed_p2a_cpfp(&mut self, tx: &Transaction) -> anyhow::Result<(), CpfpError> {
        let tx_hex = hex::encode(bitcoin::consensus::serialize(tx));

        self.callbacks
            .store_signed_p2a_cpfp(tx_hex)
            .map_err(|e| CpfpError::StoreError(e.message()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{absolute::LockTime, transaction::Version, TxOut};

    fn spk(byte: u8) -> ScriptBuf {
        ScriptBuf::from_bytes(vec![byte; 22])
    }

    fn tx_paying(outs: &[(ScriptBuf, u64)]) -> Transaction {
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: outs
                .iter()
                .map(|(s, v)| TxOut {
                    value: Amount::from_sat(*v),
                    script_pubkey: s.clone(),
                })
                .collect(),
        }
    }

    #[test]
    fn accepts_the_requested_destinations() {
        let tx = tx_paying(&[(spk(1), 1000), (spk(2), 500)]);
        check_pays(&tx, &[(spk(1), Amount::from_sat(1000))]).unwrap();
    }

    #[test]
    fn a_change_output_is_allowed() {
        let tx = tx_paying(&[(spk(1), 1000), (spk(9), 4242)]);
        check_pays(&tx, &[(spk(1), Amount::from_sat(1000))]).unwrap();
    }

    /// The case this guards: a wallet that pays someone else instead.
    #[test]
    fn refuses_a_substituted_destination() {
        let tx = tx_paying(&[(spk(7), 1000)]);
        check_pays(&tx, &[(spk(1), Amount::from_sat(1000))]).unwrap_err();
    }

    #[test]
    fn refuses_a_short_payment() {
        let tx = tx_paying(&[(spk(1), 999)]);
        check_pays(&tx, &[(spk(1), Amount::from_sat(1000))]).unwrap_err();
    }

    /// Two equal destinations need two outputs, not one counted twice.
    #[test]
    fn duplicate_destinations_need_one_output_each() {
        let one = tx_paying(&[(spk(1), 1000)]);
        let required = [
            (spk(1), Amount::from_sat(1000)),
            (spk(1), Amount::from_sat(1000)),
        ];
        check_pays(&one, &required).unwrap_err();

        let two = tx_paying(&[(spk(1), 1000), (spk(1), 1000)]);
        check_pays(&two, &required).unwrap();
    }
    #[test]
    fn signing_preserves_the_requested_transaction() {
        let original = Psbt::from_unsigned_tx(tx_paying(&[(spk(1), 1000)])).unwrap();
        validate_signed_psbt(&original, &original.to_string()).unwrap();
        let substituted = Psbt::from_unsigned_tx(tx_paying(&[(spk(7), 1000)])).unwrap();
        assert!(validate_signed_psbt(&original, &substituted.to_string())
            .unwrap_err()
            .to_string()
            .contains("different transaction"));
        assert!(validate_signed_psbt(&original, "invalid base64").is_err());
    }
}
