//! Real WCAHT integration for Denvion SplitSeal.
//!
//! - [`WcahtRpc`] reads LIVE finality (`/health` → `finalized_slot`) and submits
//!   anchor transactions to a running WCAHT node (default N1 `http://127.0.0.1:8901`).
//! - [`WcahtSealChain`] implements [`seal_core::SealChain`], so the SAME `try_open`
//!   used with the mock now gates on REAL chain finality.
//! - [`AnchorSigner`] builds + signs a WCAHT Transfer that anchors a seal leaf hash
//!   (byte-exact replica of the runtime `TX::v2` canonical preimage).
//!
//! v1 anchors a seal by committing the leaf hash as the recipient address of a tiny
//! transfer; the native `SEAL_ROOT` transaction (spec §9) is the eventual upgrade and
//! slots in behind the same `SealChain` trait.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Result};
use ed25519_dalek::{Signer, SigningKey};
use seal_core::{SealChain, SealProof, SealStatus};
use serde_json::{json, Value};

pub mod directory;
pub mod relay;

// ───────────────────────────── live RPC client ──────────────────────────────

pub struct WcahtRpc {
    base: String,
    http: reqwest::blocking::Client,
}

impl WcahtRpc {
    pub fn new(base_url: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_string(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("http client"),
        }
    }

    fn get_json(&self, path: &str) -> Result<Value> {
        Ok(self.http.get(format!("{}{}", self.base, path)).send()?.error_for_status()?.json()?)
    }

    /// REAL WCAHT finality — the slot the chain has finalised (spec §9.3 FINALISED gate).
    pub fn finalized_slot(&self) -> Result<u64> {
        self.get_json("/health")?
            .get("finalized_slot")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("no finalized_slot in /health"))
    }

    pub fn wall_clock_slot(&self) -> Result<u64> {
        let v = self.get_json("/health")?;
        v.pointer("/consensus_clock/wall_clock_slot")
            .and_then(Value::as_u64)
            .or_else(|| v.get("slot").and_then(Value::as_u64))
            .ok_or_else(|| anyhow!("no wall_clock_slot in /health"))
    }

    /// `(recent_blockhash_hex, last_valid_slot)` for signing an anchor tx.
    pub fn recent_blockhash(&self) -> Result<(String, u64)> {
        let v = self.get_json("/blockchain/recent_blockhash")?;
        let bh = v.get("recent_blockhash").and_then(Value::as_str).ok_or_else(|| anyhow!("no recent_blockhash"))?.to_string();
        let lvs = v.get("last_valid_slot").and_then(Value::as_u64).unwrap_or(0);
        Ok((bh, lvs))
    }

    /// Submit a signed transfer JSON to `/transactions/submit`. `api_key` is the
    /// node's SubmitTransaction API key (WCAHT security_config).
    pub fn submit_tx(&self, tx: &Value, api_key: Option<&str>) -> Result<Value> {
        let mut req = self.http.post(format!("{}/transactions/submit", self.base)).json(tx);
        if let Some(k) = api_key {
            req = req.header("x-api-key", k);
        }
        let resp = req.send()?;
        let ok = resp.status().is_success();
        let status = resp.status();
        let body: Value = resp.json().unwrap_or(Value::Null);
        if ok {
            Ok(body)
        } else {
            Err(anyhow!("submit rejected ({status}): {body}"))
        }
    }

    /// GET `/transaction/:signature` — the confirmed tx JSON (`{"status":"confirmed","slot":N,…}`).
    /// Errors while the tx is not yet found (404), which the caller polls through.
    pub fn transaction(&self, signature: &str) -> Result<Value> {
        self.get_json(&format!("/transaction/{signature}"))
    }
}

// ─────────────────────────── anchor transaction signer ──────────────────────

/// Signs the anchor transfer with a funded WCAHT account key.
pub struct AnchorSigner {
    sk: SigningKey,
    from_b58: String,
}

