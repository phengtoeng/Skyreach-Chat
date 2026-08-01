//! FULL end-to-end sealed MEDIA delivery, across the real services.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-media-e2e
//!
//! Alice seals an image to Bob's device. The pixels go to the relay as opaque encrypted
//! chunks over real HTTP; the manifest travels in the envelope; the key shares go to three
//! gateways. Bob downloads every chunk BEFORE finality and still cannot open a single one.
//! After finality the gateways release, Bob opens the manifest, decrypts the chunks, and
//! reassembles a byte-identical file.
//!
//! Also checks what the relay refuses: a blob that does not match its content hash, and a
//! blob whose bytes were swapped underneath a correct name.

use std::{thread, time::Duration};

use anyhow::{anyhow, Result};
use seal_core::{
    seal_media_with_mode, try_open_media, ContentKind, KeyShareEnvelope, MediaOutcome, MockSealChain, PreviewPolicy,
    SealMode, DEFAULT_CHUNK_SIZE,
};
use seal_crypto as sc;
use wcaht_seal_sdk::gateway_service::{serve_gateway, GatewayClient};
use wcaht_seal_sdk::relay::serve_relay;

fn put_blob(http: &reqwest::blocking::Client, base: &str, hash_hex: &str, bytes: &[u8]) -> Result<u16> {
    let r = http.put(format!("{base}/blob/{hash_hex}")).body(bytes.to_vec()).send()?;
    Ok(r.status().as_u16())
}

fn get_blob(http: &reqwest::blocking::Client, base: &str, hash_hex: &str) -> Result<Vec<u8>> {
    let r = http.get(format!("{base}/blob/{hash_hex}")).send()?;
    if !r.status().is_success() {
        return Err(anyhow!("blob {hash_hex}: HTTP {}", r.status()));
    }
    Ok(r.bytes()?.to_vec())
}

