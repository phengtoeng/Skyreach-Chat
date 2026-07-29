//! Two-device smoke test of the LIVE delivery backend, exercising the EXACT mobile-app
//! wire path: Alice seals to Bob's device -> ships {seal_id,bundle} to the relay + the 3
//! shares to the gateways (+finalize) -> Bob polls HIS OWN mailbox tag -> collects the
//! released shares -> opens with his device seed. If the last line prints PASS, a message
//! sent on one device will show up + open on the other.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-n6-e2e -- [host]   (default 51.79.176.134 = N6)

use anyhow::{anyhow, Result};
use seal_core::*;
use seal_crypto as sc;
use serde_json::{json, Value};

fn main() -> Result<()> {
    let host = std::env::args().nth(1).unwrap_or_else(|| "51.79.176.134".to_string());
    let relay = format!("http://{host}:9200");
    let gateways = [
        format!("http://{host}:9201"),
        format!("http://{host}:9202"),
        format!("http://{host}:9203"),
    ];
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let text = "meet at the safehouse, 9pm";
    println!("== two-device transfer test against {host} ==");

    // --- Bob: the recipient (his device seed opens; his device pub is the address) ---
    let bob = Identity::generate("Bob");
    let bob_device_pub = bob.device_pub();
    let bob_device_seed = bob.device_seed;
    println!("Bob = {}", bob.address());

    // --- Alice seals to Bob's device (mirrors ss_seal_shippable) ---
    let sender_id = sc::SignId::generate();
    let sender_dev = sc::SignId::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let tag = mailbox_tag(&bob_device_pub);
    let item = seal_text_with_mode(
        text.as_bytes(),
        &sender_id,
        &sender_dev,
        &bob_device_pub,
        tag,
        &gw_ids,
        2,
        100,
        100_000,
        SealMode::StrictSeal,
    )
    .map_err(|e| anyhow!("seal: {e}"))?;
    let seal_id_hex = hex::encode(item.signed_leaf.leaf.seal_id);
    let bundle = json!({
        "envelope": serde_json::to_value(&item.envelope)?,
        "signed_leaf": serde_json::to_value(&item.signed_leaf)?,
        "sender_id_pub": hex::encode(sender_id.public()),
    });
    println!("Alice sealed \"{text}\"  (seal {}…)", &seal_id_hex[..16]);

    // --- SHIP: {seal_id,bundle} -> relay inbox (Bob's tag); shares -> gateways(+finalize) ---
    let item_json = json!({ "seal_id": seal_id_hex, "bundle": bundle });
    http.post(format!("{relay}/inbox/{}", hex::encode(tag))).json(&item_json).send()?.error_for_status()?;
    for (i, share) in item.share_envelopes.iter().enumerate().take(3) {
        http.post(format!("{}/deposit", gateways[i])).json(share).send()?.error_for_status()?;
        http.post(format!("{}/finalize/{}", gateways[i], seal_id_hex)).send()?.error_for_status()?;
    }
    println!("shipped: ciphertext -> relay, 3 shares -> gateways (finalized)");

    // --- Bob: poll HIS OWN mailbox tag, fetch bundle, collect released shares, open ---
    let my_tag = mailbox_tag(&bob_device_pub);
    if my_tag != tag {
        return Err(anyhow!("recipient's own tag must equal the ship target"));
    }
    let inbox: Vec<Value> = http.get(format!("{relay}/inbox/{}", hex::encode(my_tag))).send()?.error_for_status()?.json()?;
    let got = inbox
        .iter()
        .find(|v| v.get("seal_id").and_then(Value::as_str) == Some(seal_id_hex.as_str()))
        .ok_or_else(|| anyhow!("Bob's inbox has no such seal"))?;
    println!("Bob polled his inbox -> {} item(s)", inbox.len());

    let mut shares: Vec<KeyShareEnvelope> = Vec::new();
    for gw in &gateways {
        let released: Vec<KeyShareEnvelope> = http.get(format!("{gw}/release/{seal_id_hex}")).send()?.error_for_status()?.json()?;
        shares.extend(released);
    }
    println!("Bob collected {} released share(s)", shares.len());

    // open with Bob's device seed (mirrors ss_open_received)
    let bundle_v = got.get("bundle").cloned().unwrap_or(Value::Null);
    let envelope: EncryptedEnvelope = serde_json::from_value(bundle_v.get("envelope").cloned().unwrap_or(Value::Null))?;
    let signed_leaf: SignedLeaf = serde_json::from_value(bundle_v.get("signed_leaf").cloned().unwrap_or(Value::Null))?;
    let sender_pub: [u8; 32] = hex::decode(bundle_v.get("sender_id_pub").and_then(Value::as_str).unwrap_or(""))?
        .try_into()
        .map_err(|_| anyhow!("bad sender pub"))?;
    let recipient = sc::DeviceKey::from_seed(bob_device_seed);
    let mut chain = MockSealChain::new(1000);
    let _ = chain.submit_leaf(&signed_leaf, &sender_pub);
    let _ = chain.finalize(&signed_leaf.leaf.seal_id);

    match try_open(&envelope, &signed_leaf, &sender_pub, &recipient, &shares, &chain) {
        OpenOutcome::Opened { plaintext } => {
            let got_text = String::from_utf8_lossy(&plaintext);
            println!("\nBob OPENED: \"{got_text}\"");
            if got_text == text {
                println!("\n✅ PASS — the message transferred through the live backend and opened on the other device.");
                Ok(())
            } else {
                Err(anyhow!("plaintext mismatch"))
            }
        }
        OpenOutcome::Locked { reason, .. } => Err(anyhow!("still LOCKED: {reason}")),
        OpenOutcome::Rejected { reason } => Err(anyhow!("REJECTED: {reason}")),
    }
}