impl AnchorSigner {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let sk = SigningKey::from_bytes(seed);
        let from_b58 = bs58::encode(sk.verifying_key().to_bytes()).into_string();
        Self { sk, from_b58 }
    }
    pub fn address(&self) -> &str {
        &self.from_b58
    }

    /// Build + sign a generic Transfer — the on-chain primitive under both anchoring
    /// and gateway staking. Returns tx JSON ready for `/transactions/submit`.
    pub fn transfer_tx(
        &self,
        to_b58: &str,
        amount: u128,
        recent_blockhash_hex: &str,
        last_valid_slot: u64,
        timestamp: u64,
        fee: u64,
    ) -> Result<Value> {
        let rbh = decode_32(recent_blockhash_hex)?;
        let compute_units: u64 = 200_000;
        let priority_fee: u64 = 0;

        let preimage = canonical_bytes_v2(
            &rbh, &self.from_b58, to_b58, amount, fee, compute_units, priority_fee, last_valid_slot, timestamp,
        );
        let signature = bs58::encode(self.sk.sign(&preimage).to_bytes()).into_string();
        let tx_hash = hex::encode(blake3::hash(&preimage).as_bytes());

        Ok(json!({
            "recent_blockhash": recent_blockhash_hex,
            "last_valid_slot": last_valid_slot,
            "from": self.from_b58,
            "to": to_b58,
            "amount": amount,
            "compute_units": compute_units,
            "fee": fee,
            "signature": signature,
            "public_key": self.from_b58,
            "timestamp": timestamp,
            "tx_hash": tx_hash,
            "processor": null,
            "wasm": null,
            "priority_fee": priority_fee,
            "idempotency_key": null,
            "transaction_type": "Transfer",
            "read_set": [],
            "write_set": [],
            "kv_access_list": [],
            "token_op": null,
            "vote_data": null
        }))
    }

    /// Anchor a seal leaf hash on-chain (committed as the recipient address).
    pub fn anchor_tx(
        &self,
        leaf_hash: &[u8; 32],
        recent_blockhash_hex: &str,
        last_valid_slot: u64,
        timestamp: u64,
        fee: u64,
    ) -> Result<Value> {
        self.transfer_tx(
            &bs58::encode(leaf_hash).into_string(),
            10_000, // tiny, above any dust floor; the anchor is a commitment
            recent_blockhash_hex,
            last_valid_slot,
            timestamp,
            fee,
        )
    }

    /// Bond a gateway on-chain: lock `bond` into the gateway's deterministic stake
    /// escrow (backs `GatewayRegistry::stake` with a real finalized transfer).
    pub fn stake_anchor_tx(
        &self,
        gateway_id: &[u8; 32],
        bond: u128,
        recent_blockhash_hex: &str,
        last_valid_slot: u64,
        timestamp: u64,
        fee: u64,
    ) -> Result<Value> {
        self.transfer_tx(
            &gateway_escrow_address(gateway_id),
            bond,
            recent_blockhash_hex,
            last_valid_slot,
            timestamp,
            fee,
        )
    }
}

/// Deterministic, unspendable on-chain stake-escrow address for a gateway. Bonding is a
/// transfer here; the funds are locked (no one holds this address's key), and the bond
/// is attributable to `gateway_id` via the derivation.
pub fn gateway_escrow_address(gateway_id: &[u8; 32]) -> String {
    bs58::encode(seal_core_hash(gateway_id)).into_string()
}

fn seal_core_hash(gateway_id: &[u8; 32]) -> [u8; 32] {
    // BLAKE3(domain || 0x1f || gateway_id), matching the seal-crypto convention.
    let mut h = blake3::Hasher::new();
    h.update(b"DSCP-2/gw-escrow");
    h.update(&[0x1f]);
    h.update(gateway_id);
    *h.finalize().as_bytes()
}

fn decode_32(hex_str: &str) -> Result<[u8; 32]> {
    hex::decode(hex_str.trim_start_matches("0x"))?
        .try_into()
        .map_err(|_| anyhow!("recent_blockhash must be 32 bytes"))
}

/// EXACT replica of WCAHT `Transaction::canonical_bytes()` (`TX::v2`) for a simple
/// Transfer with empty access lists and no token/wasm/vote/idempotency payload.
/// Must match the runtime byte-for-byte or the node rejects the signature.
#[allow(clippy::too_many_arguments)]
fn canonical_bytes_v2(
    rbh: &[u8; 32],
    from: &str,
    to: &str,
    amount: u128,
    fee: u64,
    cu: u64,
    priority_fee: u64,
    last_valid_slot: u64,
    timestamp: u64,
) -> Vec<u8> {
    let mut v = Vec::with_capacity(256);
    v.extend_from_slice(b"TX::v2|");
    v.extend_from_slice(rbh);
    v.push(b'|');
    let fb = from.as_bytes();
    v.extend_from_slice(&(fb.len() as u32).to_le_bytes());
    v.extend_from_slice(fb);
    let tb = to.as_bytes();
    v.extend_from_slice(&(tb.len() as u32).to_le_bytes());
    v.extend_from_slice(tb);
    v.extend_from_slice(&amount.to_le_bytes());
    v.extend_from_slice(&fee.to_le_bytes());
    v.extend_from_slice(&cu.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes()); // read_set count
    v.extend_from_slice(&0u32.to_le_bytes()); // write_set count
    v.extend_from_slice(&0u32.to_le_bytes()); // kv_access_list count
    v.extend_from_slice(&priority_fee.to_le_bytes());
    v.extend_from_slice(&last_valid_slot.to_le_bytes());
    v.extend_from_slice(&timestamp.to_le_bytes());
    v.push(0u8); // TransactionType::Transfer
    v.push(0u8); // wasm None
    v.push(0u8); // vote_data None
    v.push(0u8); // idempotency_key None
    v
}