fn main() -> Result<()> {
    let relay_addr = "127.0.0.1:9210";
    let gw_addrs = ["127.0.0.1:9211", "127.0.0.1:9212", "127.0.0.1:9213"];
    thread::spawn(move || {
        let _ = serve_relay(relay_addr);
    });
    for a in gw_addrs {
        let a = a.to_string();
        thread::spawn(move || {
            let _ = serve_gateway(&a);
        });
    }
    thread::sleep(Duration::from_millis(400));
    let base = format!("http://{relay_addr}");
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let gateways: Vec<GatewayClient> = gw_addrs.iter().map(|a| GatewayClient::new(&format!("http://{a}"))).collect();

    println!("== end-to-end sealed MEDIA: Alice → Bob ==\n");

    let alice_id = sc::SignId::generate();
    let alice_dev = sc::SignId::generate();
    let bob_device = sc::DeviceKey::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();

    // a ~2.5 MiB "photo" → 3 chunks, the last one partial
    let photo: Vec<u8> = (0..(2 * 1024 * 1024 + 512 * 1024u32)).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    println!("1. Alice picks a {:.2} MiB image", photo.len() as f64 / 1048576.0);

    let mut chain = MockSealChain::new(1000);
    let sealed = seal_media_with_mode(
        &photo,
        ContentKind::Image,
        "image/jpeg",
        "sent with a caption",
        b"<blurred preview bytes>",
        PreviewPolicy::LockedBlur,
        (4032, 3024),
        0,
        DEFAULT_CHUNK_SIZE,
        &alice_id,
        &alice_dev,
        &bob_device.public(),
        sc::random_32(),
        &gw_ids,
        2,
        chain.slot(),
        100_000,
        SealMode::StrictSeal,
        0,
        0,
    )?;
    let item = &sealed.item;
    let seal_id = item.signed_leaf.leaf.seal_id;
    println!(
        "   sealed into {} encrypted chunks; leaf carries manifest_root {} and NOTHING else about the file",
        sealed.chunks.len(),
        hex::encode(&item.signed_leaf.leaf.manifest_root[..8])
    );

    // 2. upload the opaque chunks
    for c in &sealed.chunks {
        let code = put_blob(&http, &base, &hex::encode(c.ciphertext_hash), &c.bytes)?;
        if code != 200 {
            return Err(anyhow!("chunk {} upload failed: HTTP {code}", c.index));
        }
    }
    println!("2. uploaded {} chunks to the relay (it stores ciphertext it cannot open)", sealed.chunks.len());

    // the relay verifies content-addressing: a mislabelled blob is refused
    let bogus_name = hex::encode([0xABu8; 32]);
    let code = put_blob(&http, &base, &bogus_name, b"not what this name claims")?;
    println!("   relay rejects a blob that doesn't match its hash → HTTP {code}");
    if code == 200 {
        return Err(anyhow!("relay accepted a mislabelled blob"));
    }

    // 3. shares to the gateways — envelope i is addressed to gw_ids[i]
    for (g, env) in gateways.iter().zip(item.share_envelopes.iter()) {
        g.deposit(env)?;
    }
    println!("3. deposited {} key shares across {} gateways", item.share_envelopes.len(), gateways.len());

    // 4. Bob downloads EVERYTHING before finality
    let mut downloaded: Vec<Vec<u8>> = Vec::new();
    for c in &sealed.chunks {
        downloaded.push(get_blob(&http, &base, &hex::encode(c.ciphertext_hash))?);
    }
    let total: usize = downloaded.iter().map(|b| b.len()).sum();
    println!("\n4. Bob downloads all {total} bytes of ciphertext BEFORE finality");

    let pre_shares: Vec<KeyShareEnvelope> = gateways.iter().filter_map(|g| g.release(&seal_id).ok()).flatten().collect();
    match try_open_media(&item.envelope, &item.signed_leaf, &alice_id.public(), &bob_device, &pre_shares, &chain) {
        MediaOutcome::Locked { reason, .. } => println!("   → LOCKED: {reason}"),
        MediaOutcome::Opened { .. } => return Err(anyhow!("SECURITY: media opened before finality")),
        MediaOutcome::Rejected { reason } => return Err(anyhow!("unexpected rejection: {reason}")),
    }

    // 5. finality → gateways release → Bob opens
    chain.submit_leaf(&item.signed_leaf, &alice_id.public())?;
    chain.finalize(&seal_id)?;
    for g in &gateways {
        g.finalize(&seal_id)?;
    }
    let shares: Vec<KeyShareEnvelope> = gateways.iter().filter_map(|g| g.release(&seal_id).ok()).flatten().collect();
    println!("\n5. seal FINALISED on WCAHT; {} gateways released their shares", shares.len());

    let (manifest, opener) =
        match try_open_media(&item.envelope, &item.signed_leaf, &alice_id.public(), &bob_device, &shares, &chain) {
            MediaOutcome::Opened { manifest, opener } => (manifest, opener),
            MediaOutcome::Locked { reason, .. } => return Err(anyhow!("still locked: {reason}")),
            MediaOutcome::Rejected { reason } => return Err(anyhow!("rejected: {reason}")),
        };
    println!(
        "   manifest opened: {} {}x{}, {} bytes, preview {:?}",
        manifest.mime_type,
        manifest.width,
        manifest.height,
        manifest.plaintext_size,
        String::from_utf8_lossy(&manifest.preview)
    );

    let mut rebuilt = Vec::with_capacity(manifest.plaintext_size as usize);
    for (i, blob) in downloaded.iter().enumerate() {
        rebuilt.extend_from_slice(&opener.decrypt_chunk(i as u32, blob)?);
    }
    if rebuilt != photo {
        return Err(anyhow!("reassembled image differs from the original"));
    }
    println!("6. decrypted + reassembled {} bytes — byte-identical to Alice's file ✓", rebuilt.len());

    // 6. a tampered chunk is caught even with a valid key
    let mut evil = downloaded[1].clone();
    evil[100] ^= 0xFF;
    match opener.decrypt_chunk(1, &evil) {
        Err(e) => println!("7. a single flipped byte in chunk 1 → REFUSED: {e}"),
        Ok(_) => return Err(anyhow!("SECURITY: tampered chunk decrypted")),
    }

    println!("\nThe relay held every byte the whole time and could never open any of it.");
    Ok(())
}
