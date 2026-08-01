//! End-to-end proof that batching gives a capsule per message on ONE transaction.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-batch-demo -- http://127.0.0.1:9300
//!
//! Seals several messages, hands their leaves to the batcher, waits for the batch to be
//! anchored, then fetches each proof and verifies it the way a RECIPIENT would — recomputing
//! the root from its own leaf and the path it was given. No trust in the batcher at any point.

use std::time::Duration;

use anyhow::{anyhow, Result};
use seal_core::{seal_text_with_mode, verify_seal_inclusion, SealMode, SealProof};
use seal_crypto as sc;

fn main() -> Result<()> {
    let base = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:9300".to_string());
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build()?;

    println!("== batched capsules: many messages, one transaction ==\n");

    let alice = sc::SignId::generate();
    let alice_dev = sc::SignId::generate();
    let bob = sc::DeviceKey::generate();
    let gw: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();

    // 1. seal a handful of ordinary messages
    const N: usize = 6;
    let mut sealed = Vec::new();
    for i in 0..N {
        let item = seal_text_with_mode(
            format!("message {i}").as_bytes(),
            &alice,
            &alice_dev,
            &bob.public(),
            sc::random_32(),
            &gw,
            2,
            100,
            100_000,
            SealMode::StrictSeal,
            0,
            0,
        )?;
        sealed.push(item);
    }
    println!("1. sealed {N} messages — each with its own signed leaf");

    // 2. hand every leaf to the batcher
    for item in &sealed {
        let body = serde_json::json!({
            "signed_leaf": serde_json::to_value(&item.signed_leaf)?,
            "sender_id_pub": hex::encode(alice.public()),
        });
        let r = http.post(format!("{base}/leaf")).json(&body).send()?;
        if !r.status().is_success() {
            return Err(anyhow!("batcher rejected a leaf: {}", r.text()?));
        }
    }
    println!("2. queued all {N} leaves with the batcher");

    // 3. wait for it to commit them under one root and anchor it
    let mut proofs: Vec<SealProof> = Vec::new();
    for attempt in 0..40 {
        std::thread::sleep(Duration::from_millis(1000));
        proofs.clear();
        let mut all = true;
        for item in &sealed {
            let sid = hex::encode(item.signed_leaf.leaf.seal_id);
            let r = http.get(format!("{base}/proof/{sid}")).send()?;
            if r.status().is_success() {
                proofs.push(r.json::<SealProof>()?);
            } else {
                all = false;
                break;
            }
        }
        if all {
            println!("3. batch anchored after ~{}s", attempt + 1);
            break;
        }
    }
    if proofs.len() != N {
        return Err(anyhow!("batch was not anchored in time ({} of {N} proofs)", proofs.len()));
    }

    // 4. verify each proof the way the recipient does
    let root = proofs[0].seal_root;
    for (i, p) in proofs.iter().enumerate() {
        if p.seal_root != root {
            return Err(anyhow!("message {i} points at a different root"));
        }
        let expected = sealed[i].signed_leaf.leaf.leaf_hash();
        if p.leaf_hash != expected {
            return Err(anyhow!("message {i} proof is for the wrong leaf"));
        }
        if !verify_seal_inclusion(&p.leaf_hash, &p.merkle_path, p.leaf_index, p.leaf_count, &p.seal_root) {
            return Err(anyhow!("message {i} did not verify against the root"));
        }
    }
    println!("4. every message verified against the SAME root {}", hex::encode(&root[..8]));

    // 5. and a message that was never batched cannot borrow the root
    let outsider = seal_text_with_mode(
        b"never batched", &alice, &alice_dev, &bob.public(), sc::random_32(), &gw, 2, 100, 100_000,
        SealMode::StrictSeal, 0, 0,
    )?;
    let forged = outsider.signed_leaf.leaf.leaf_hash();
    let borrowed = &proofs[0];
    if verify_seal_inclusion(&forged, &borrowed.merkle_path, borrowed.leaf_index, borrowed.leaf_count, &root) {
        return Err(anyhow!("SECURITY: an unbatched message verified against the root"));
    }
    println!("5. a message that was never batched cannot borrow the root ✓");

    println!(
        "\n{N} capsules, 1 anchoring transaction. Per-message cost: {:.1} kak.",
        crate_fee() as f64 / N as f64
    );
    Ok(())
}

fn crate_fee() -> u64 {
    wcaht_seal_sdk::ANCHOR_MIN_FEE
}