// ─────────────────────── SealChain backed by live WCAHT ─────────────────────

/// A [`SealChain`] whose finality gate is the REAL WCAHT finalized slot. A seal is
/// FINALISED once the chain has finalised past the slot at which its anchor was
/// recorded.
pub struct WcahtSealChain {
    rpc: WcahtRpc,
    anchors: Mutex<HashMap<[u8; 32], ([u8; 32], u64)>>, // seal_id -> (leaf_hash, baseline_finalized_slot)
}

impl WcahtSealChain {
    pub fn new(base_url: &str) -> Self {
        Self { rpc: WcahtRpc::new(base_url), anchors: Mutex::new(HashMap::new()) }
    }
    pub fn rpc(&self) -> &WcahtRpc {
        &self.rpc
    }

    /// Record an on-chain anchor. Call AFTER the anchor tx is accepted. `leaf_hash`
    /// must equal `signed_leaf.leaf.leaf_hash()`.
    pub fn record_anchor(&self, seal_id: [u8; 32], leaf_hash: [u8; 32]) -> Result<()> {
        let baseline = self.rpc.finalized_slot()?;
        self.anchors.lock().unwrap().insert(seal_id, (leaf_hash, baseline));
        Ok(())
    }

    /// Record an anchor whose tx confirmed in `anchor_slot`; the seal opens once the
    /// chain finalises at or past that slot (the anchor's own block becomes final).
    pub fn record_anchor_at(&self, seal_id: [u8; 32], leaf_hash: [u8; 32], anchor_slot: u64) {
        self.anchors
            .lock()
            .unwrap()
            .insert(seal_id, (leaf_hash, anchor_slot.saturating_sub(1)));
    }
}

impl SealChain for WcahtSealChain {
    fn slot(&self) -> u64 {
        self.rpc.finalized_slot().unwrap_or(0)
    }
    fn status(&self, seal_id: &[u8; 32]) -> SealStatus {
        let guard = self.anchors.lock().unwrap();
        let Some(&(_, baseline)) = guard.get(seal_id) else {
            return SealStatus::Unknown;
        };
        match self.rpc.finalized_slot() {
            Ok(fin) if fin > baseline => SealStatus::Finalised,
            Ok(_) => SealStatus::Finalising,
            Err(_) => SealStatus::Unknown,
        }
    }
    fn proof(&self, seal_id: &[u8; 32]) -> Option<SealProof> {
        let guard = self.anchors.lock().unwrap();
        let &(leaf_hash, baseline) = guard.get(seal_id)?;
        let fin = self.rpc.finalized_slot().ok()?;
        if fin <= baseline {
            return None;
        }
        Some(SealProof {
            seal_id: *seal_id,
            leaf_hash,
            merkle_path: Vec::new(),
            seal_root: leaf_hash,
            finalized_slot: fin,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bytes_are_deterministic_and_prefixed() {
        let rbh = [7u8; 32];
        let a = canonical_bytes_v2(&rbh, "alice", "bob", 1, 5000, 200_000, 0, 10, 123);
        let b = canonical_bytes_v2(&rbh, "alice", "bob", 1, 5000, 200_000, 0, 10, 123);
        assert_eq!(a, b);
        assert!(a.starts_with(b"TX::v2|"));
    }

    #[test]
    fn anchor_tx_has_required_fields() {
        let signer = AnchorSigner::from_seed(&[3u8; 32]);
        let tx = signer.anchor_tx(&[9u8; 32], &hex::encode([1u8; 32]), 100, 42, 5000).unwrap();
        for f in ["from", "to", "signature", "tx_hash", "recent_blockhash", "transaction_type"] {
            assert!(tx.get(f).is_some(), "missing {f}");
        }
        assert_eq!(tx["transaction_type"], "Transfer");
    }
}
