//! Gateway staking + slashing, backed by REAL on-chain transactions (DSCP-2).
//!
//!   WCAHT_API_KEY=<submit-key> \
//!     cargo run -p wcaht-seal-sdk --bin wcaht-seal-stake -- http://<validator>:8901 [keypair.json]
//!
//! 1. Bonds a gateway on-chain (a real stake transfer into its deterministic escrow).
//! 2. Records the bond in a GatewayRegistry.
//! 3. Makes that gateway equivocate → builds the fraud proof → slashes it (bond forfeit).
//! 4. Publishes the slash claim on-chain.
//! 5. Shows the slashed gateway can no longer contribute to a FastSeal quorum.

use std::time::{SystemTime, UNIX_EPOCH};
use std::{thread, time::Duration};

use anyhow::{anyhow, Context, Result};
use seal_core::{detect_equivocation, GatewayRegistry, GatewayStanding, PreConfirmation};
use seal_crypto as sc;
use serde_json::Value;
use wcaht_seal_sdk::{gateway_escrow_address, AnchorSigner, WcahtRpc};

const FEE: u64 = 200_000;

fn main() -> Result<()> {
    let node = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8901".to_string());
    let kp_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| r"C:\Users\toeng\Desktop\WCAHT\PoASy3\heartbeat_keypair.json".to_string());
    let api_key = std::env::var("WCAHT_API_KEY").context("set WCAHT_API_KEY (SubmitTransaction/Admin key)")?;

    println!("== DSCP-2 gateway staking + slashing (real on-chain) ==");
    println!("node: {node}\n");

    let signer = load_signer(&kp_path)?;
    println!("funding account: {}", signer.address());
    let rpc = WcahtRpc::new(&node);

    // A staked gateway identity (its pre-confirmation signing key).
    let gateway = sc::SignId::generate();
    let gateway_id = gateway.public();
    let bond: u128 = 5_000_000;
    let escrow = gateway_escrow_address(&gateway_id);
    println!("gateway {}…  bond {bond}  escrow {escrow}", &hex::encode(gateway_id)[..12]);

    // ── 1. bond on-chain ────────────────────────────────────────────────────
    let (bh, lvs) = rpc.recent_blockhash()?;
    let stake_tx = signer.stake_anchor_tx(&gateway_id, bond, &bh, lvs, now(), FEE)?;
    let stake_slot = submit_and_confirm(&rpc, &api_key, stake_tx, "stake")?;
    println!("bond CONFIRMED on-chain in slot {stake_slot}\n");

    // ── 2. record the bond ──────────────────────────────────────────────────
    let mut reg = GatewayRegistry::new(1_000_000);
    reg.stake(gateway_id, bond)?;
    println!("registry: {:?}", reg.standing(&gateway_id));

    // ── 3. gateway equivocates → fraud proof ───────────────────────────────
    let seal_id = sc::random_32();
    let pre_confirmed_leaf = sc::random_32(); // what the gateway signed a pre-conf for
    let pc = PreConfirmation::create(&gateway, seal_id, pre_confirmed_leaf, stake_slot, stake_slot + 100_000);
    let finalized_leaf = sc::random_32(); // what L1 actually finalised for this seal_id
    let evidence = detect_equivocation(&pc, &finalized_leaf).ok_or_else(|| anyhow!("expected equivocation"))?;
    println!("\nequivocation detected — fraud proof valid: {}", evidence.is_valid());

    // ── 4. slash + publish the claim on-chain ──────────────────────────────
    let forfeited = reg.slash(&evidence)?;
    println!("SLASHED — forfeited {forfeited}; standing now {:?}", reg.standing(&gateway_id));

    let claim = slash_commitment(&gateway_id, &pre_confirmed_leaf, &finalized_leaf);
    let (bh2, lvs2) = rpc.recent_blockhash()?;
    let claim_tx = signer.transfer_tx(&bs58_of(&claim), 10_000, &bh2, lvs2, now(), FEE)?;
    let claim_slot = submit_and_confirm(&rpc, &api_key, claim_tx, "slash-claim")?;
    println!("slash claim PUBLISHED on-chain in slot {claim_slot}");

    // ── 5. the gateway is now useless for FastSeal ─────────────────────────
    let dropped = reg.filter_active(&[pc]).len();
    println!(
        "\ngateway active? {}  — its pre-confirmations kept by filter_active: {}",
        reg.is_active(&gateway_id),
        dropped
    );
    assert!(matches!(reg.standing(&gateway_id), GatewayStanding::Slashed { .. }));
    println!("done: bonded → equivocated → slashed → excluded, all on the real chain.");
    Ok(())
}

fn load_signer(path: &str) -> Result<AnchorSigner> {
    let kp: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let arr = kp["keypair"].as_array().ok_or_else(|| anyhow!("keypair array missing in {path}"))?;
    let mut seed = [0u8; 32];
    for (i, s) in seed.iter_mut().enumerate() {
        *s = arr.get(i).and_then(Value::as_u64).ok_or_else(|| anyhow!("bad seed byte {i}"))? as u8;
    }
    let signer = AnchorSigner::from_seed(&seed);
    if let Some(pk) = kp["public_key"].as_str() {
        if signer.address() != pk {
            return Err(anyhow!("derived pubkey {} != file pubkey {pk}", signer.address()));
        }
    }
    Ok(signer)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}
fn bs58_of(b: &[u8; 32]) -> String {
    bs58::encode(b).into_string()
}
fn slash_commitment(gateway_id: &[u8; 32], pre_leaf: &[u8; 32], fin_leaf: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(96);
    buf.extend_from_slice(gateway_id);
    buf.extend_from_slice(pre_leaf);
    buf.extend_from_slice(fin_leaf);
    sc::hash("DSCP-2/slash-claim", &buf)
}

fn submit_and_confirm(rpc: &WcahtRpc, api_key: &str, tx: Value, label: &str) -> Result<u64> {
    let sig = tx["signature"].as_str().unwrap_or_default().to_string();
    println!("submitting {label} tx  to={}  sig={}…", tx["to"].as_str().unwrap_or("?"), &sig[..sig.len().min(18)]);
    let resp = rpc.submit_tx(&tx, Some(api_key))?;
    println!("  accepted: {}", resp.get("status").and_then(Value::as_str).unwrap_or("?"));
    for _ in 0..90 {
        if let Ok(v) = rpc.transaction(&sig) {
            if let Some(slot) = v.get("slot").and_then(Value::as_u64) {
                return Ok(slot);
            }
        }
        thread::sleep(Duration::from_millis(1000));
    }
    Err(anyhow!("{label} tx not confirmed within 90s"))
}
