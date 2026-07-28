//! End-to-end SplitSeal flow gated on REAL WCAHT finality (no mock chain).
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-live-demo [http://HOST:PORT]
//!
//! Seals a message, records its on-chain anchor at the live finalized slot, shows it
//! LOCKED, waits for the real chain to finalise past that slot, then OPENS it. The
//! finality gate + gateway share release both consult the running node — the only
//! part still stubbed is submitting the anchor transaction (needs a funded account +
//! API key; see `AnchorSigner` / `WcahtRpc::submit_tx`).

use std::io::Write;
use std::{thread, time::Duration};

use anyhow::Result;
use seal_core::{seal_text, try_open, Gateway, KeyShareEnvelope, OpenOutcome, SealChain};
use seal_crypto as sc;
use wcaht_seal_sdk::WcahtSealChain;

fn main() -> Result<()> {
    let base = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8901".to_string());
    println!("== SplitSeal live demo — gated on REAL WCAHT finality ==");
    println!("node: {base}\n");

    let chain = WcahtSealChain::new(&base);
    let start_fin = chain.rpc().finalized_slot()?;
    println!("chain finalized_slot at seal time = {start_fin}");

    // Scenario: sender + recipient device + 3 seal gateways, threshold t = 2.
    let sender_id = sc::SignId::generate();
    let sender_dev = sc::SignId::generate();
    let bob = sc::DeviceKey::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let mut gateways: Vec<Gateway> = gw_ids.iter().map(|id| Gateway::new(*id)).collect();

    let item = seal_text(
        b"Opened only after the real WCAHT chain finalised.",
        &sender_id,
        &sender_dev,
        &bob.public(),
        sc::random_32(),
        &gw_ids,
        2,
        start_fin,
        100_000, // ttl slots — long window so it doesn't expire during the demo
    )?;
    for e in &item.share_envelopes {
        if let Some(g) = gateways.iter_mut().find(|g| g.id == e.gateway_id) {
            g.deposit(e.clone());
        }
    }
    let seal_id = item.signed_leaf.leaf.seal_id;
    let leaf_hash = item.signed_leaf.leaf.leaf_hash();

    // Anchor the seal on the live chain (records baseline = current finalized slot).
    // Production: submit the SEAL_ROOT / anchor transfer here via AnchorSigner + submit_tx.
    chain.record_anchor(seal_id, leaf_hash)?;
    println!("anchored seal {}… at baseline slot {start_fin}\n", hex::encode(&seal_id[..6]));

    let collect = |chain: &WcahtSealChain, gws: &[Gateway]| -> Vec<KeyShareEnvelope> {
        gws.iter().filter_map(|g| g.request_share(&seal_id, chain)).collect()
    };

    // t0: chain has not finalised past the anchor yet → LOCKED, gateways release nothing.
    let shares = collect(&chain, &gateways);
    let before = try_open(&item.envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &chain);
    println!(
        "[t0] status={:?}  shares_released={}  ->  {}",
        chain.status(&seal_id),
        shares.len(),
        outcome(&before)
    );

    // Wait for REAL finality to advance past the baseline slot.
    print!("waiting for real finality to advance");
    std::io::stdout().flush().ok();
    loop {
        let now = chain.rpc().finalized_slot()?;
        if now > start_fin {
            println!("  finalized_slot {start_fin} -> {now}\n");
            break;
        }
        print!(".");
        std::io::stdout().flush().ok();
        thread::sleep(Duration::from_millis(500));
    }

    // t1: chain finalised past the anchor → gateways release t shares → OPENS.
    let shares = collect(&chain, &gateways);
    let after = try_open(&item.envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &chain);
    println!(
        "[t1] status={:?}  shares_released={}  ->  {}",
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
