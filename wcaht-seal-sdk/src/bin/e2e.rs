//! FULL end-to-end sealed delivery (DSCP-2 Phase 3), across the real services.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-e2e
//!
//! Spins up the delivery relay + 3 gateway services, then runs Alice → Bob:
//!   Alice seals a message to Bob's device, ships the ciphertext to the relay and the
//!   t-of-n key shares to the gateways. Before finality, Bob gets nothing (gateways
//!   refuse to release). The seal finalises on WCAHT; the gateways release; Bob
//!   prefetches the ciphertext, collects the shares, and OPENS the plaintext — all over
//!   real HTTP with real crypto.

use std::{thread, time::Duration};

use anyhow::{anyhow, Result};
use seal_core::{seal_text, try_open, KeyShareEnvelope, MockSealChain, OpenOutcome};
use seal_crypto as sc;
use wcaht_seal_sdk::gateway_service::{serve_gateway, GatewayClient};
use wcaht_seal_sdk::relay::{serve_relay, DeliveryRelayClient};

fn main() -> Result<()> {
    // 1. spin up the delivery relay + 3 independent gateways
    let relay_addr = "127.0.0.1:9200";
    let gw_addrs = ["127.0.0.1:9201", "127.0.0.1:9202", "127.0.0.1:9203"];
    thread::spawn(move || {
        let _ = serve_relay(relay_addr);
    });
    for a in gw_addrs {
        let a = a.to_string();
        thread::spawn(move || {
            let _ = serve_gateway(&a);
        });
    }
    thread::sleep(Duration::from_millis(350));

    let relay = DeliveryRelayClient::new(&format!("http://{relay_addr}"));
    let gateways: Vec<GatewayClient> = gw_addrs.iter().map(|a| GatewayClient::new(&format!("http://{a}"))).collect();

    println!("== end-to-end sealed delivery: Alice → Bob (relay + 3 gateways) ==\n");

    // 2. identities — Bob holds his private device key; Alice only has his public one
    let alice_id = sc::SignId::generate();
    let alice_dev = sc::SignId::generate();
    let bob_device = sc::DeviceKey::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();

    // 3. Alice seals to Bob's device (t = 2 of 3)
    let secret = b"Meet at the safehouse, 9pm.";
    let mut chain = MockSealChain::new(1000); // Bob's view of WCAHT
    let item = seal_text(secret, &alice_id, &alice_dev, &bob_device.public(), sc::random_32(), &gw_ids, 2, chain.slot(), 100_000)?;
    let seal_id = item.signed_leaf.leaf.seal_id;
    let tag = item.envelope.recipient_mailbox_tag;
    println!("Alice sealed \"{}\" for Bob", String::from_utf8_lossy(secret));

    // 4. distribute: ciphertext → relay, one key share → each gateway
    relay.post(&item.envelope)?;
    for (i, gw) in gateways.iter().enumerate() {
        gw.deposit(&item.share_envelopes[i])?;
    }
    chain.submit_leaf(&item.signed_leaf, &alice_id.public())?;
    println!("  shipped: 1 ciphertext → relay, 3 key-shares → gateways\n");

    // 5. Bob prefetches the ciphertext and tries to open BEFORE finality
    let envelope = relay.prefetch(&tag)?.into_iter().next().ok_or_else(|| anyhow!("prefetch failed"))?;
    let early: Vec<KeyShareEnvelope> = gateways.iter().flat_map(|g| g.release(&seal_id).unwrap_or_default()).collect();
    let before = try_open(&envelope, &item.signed_leaf, &alice_id.public(), &bob_device, &early, &chain);
    println!("[before finality]  shares released = {}   →   {}", early.len(), outcome(&before));

    // 6. the seal finalises on WCAHT; the gateways observe it and release
    chain.finalize(&seal_id).ok();
    for gw in &gateways {
        gw.finalize(&seal_id)?;
    }
    println!("\nseal FINALISED on WCAHT — gateways release, Bob collects\n");

    // 7. Bob collects the shares and OPENS
    let shares: Vec<KeyShareEnvelope> = gateways.iter().flat_map(|g| g.release(&seal_id).unwrap_or_default()).collect();
    let after = try_open(&envelope, &item.signed_leaf, &alice_id.public(), &bob_device, &shares, &chain);
    println!("[after finality ]  shares released = {}   →   {}", shares.len(), outcome(&after));
    Ok(())
}

fn outcome(o: &OpenOutcome) -> String {
    match o {
        OpenOutcome::Locked { reason, .. } => format!("LOCKED ({reason})"),
        OpenOutcome::Opened { plaintext } => format!("OPENED: \"{}\"", String::from_utf8_lossy(plaintext)),
        OpenOutcome::Rejected { reason } => format!("REJECTED ({reason})"),
    }
}
