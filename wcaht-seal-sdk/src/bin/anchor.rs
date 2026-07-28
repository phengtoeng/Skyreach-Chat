//! FULL end-to-end with a REAL on-chain anchor transaction.
//!
//!   WCAHT_API_KEY=<submit-key> \
//!     cargo run -p wcaht-seal-sdk --bin wcaht-seal-anchor -- [node_url] [keypair.json]
//!
//! Seals a message, submits a real WCAHT transfer that anchors its leaf hash, waits
//! for the tx to confirm on-chain, then waits for the chain to finalise past the
//! anchor block and OPENS the message. Nothing is mocked here — the finality gate is
//! the live chain and the anchor is a real, funded transaction.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{thread, time::Duration};

use anyhow::{anyhow, Context, Result};
use seal_core::{seal_text, try_open, Gateway, KeyShareEnvelope, OpenOutcome, SealChain};
use seal_crypto as sc;
use serde_json::Value;
use wcaht_seal_sdk::{AnchorSigner, WcahtSealChain};

fn main() -> Result<()> {
    let node = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8901".to_string());
    let kp_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| r"C:\Users\toeng\Desktop\WCAHT\PoASy3\heartbeat_keypair.json".to_string());
    let api_key = std::env::var("WCAHT_API_KEY").context("set WCAHT_API_KEY (a SubmitTransaction/Admin key)")?;

    println!("== SplitSeal REAL on-chain anchor ==");
    println!("node: {node}\n");

    // ── funded signer ──────────────────────────────────────────────────────
    let kp_json: Value = serde_json::from_str(&std::fs::read_to_string(&kp_path)?)?;
    let arr = kp_json["keypair"].as_array().ok_or_else(|| anyhow!("keypair array missing in {kp_path}"))?;
    let mut seed = [0u8; 32];
    for (i, s) in seed.iter_mut().enumerate() {
        *s = arr.get(i).and_then(Value::as_u64).ok_or_else(|| anyhow!("bad seed byte {i}"))? as u8;
    }
    let signer = AnchorSigner::from_seed(&seed);
    if let Some(pk) = kp_json["public_key"].as_str() {
        if signer.address() != pk {
            return Err(anyhow!("derived pubkey {} != file pubkey {pk}", signer.address()));
        }
    }
    println!("funded account: {}", signer.address());

    let chain = WcahtSealChain::new(&node);

    // ── 1. seal a message ──────────────────────────────────────────────────
    let sender_id = sc::SignId::generate();
    let sender_dev = sc::SignId::generate();
    let bob = sc::DeviceKey::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let mut gateways: Vec<Gateway> = gw_ids.iter().map(|id| Gateway::new(*id)).collect();
    let start_fin = chain.rpc().finalized_slot()?;
    let item = seal_text(
        b"Anchored on the real WCAHT chain, opened after finality.",
        &sender_id,
        &sender_dev,
        &bob.public(),
        sc::random_32(),
        &gw_ids,
        2,
        start_fin,
        100_000,
    )?;
    for e in &item.share_envelopes {
        if let Some(g) = gateways.iter_mut().find(|g| g.id == e.gateway_id) {
            g.deposit(e.clone());
        }
    }
    let seal_id = item.signed_leaf.leaf.seal_id;
    let leaf_hash = item.signed_leaf.leaf.leaf_hash();
    println!("sealed — leaf_hash {}", hex::encode(leaf_hash));

    // ── 2. build + submit the REAL anchor tx ───────────────────────────────
    let (rbh, lvs) = chain.rpc().recent_blockhash()?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let fee: u64 = 200_000; // deterministic consensus minimum = compute_units × price_per_cu
    let tx = signer.anchor_tx(&leaf_hash, &rbh, lvs, ts, fee)?;
    let sig = tx["signature"].as_str().unwrap_or_default().to_string();
    println!(
        "\nsubmitting anchor transfer  to={}  fee={fee}  sig={}…",
        tx["to"].as_str().unwrap_or("?"),
        &sig[..sig.len().min(20)]
    );
    let resp = chain.rpc().submit_tx(&tx, Some(&api_key))?;
    println!("submit ACCEPTED: {resp}");

    // ── 3. confirm on-chain (poll /transaction/:sig) ───────────────────────
    print!("confirming on-chain");
    std::io::stdout().flush().ok();
    let mut anchor_slot = None;
    for _ in 0..90 {
        if let Ok(v) = chain.rpc().transaction(&sig) {
            if let Some(slot) = v.get("slot").and_then(Value::as_u64) {
                println!("  confirmed in slot {slot}");
                anchor_slot = Some(slot);
                break;
            }
        }
        print!(".");
        std::io::stdout().flush().ok();
        thread::sleep(Duration::from_millis(1000));
    }
    let anchor_slot = anchor_slot.ok_or_else(|| anyhow!("anchor tx not confirmed within 90s"))?;
    chain.record_anchor_at(seal_id, leaf_hash, anchor_slot);

    // ── 4. still LOCKED until the anchor block finalises ───────────────────
    let collect = |chain: &WcahtSealChain, gws: &[Gateway]| -> Vec<KeyShareEnvelope> {
        gws.iter().filter_map(|g| g.request_share(&seal_id, chain)).collect()
    };
    let shares = collect(&chain, &gateways);
    let before = try_open(&item.envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &chain);
    println!(
        "\n[pre-final ] status={:?}  shares={}  ->  {}",
        chain.status(&seal_id),
        shares.len(),
        outcome(&before)
    );

    // ── 5. wait for real finality to reach the anchor slot, then OPEN ──────
    print!("waiting for finality to reach slot {anchor_slot}");
    std::io::stdout().flush().ok();
    loop {
        let fin = chain.rpc().finalized_slot()?;
        if fin >= anchor_slot {
            println!("  finalized_slot = {fin}");
            break;
        }
        print!(".");
        std::io::stdout().flush().ok();
        thread::sleep(Duration::from_millis(500));
    }
    let shares = collect(&chain, &gateways);
    let after = try_open(&item.envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &chain);
    println!(
        "[post-final] status={:?}  shares={}  ->  {}",
        chain.status(&seal_id),
        shares.len(),
        outcome(&after)
    );
    Ok(())
}

fn outcome(o: &OpenOutcome) -> String {
    match o {
        OpenOutcome::Locked { reason, .. } => format!("LOCKED ({reason})"),
        OpenOutcome::Opened { plaintext } => format!("OPENED: \"{}\"", String::from_utf8_lossy(plaintext)),
        OpenOutcome::Rejected { reason } => format!("REJECTED ({reason})"),
    }
}
