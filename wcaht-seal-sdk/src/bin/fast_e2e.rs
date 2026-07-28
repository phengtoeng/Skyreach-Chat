//! FastSeal end-to-end with the payload-prefetch relay (DSCP-2 sub-250ms fast path).
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-fast-e2e
//!
//! Spins up an in-process delivery relay, then: sender seals a FastSeal item and posts
//! its ciphertext; recipient PREFETCHES the locked ciphertext (before it can be opened);
//! a gateway pre-confirmation quorum arrives; the recipient unlocks — locally, with no
//! network round-trip, because the ciphertext was already prefetched. No L1 finality.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use seal_core::{
    seal_text_with_mode, try_open_dscp2, Gateway, MockSealChain, OpenOutcome, SealMode,
};
use seal_crypto as sc;
use wcaht_seal_sdk::relay::{serve_relay, DeliveryRelayClient};

fn main() -> Result<()> {
    let addr = "127.0.0.1:9977";
    thread::spawn(move || {
        let _ = serve_relay(addr);
    });
    // let the relay bind
    thread::sleep(Duration::from_millis(250));
    let relay = DeliveryRelayClient::new(&format!("http://{addr}"));

    println!("== FastSeal end-to-end via payload-prefetch relay (no L1 finality) ==\n");

    // Scenario: staked signing gateways (t=2 of 3), a recipient device.
    let sender_id = sc::SignId::generate();
    let sender_dev = sc::SignId::generate();
    let bob = sc::DeviceKey::generate();
    let signers: Vec<sc::SignId> = (0..3).map(|_| sc::SignId::generate()).collect();
    let gw_ids: Vec<[u8; 32]> = signers.iter().map(|s| s.public()).collect();
    let mut gateways: Vec<Gateway> = signers.into_iter().map(Gateway::with_identity).collect();
    let chain = MockSealChain::new(1000); // never finalised — the fast path must carry this

    let item = seal_text_with_mode(
        b"FastSeal: prefetched locked, unlocked by pre-confs.",
        &sender_id,
        &sender_dev,
        &bob.public(),
        sc::random_32(),
        &gw_ids,
        2,
        chain.slot(),
        100_000,
        SealMode::FastSeal,
    )?;
    for e in &item.share_envelopes {
        if let Some(g) = gateways.iter_mut().find(|g| g.id == e.gateway_id) {
            g.deposit(e.clone());
        }
    }
    let seal_id = item.signed_leaf.leaf.seal_id;
    let tag = item.envelope.recipient_mailbox_tag;

    // 1) Sender posts ciphertext to the relay.
    relay.post(&item.envelope)?;
    println!("sender posted ciphertext to relay ({} bytes)", item.envelope.ciphertext.len());

    // 2) Recipient PREFETCHES the locked ciphertext (before it is openable).
    let t_pf = Instant::now();
    let fetched = relay.prefetch(&tag)?;
    let prefetch = t_pf.elapsed();
    let envelope = fetched.into_iter().next().ok_or_else(|| anyhow!("nothing prefetched"))?;
    println!("recipient prefetched locked ciphertext in {:.2} ms", prefetch.as_secs_f64() * 1e3);

    // Prefetched but LOCKED (no pre-confirmations yet).
    let shares: Vec<_> = gateways.iter().filter_map(|g| g.request_share_fast(&seal_id)).collect();
    let locked = try_open_dscp2(&envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &[], &chain);
    println!("  before pre-confs: {}", outcome(&locked));

    // 3) Gateway pre-confirmation quorum arrives; recipient unlocks LOCALLY (no network).
    let preconfs: Vec<_> = gateways.iter().filter_map(|g| g.pre_confirm(&seal_id, chain.slot())).collect();
    let t_open = Instant::now();
    let opened = try_open_dscp2(&envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &preconfs, &chain);
    let unlock = t_open.elapsed();
    println!(
        "  {} pre-confs → unlock in {:.3} ms (local; ciphertext already in hand)",
        preconfs.len(),
        unlock.as_secs_f64() * 1e3
    );
    println!("  {}", outcome(&opened));

    println!(
        "\nfinalised? {}   total prefetch+unlock ≈ {:.2} ms",
        chain.proof(&seal_id).is_some(),
        (prefetch + unlock).as_secs_f64() * 1e3
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
