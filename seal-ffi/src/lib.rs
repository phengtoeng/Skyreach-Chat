//! C ABI over `seal-core` for the Flutter iOS/Android app (called via `dart:ffi`).
//!
//! Every function returns a heap-allocated, NUL-terminated JSON C string that the
//! Dart side must release with [`ss_free`]. This first cut exposes a self-contained
//! on-device demo of the sealed-message flow; the granular per-operation surface
//! (keys, seal, submit, open against real relay/gateway/WCAHT services) is the next
//! phase and slots in behind the same ABI.

use std::ffi::{c_char, CStr, CString};

use seal_core::*;
use seal_crypto as sc;
use serde_json::{json, Value};

fn to_c(s: String) -> *mut c_char {
    CString::new(s).unwrap_or_else(|_| CString::new("{}").unwrap()).into_raw()
}

/// # Safety
/// `p` must be null or a valid NUL-terminated C string.
unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// WCAHT produces a slot every 400 ms.
const SLOT_MS: u64 = 400;

/// The chain-time floor for a seal: the slot the chain must have FINALISED past before the
/// item may open. Returns `current_slot` unchanged when there is no reveal time, or when the
/// caller could not tell us the chain's position (`current_slot == 0`), in which case the
/// timelock rests on the signed wall-clock value alone.
fn reveal_floor_slot(current_slot: i64, reveal_at: i64) -> u64 {
    let base = current_slot.max(0) as u64;
    if base == 0 || reveal_at <= 0 {
        return base;
    }
    let now = now_unix() as i64;
    let secs_ahead = (reveal_at - now).max(0) as u64;
    base.saturating_add(secs_ahead.saturating_mul(1000) / SLOT_MS)
}

fn seed_from_hex(s: &str) -> [u8; 32] {
    let v = hex::decode(s.trim()).unwrap_or_default();
    let mut out = [0u8; 32];
    if v.len() == 32 {
        out.copy_from_slice(&v);
    }
    out
}

// ── identity / contacts (shared by the C ABI and JNI) ──

/// Create a fresh account. The app must PERSIST `identity_seed` + `device_seed` (in the
/// platform keystore). Returns the address + shareable card.
fn new_identity_json(name: &str) -> String {
    let name = if name.trim().is_empty() { "Me" } else { name.trim() };
    let id = Identity::generate(name);
    json!({
        "name": id.name,
        "address": id.address(),
        "identity_seed": hex::encode(id.identity_seed),
        "device_seed": hex::encode(id.device_seed),
        "identity_pub": hex::encode(id.identity_pub()),
        "device_pub": hex::encode(id.device_pub()),
        "card": id.card().encode(),
    })
    .to_string()
}

/// Rebuild the address + card from stored seeds (e.g. on app relaunch).
fn card_for_json(identity_seed_hex: &str, device_seed_hex: &str, name: &str) -> String {
    let id = Identity::from_seeds(name, seed_from_hex(identity_seed_hex), seed_from_hex(device_seed_hex));
    json!({
        "name": id.name,
        "address": id.address(),
        "identity_pub": hex::encode(id.identity_pub()),
        "device_pub": hex::encode(id.device_pub()),
        "card": id.card().encode(),
    })
    .to_string()
}

/// Seal `text` to a specific contact's device public key (hex). The result is
/// openable ONLY by that device — proof the message is bound to the real contact.
fn seal_to_json(device_pub_hex: &str, text: &str, fast: bool) -> String {
    let dp: Option<[u8; 32]> = hex::decode(device_pub_hex.trim()).ok().and_then(|v| v.try_into().ok());
    let Some(device_pub) = dp else {
        return json!({ "ok": false, "error": "bad device pubkey" }).to_string();
    };
    let sender_id = sc::SignId::generate();
    let sender_dev = sc::SignId::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let mode = if fast { SealMode::FastSeal } else { SealMode::StrictSeal };
    match seal_text_with_mode(text.as_bytes(), &sender_id, &sender_dev, &device_pub, sc::random_32(), &gw_ids, 2, 100, 100_000, mode, 0, 0) {
        Ok(item) => json!({
            "ok": true,
            "seal_id": hex::encode(item.signed_leaf.leaf.seal_id),
            "recipient_device_commitment": hex::encode(item.signed_leaf.leaf.recipient_device_commitment),
            "ciphertext_len": item.envelope.ciphertext.len(),
        })
        .to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

/// Seal `text` to a device and return the artifacts to SHIP over the services:
/// the ciphertext `bundle` (→ delivery relay, keyed by `mailbox_tag`) and the `shares`
/// (one → each gateway). This is real cross-device delivery, not an in-process demo.
///
/// `sender_card` is the SENDER's own contact card, embedded in the bundle so the recipient
/// can identify the sender and REPLY without a prior manual add (self-describing message).
fn seal_shippable_json(
    identity_seed_hex: &str,
    sender_card: &str,
    device_pub_hex: &str,
    text: &str,
    fast: bool,
    reveal_at: i64,
    destroy_at: i64,
    current_slot: i64,
) -> String {
    let dp: Option<[u8; 32]> = hex::decode(device_pub_hex.trim()).ok().and_then(|v| v.try_into().ok());
    let Some(device_pub) = dp else {
        return json!({ "ok": false, "error": "bad device pubkey" }).to_string();
    };
    // Sign with the sender's STABLE chat identity (not an ephemeral key) so the recipient can
    // attribute the message to the right conversation: bundle.sender_id_pub == sender identity_pub.
    let sender_id = sc::SignId::from_seed(&seed_from_hex(identity_seed_hex));
    let sender_dev = sc::SignId::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let tag = mailbox_tag(&device_pub);
    let mode = if fast { SealMode::FastSeal } else { SealMode::StrictSeal };
    let item = match seal_text_with_mode(
        text.as_bytes(), &sender_id, &sender_dev, &device_pub, tag, &gw_ids, 2,
        // not_before_finalized_slot: the chain must finalise past this before it opens
        reveal_floor_slot(current_slot, reveal_at),
        100_000, mode,
        reveal_at.max(0) as u64, destroy_at.max(0) as u64,
    ) {
        Ok(i) => i,
        Err(e) => return json!({ "ok": false, "error": e.to_string() }).to_string(),
    };
    let bundle = json!({
        "envelope": serde_json::to_value(&item.envelope).unwrap_or(Value::Null),
        "signed_leaf": serde_json::to_value(&item.signed_leaf).unwrap_or(Value::Null),
        "sender_id_pub": hex::encode(sender_id.public()),
        "sender_card": sender_card,
        // timelock window (unix secs, 0 = none): carried so the recipient can DISPLAY "opens at T"
        // / "destroyed"; the actual guarantee is the gateways withholding shares outside the window.
        "reveal_at": reveal_at,
        "destroy_at": destroy_at,
    });
    let shares: Vec<Value> = item.share_envelopes.iter().filter_map(|s| serde_json::to_value(s).ok()).collect();
    json!({
        "ok": true,
        "seal_id": hex::encode(item.signed_leaf.leaf.seal_id),
        "mailbox_tag": hex::encode(tag),
        "bundle": bundle,
        "shares": shares,
        "reveal_at": reveal_at,
        "destroy_at": destroy_at,
    })
    .to_string()
}

/// Open a message the recipient COLLECTED from the services: reconstruct the recipient
/// device from `device_seed`, then open the `bundle` (from the relay) with the `shares`
/// (from the gateways). By the time shares are in hand the gateways have already gated
/// on finality, so this treats the seal as finalised.
///
/// Refuses MEDIA leaves outright: a media envelope carries a bincode `MediaManifest`, and
/// running it through here would hand the app binary that `from_utf8_lossy` turns into a
/// bubble full of replacement characters. Callers must route media to `ss_open_media_info`.
fn open_received_json(device_seed_hex: &str, bundle_str: &str, shares_str: &str, current_slot: i64) -> String {
    let recipient = sc::DeviceKey::from_seed(seed_from_hex(device_seed_hex));
    let Ok(bundle): Result<Value, _> = serde_json::from_str(bundle_str) else {
        return json!({ "ok": false, "reason": "bad bundle" }).to_string();
    };
    let envelope: Option<EncryptedEnvelope> = serde_json::from_value(bundle.get("envelope").cloned().unwrap_or(Value::Null)).ok();
    let signed_leaf: Option<SignedLeaf> = serde_json::from_value(bundle.get("signed_leaf").cloned().unwrap_or(Value::Null)).ok();
    let sender_pub: Option<[u8; 32]> = bundle
        .get("sender_id_pub")
        .and_then(Value::as_str)
        .and_then(|s| hex::decode(s).ok())
        .and_then(|v| v.try_into().ok());
    let shares: Vec<KeyShareEnvelope> = serde_json::from_str(shares_str).unwrap_or_default();
    let (Some(envelope), Some(signed_leaf), Some(sender_pub)) = (envelope, signed_leaf, sender_pub) else {
        return json!({ "ok": false, "reason": "incomplete bundle" }).to_string();
    };

    // A media envelope holds a bincode MediaManifest, not text. Opening it here would hand the
    // caller binary that renders as a bubble full of replacement characters, so refuse it and
    // say where to go instead — an app that predates media support then shows nothing rather
    // than garbage.
    if signed_leaf.leaf.content_type != ContentType::Text {
        return json!({
            "ok": false,
            "reason": "not a text item — open media with ss_open_media_info / ss_open_media_file",
            "content_type": kind_of(signed_leaf.leaf.content_type),
        })
        .to_string();
    }

    // Timelock enforcement (defense-in-depth; the gateways are the primary gate that withholds
    // the shares outside the window). Refuse to open before reveal_at or after destroy_at.
    // window from the SIGNED leaf (see open_ctx) — the bundle's copy is unauthenticated
    let now = now_unix();
    if signed_leaf.leaf.destroy_at_unix > 0 && now >= signed_leaf.leaf.destroy_at_unix {
        return json!({ "ok": false, "reason": "destroyed" }).to_string();
    }
    if signed_leaf.leaf.reveal_at_unix > 0 && now < signed_leaf.leaf.reveal_at_unix {
        return json!({ "ok": false, "reason": "locked", "reveal_at": signed_leaf.leaf.reveal_at_unix }).to_string();
    }

    // chain view at the slot the app read from a node — see open_ctx for why
    let view_slot = if current_slot > 0 { current_slot as u64 } else { signed_leaf.leaf.not_before_finalized_slot };
    let mut chain = MockSealChain::new(view_slot);
    let _ = chain.submit_leaf(&signed_leaf, &sender_pub);
    let _ = chain.finalize(&signed_leaf.leaf.seal_id);

    match try_open(&envelope, &signed_leaf, &sender_pub, &recipient, &shares, &chain) {
        OpenOutcome::Opened { plaintext } => json!({ "ok": true, "plaintext": String::from_utf8_lossy(&plaintext) }).to_string(),
        OpenOutcome::Locked { reason, .. } => json!({ "ok": false, "reason": reason }).to_string(),
        OpenOutcome::Rejected { reason } => json!({ "ok": false, "reason": reason }).to_string(),
    }
}

// ─────────────────────────────── media ──────────────────────────────────────
//
// Media crosses this boundary as FILE PATHS, never as bytes: a 40 MB video base64'd
// through JNI would cost several copies of itself in RAM. Rust reads the source file,
// writes each encrypted chunk out as its own file, and returns only JSON metadata.

/// Refuse to seal anything larger than this in one item (this cut holds the plaintext in
/// memory while chunking; true streaming is a follow-up).
const MAX_MEDIA_BYTES: u64 = 64 * 1024 * 1024;

fn kind_from_str(s: &str) -> ContentKind {
    match s.trim().to_ascii_lowercase().as_str() {
        "video" => ContentKind::Video,
        "audio" => ContentKind::Audio,
        "file" => ContentKind::File,
        _ => ContentKind::Image,
    }
}

/// Label for a leaf's content type, for error reporting.
fn kind_of(c: ContentType) -> &'static str {
    match c {
        ContentType::Text => "text",
        ContentType::Media => "media",
        ContentType::Document => "document",
        ContentType::CallSession => "call_session",
    }
}

fn kind_str(k: ContentKind) -> &'static str {
    match k {
        ContentKind::Image => "image",
        ContentKind::Audio => "audio",
        ContentKind::File => "file",
        ContentKind::Video => "video",
    }
}

/// Seal a media file. Writes one file per encrypted chunk into `out_dir`, named by its
/// ciphertext hash, and returns the same `{bundle, shares}` shape a text seal produces —
/// so the existing ship/collect plumbing is unchanged — plus the chunk list to upload.
///
/// `preview_path` must point at an ALREADY blurred/downscaled image; it is sealed inside
/// the manifest and is never uploaded as its own object.
#[allow(clippy::too_many_arguments)]
fn seal_media_file_json(
    identity_seed_hex: &str,
    sender_card: &str,
    device_pub_hex: &str,
    in_path: &str,
    mime: &str,
    kind: &str,
    caption: &str,
    preview_path: &str,
    out_dir: &str,
    fast: bool,
    reveal_at: i64,
    destroy_at: i64,
    current_slot: i64,
) -> String {
    let dp: Option<[u8; 32]> = hex::decode(device_pub_hex.trim()).ok().and_then(|v| v.try_into().ok());
    let Some(device_pub) = dp else {
        return json!({ "ok": false, "error": "bad device pubkey" }).to_string();
    };
    let meta = match std::fs::metadata(in_path) {
        Ok(m) => m,
        Err(e) => return json!({ "ok": false, "error": format!("cannot read media: {e}") }).to_string(),
    };
    if meta.len() == 0 {
        return json!({ "ok": false, "error": "media file is empty" }).to_string();
    }
    if meta.len() > MAX_MEDIA_BYTES {
        return json!({ "ok": false, "error": format!("media too large ({} bytes, cap {MAX_MEDIA_BYTES})", meta.len()) })
            .to_string();
    }
    let bytes = match std::fs::read(in_path) {
        Ok(b) => b,
        Err(e) => return json!({ "ok": false, "error": format!("cannot read media: {e}") }).to_string(),
    };
    // an empty/unreadable preview simply means "no locked preview"
    let preview = if preview_path.trim().is_empty() {
        Vec::new()
    } else {
        std::fs::read(preview_path).unwrap_or_default()
    };
    let policy = if preview.is_empty() { PreviewPolicy::None } else { PreviewPolicy::LockedBlur };

    if let Err(e) = std::fs::create_dir_all(out_dir) {
        return json!({ "ok": false, "error": format!("cannot create out_dir: {e}") }).to_string();
    }

    let sender_id = sc::SignId::from_seed(&seed_from_hex(identity_seed_hex));
    let sender_dev = sc::SignId::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let tag = mailbox_tag(&device_pub);
    let mode = if fast { SealMode::FastSeal } else { SealMode::StrictSeal };

    let sealed = match seal_media_with_mode(
        &bytes,
        kind_from_str(kind),
        mime,
        caption,
        &preview,
        policy,
        (0, 0),
        0,
        DEFAULT_CHUNK_SIZE,
        &sender_id,
        &sender_dev,
        &device_pub,
        tag,
        &gw_ids,
        2,
        reveal_floor_slot(current_slot, reveal_at),
        100_000,
        mode,
        reveal_at.max(0) as u64,
        destroy_at.max(0) as u64,
    ) {
        Ok(s) => s,
        Err(e) => return json!({ "ok": false, "error": e.to_string() }).to_string(),
    };

    // spill the encrypted chunks to disk for the uploader
    let mut chunk_meta = Vec::with_capacity(sealed.chunks.len());
    for c in &sealed.chunks {
        let hex_hash = hex::encode(c.ciphertext_hash);
        let path = format!("{out_dir}/{hex_hash}");
        if let Err(e) = std::fs::write(&path, &c.bytes) {
            return json!({ "ok": false, "error": format!("cannot write chunk: {e}") }).to_string();
        }
        chunk_meta.push(json!({ "index": c.index, "hash": hex_hash, "path": path, "size": c.bytes.len() }));
    }

    let item = &sealed.item;
    let bundle = json!({
        "envelope": serde_json::to_value(&item.envelope).unwrap_or(Value::Null),
        "signed_leaf": serde_json::to_value(&item.signed_leaf).unwrap_or(Value::Null),
        "sender_id_pub": hex::encode(sender_id.public()),
        "sender_card": sender_card,
        "reveal_at": reveal_at,
        "destroy_at": destroy_at,
    });
    let shares: Vec<Value> = item.share_envelopes.iter().filter_map(|s| serde_json::to_value(s).ok()).collect();
    json!({
        "ok": true,
        "seal_id": hex::encode(item.signed_leaf.leaf.seal_id),
        "mailbox_tag": hex::encode(tag),
        "bundle": bundle,
        "shares": shares,
        "chunk_count": sealed.chunks.len(),
        "chunks": chunk_meta,
        "reveal_at": reveal_at,
        "destroy_at": destroy_at,
    })
    .to_string()
}

/// Everything needed to run the recipient gate, parsed out of a relay bundle.
struct OpenCtx {
    envelope: EncryptedEnvelope,
    signed_leaf: SignedLeaf,
    sender_pub: [u8; 32],
    shares: Vec<KeyShareEnvelope>,
    chain: MockSealChain,
}

/// Parse a bundle + shares and apply the timelock window. `Err` is a ready-to-return JSON
/// string. The gateways are the primary gate — they withhold shares outside the window —
/// so the checks here are defence in depth.
fn open_ctx(bundle_str: &str, shares_str: &str, current_slot: i64) -> std::result::Result<OpenCtx, String> {
    let Ok(bundle): Result<Value, _> = serde_json::from_str(bundle_str) else {
        return Err(json!({ "ok": false, "reason": "bad bundle" }).to_string());
    };
    let envelope: Option<EncryptedEnvelope> =
        serde_json::from_value(bundle.get("envelope").cloned().unwrap_or(Value::Null)).ok();
    let signed_leaf: Option<SignedLeaf> =
        serde_json::from_value(bundle.get("signed_leaf").cloned().unwrap_or(Value::Null)).ok();
    let sender_pub: Option<[u8; 32]> = bundle
        .get("sender_id_pub")
        .and_then(Value::as_str)
        .and_then(|s| hex::decode(s).ok())
        .and_then(|v| v.try_into().ok());
    let shares: Vec<KeyShareEnvelope> = serde_json::from_str(shares_str).unwrap_or_default();
    let (Some(envelope), Some(signed_leaf), Some(sender_pub)) = (envelope, signed_leaf, sender_pub) else {
        return Err(json!({ "ok": false, "reason": "incomplete bundle" }).to_string());
    };

    // The window is read from the SIGNED leaf, never from the bundle JSON: the bundle is not
    // covered by any signature, so a relay could have edited a deadline there. `try_open*`
    // enforces the same values again — this is only to give the app a clean reason + deadline.
    let now = now_unix();
    let leaf = &signed_leaf.leaf;
    if leaf.destroy_at_unix > 0 && now >= leaf.destroy_at_unix {
        return Err(json!({ "ok": false, "reason": "destroyed" }).to_string());
    }
    if leaf.reveal_at_unix > 0 && now < leaf.reveal_at_unix {
        return Err(json!({ "ok": false, "reason": "locked", "reveal_at": leaf.reveal_at_unix }).to_string());
    }

    // The gateways gate on finality before releasing, but the recipient should not have to
    // take their word for the CHAIN TIME: building the chain view at the slot the app just read
    // from a node means `release_gate` compares the seal's signed slot floor against real
    // finality here, on the recipient's device.
    //
    // `current_slot == 0` means the app could not reach a node. Fall back to the leaf's own
    // floor — that satisfies the slot gate without pretending the chain has run far ahead
    // (a huge stand-in slot would trip the seal's `expires_at_slot` instead). Offline, the
    // signed wall-clock window and the gateways' release are what still apply.
    let view_slot = if current_slot > 0 { current_slot as u64 } else { signed_leaf.leaf.not_before_finalized_slot };
    let mut chain = MockSealChain::new(view_slot);
    let _ = chain.submit_leaf(&signed_leaf, &sender_pub);
    let _ = chain.finalize(&signed_leaf.leaf.seal_id);
    Ok(OpenCtx { envelope, signed_leaf, sender_pub, shares, chain })
}

/// Step 1 of receiving media: open the MANIFEST only. Tells the app what the item is and
/// which chunks to fetch. Writes the locked preview (if any) to `preview_out` so the app
/// can show something while the chunks download.
fn open_media_info_json(device_seed_hex: &str, bundle_str: &str, shares_str: &str, preview_out: &str, current_slot: i64) -> String {
    let recipient = sc::DeviceKey::from_seed(seed_from_hex(device_seed_hex));
    let ctx = match open_ctx(bundle_str, shares_str, current_slot) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match try_open_media(&ctx.envelope, &ctx.signed_leaf, &ctx.sender_pub, &recipient, &ctx.shares, &ctx.chain) {
        MediaOutcome::Opened { manifest, .. } => {
            if !preview_out.trim().is_empty() && !manifest.preview.is_empty() {
                let _ = std::fs::write(preview_out, &manifest.preview);
            }
            let hashes: Vec<String> = manifest.ciphertext_chunk_hashes.iter().map(hex::encode).collect();
            json!({
                "ok": true,
                "mime_type": manifest.mime_type,
                "kind": kind_str(manifest.content_kind),
                "caption": manifest.caption,
                "plaintext_size": manifest.plaintext_size,
                "chunk_count": manifest.chunk_count,
                "chunks": hashes,
                "width": manifest.width,
                "height": manifest.height,
                "duration_ms": manifest.duration_ms,
                "has_preview": !manifest.preview.is_empty(),
            })
            .to_string()
        }
        MediaOutcome::Locked { reason, .. } => json!({ "ok": false, "reason": reason }).to_string(),
        MediaOutcome::Rejected { reason } => json!({ "ok": false, "reason": reason }).to_string(),
    }
}

/// Step 2: with every chunk downloaded into `chunk_dir` (each file named by its hex hash,
/// exactly as `open_media_info` listed them), decrypt and reassemble into `out_path`.
///
/// A missing or altered chunk fails here rather than producing a corrupt file.
fn open_media_file_json(
    device_seed_hex: &str,
    bundle_str: &str,
    shares_str: &str,
    chunk_dir: &str,
    out_path: &str,
    current_slot: i64,
) -> String {
    let recipient = sc::DeviceKey::from_seed(seed_from_hex(device_seed_hex));
    let ctx = match open_ctx(bundle_str, shares_str, current_slot) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let (manifest, opener) =
        match try_open_media(&ctx.envelope, &ctx.signed_leaf, &ctx.sender_pub, &recipient, &ctx.shares, &ctx.chain) {
            MediaOutcome::Opened { manifest, opener } => (manifest, opener),
            MediaOutcome::Locked { reason, .. } => return json!({ "ok": false, "reason": reason }).to_string(),
            MediaOutcome::Rejected { reason } => return json!({ "ok": false, "reason": reason }).to_string(),
        };

    let mut out = Vec::with_capacity(manifest.plaintext_size as usize);
    for (i, h) in manifest.ciphertext_chunk_hashes.iter().enumerate() {
        let path = format!("{chunk_dir}/{}", hex::encode(h));
        let blob = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return json!({ "ok": false, "reason": format!("chunk {i} missing: {e}") }).to_string(),
        };
        match opener.decrypt_chunk(i as u32, &blob) {
            Ok(plain) => out.extend_from_slice(&plain),
            Err(e) => return json!({ "ok": false, "reason": format!("chunk {i}: {e}") }).to_string(),
        }
    }
    if out.len() as u64 != manifest.plaintext_size {
        return json!({ "ok": false, "reason": "reassembled size does not match the manifest" }).to_string();
    }
    if let Err(e) = std::fs::write(out_path, &out) {
        return json!({ "ok": false, "reason": format!("cannot write output: {e}") }).to_string();
    }
    json!({
        "ok": true,
        "out_path": out_path,
        "mime_type": manifest.mime_type,
        "kind": kind_str(manifest.content_kind),
        "bytes": out.len(),
    })
    .to_string()
}

/// Privacy-preserving directory key for a phone number (never the raw number, never
/// on-chain). `hash(normalized phone)` — what resolves to a WCAHT address in the directory.
fn phone_commitment_json(phone: &str) -> String {
    json!({
        "normalized": normalize_phone(phone),
        "phone_commitment": hex::encode(phone_commitment(phone)),
    })
    .to_string()
}

/// Deterministic relay/inbox address for a device public key (hex):
/// `hash("DSCP-2/mailbox", device_pub)`. A recipient computes this from its OWN device
/// pubkey to poll the relay for inbound sealed messages (senders address the same tag).
fn mailbox_tag_json(device_pub_hex: &str) -> String {
    let dp: Option<[u8; 32]> = hex::decode(device_pub_hex.trim()).ok().and_then(|v| v.try_into().ok());
    match dp {
        Some(device_pub) => json!({ "ok": true, "mailbox_tag": hex::encode(mailbox_tag(&device_pub)) }).to_string(),
        None => json!({ "ok": false, "error": "bad device pubkey" }).to_string(),
    }
}

/// Validate a scanned/pasted contact code into a contact.
fn parse_card_json(code: &str) -> String {
    match ContactCard::decode(code) {
        Ok(c) => json!({
            "ok": true,
            "name": c.name,
            "address": c.address(),
            "identity_pub": hex::encode(c.identity_pub),
            "device_pub": hex::encode(c.device_pub),
        })
        .to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

/// Protocol identity: version + chain id.
#[no_mangle]
pub extern "C" fn ss_version() -> *mut c_char {
    to_c(json!({ "protocol": "DSCP-2", "version": PROTOCOL_VERSION, "chain_id": CHAIN_ID }).to_string())
}

/// Run the full seal → (locked) → finalise → (opened) flow in-process and return
/// a JSON transcript. Lets the mobile app prove the core on-device with one call.
#[no_mangle]
pub extern "C" fn ss_run_demo() -> *mut c_char {
    to_c(run_demo())
}

/// Run the DSCP-2 FastSeal fast path in-process: the item opens on a quorum of
/// slashable gateway pre-confirmations BEFORE L1 finality. Returns a JSON transcript.
#[no_mangle]
pub extern "C" fn ss_run_fast_demo() -> *mut c_char {
    to_c(run_fast_demo())
}

/// Create a new account (identity + device keys). Returns JSON with seeds to persist,
/// the WCAHT address, and the shareable card.
///
/// # Safety
/// `name` must be null or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn ss_new_identity(name: *const c_char) -> *mut c_char {
    to_c(new_identity_json(&cstr(name)))
}

/// Rebuild the address + card from previously-stored seeds.
///
/// # Safety
/// All arguments must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn ss_card_for(
    identity_seed: *const c_char,
    device_seed: *const c_char,
    name: *const c_char,
) -> *mut c_char {
    to_c(card_for_json(&cstr(identity_seed), &cstr(device_seed), &cstr(name)))
}

/// Validate a scanned/pasted contact code (`denvion:…`) into a contact.
///
/// # Safety
/// `code` must be null or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn ss_parse_card(code: *const c_char) -> *mut c_char {
    to_c(parse_card_json(&cstr(code)))
}

/// Directory key for a phone number: `{ normalized, phone_commitment }`.
///
/// # Safety
/// `phone` must be null or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn ss_phone_commitment(phone: *const c_char) -> *mut c_char {
    to_c(phone_commitment_json(&cstr(phone)))
}

/// Inbox/mailbox tag for a device pubkey (hex): `{ ok, mailbox_tag }`. A recipient polls
/// the delivery relay at this tag for inbound sealed messages.
///
/// # Safety
/// `device_pub` must be null or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn ss_mailbox_tag(device_pub: *const c_char) -> *mut c_char {
    to_c(mailbox_tag_json(&cstr(device_pub)))
}

/// Seal `text` to a contact's device public key (hex). Returns
/// `{ ok, seal_id, recipient_device_commitment, ciphertext_len }`.
///
/// # Safety
/// `device_pub` and `text` must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn ss_seal_to(device_pub: *const c_char, text: *const c_char, fast: i32) -> *mut c_char {
    to_c(seal_to_json(&cstr(device_pub), &cstr(text), fast != 0))
}

/// Seal + return shippable artifacts: `{ ok, seal_id, mailbox_tag, bundle, shares }`. `identity_seed`
/// is the SENDER's stored identity seed (so the message is attributable to the sender's identity).
///
/// # Safety
/// `identity_seed`, `device_pub` and `text` must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn ss_seal_shippable(
    identity_seed: *const c_char,
    sender_card: *const c_char,
    device_pub: *const c_char,
    text: *const c_char,
    fast: i32,
    reveal_at: i64,
    destroy_at: i64,
    current_slot: i64,
) -> *mut c_char {
    to_c(seal_shippable_json(&cstr(identity_seed), &cstr(sender_card), &cstr(device_pub), &cstr(text), fast != 0, reveal_at, destroy_at, current_slot))
}

/// Open a collected message: `{ ok, plaintext }` or `{ ok:false, reason }`.
///
/// # Safety
/// All arguments must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn ss_open_received(device_seed: *const c_char, bundle: *const c_char, shares: *const c_char, current_slot: i64) -> *mut c_char {
    to_c(open_received_json(&cstr(device_seed), &cstr(bundle), &cstr(shares), current_slot))
}

/// Seal a media FILE. Chunks land in `out_dir` named by hash; returns `{bundle, shares, chunks}`.
///
/// # Safety
/// All arguments must be null or valid NUL-terminated C strings.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn ss_seal_media_file(
    identity_seed: *const c_char,
    sender_card: *const c_char,
    device_pub: *const c_char,
    in_path: *const c_char,
    mime: *const c_char,
    kind: *const c_char,
    caption: *const c_char,
    preview_path: *const c_char,
    out_dir: *const c_char,
    fast: i32,
    reveal_at: i64,
    destroy_at: i64,
    current_slot: i64,
) -> *mut c_char {
    to_c(seal_media_file_json(
        &cstr(identity_seed),
        &cstr(sender_card),
        &cstr(device_pub),
        &cstr(in_path),
        &cstr(mime),
        &cstr(kind),
        &cstr(caption),
        &cstr(preview_path),
        &cstr(out_dir),
        fast != 0,
        reveal_at,
        destroy_at,
        current_slot,
    ))
}

/// Media step 1: open the manifest → `{ ok, mime_type, kind, chunks:[hash] }`.
///
/// # Safety
/// All arguments must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn ss_open_media_info(
    device_seed: *const c_char,
    bundle: *const c_char,
    shares: *const c_char,
    preview_out: *const c_char,
    current_slot: i64,
) -> *mut c_char {
    to_c(open_media_info_json(&cstr(device_seed), &cstr(bundle), &cstr(shares), &cstr(preview_out), current_slot))
}

/// Media step 2: reassemble the downloaded chunks into `out_path`.
///
/// # Safety
/// All arguments must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn ss_open_media_file(
    device_seed: *const c_char,
    bundle: *const c_char,
    shares: *const c_char,
    chunk_dir: *const c_char,
    out_path: *const c_char,
    current_slot: i64,
) -> *mut c_char {
    to_c(open_media_file_json(&cstr(device_seed), &cstr(bundle), &cstr(shares), &cstr(chunk_dir), &cstr(out_path), current_slot))
}

/// Free a string returned by any `ss_*` function. Safe on null.
///
/// # Safety
/// `ptr` must be a value previously returned by an `ss_*` function and not freed before.
#[no_mangle]
pub unsafe extern "C" fn ss_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

fn describe(o: &OpenOutcome) -> Value {
    match o {
        OpenOutcome::Locked { state, reason } => {
            json!({ "result": "LOCKED", "state": format!("{state:?}"), "reason": reason })
        }
        OpenOutcome::Opened { plaintext } => {
            json!({ "result": "OPENED", "plaintext": String::from_utf8_lossy(plaintext) })
        }
        OpenOutcome::Rejected { reason } => json!({ "result": "REJECTED", "reason": reason }),
    }
}

fn run_demo() -> String {
    let sender_id = sc::SignId::generate();
    let sender_dev = sc::SignId::generate();
    let bob = sc::DeviceKey::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let mut gateways: Vec<Gateway> = gw_ids.iter().map(|id| Gateway::new(*id)).collect();
    let mut chain = MockSealChain::new(1000);
    let mut transcript: Vec<Value> = Vec::new();

    let item = match seal_text(
        b"Sealed by WCAHT before it opens.",
        &sender_id,
        &sender_dev,
        &bob.public(),
        sc::random_32(),
        &gw_ids,
        2,
        chain.slot(),
        500,
    ) {
        Ok(i) => i,
        Err(e) => return json!({ "error": e.to_string() }).to_string(),
    };
    for e in &item.share_envelopes {
        if let Some(g) = gateways.iter_mut().find(|g| g.id == e.gateway_id) {
            g.deposit(e.clone());
        }
    }
    let seal_id = match chain.submit_leaf(&item.signed_leaf, &sender_id.public()) {
        Ok(id) => id,
        Err(e) => return json!({ "error": e.to_string() }).to_string(),
    };
    transcript.push(json!({ "step": "submitted", "status": format!("{:?}", chain.status(&seal_id)) }));

    let collect = |chain: &MockSealChain, gws: &[Gateway]| -> Vec<KeyShareEnvelope> {
        gws.iter().filter_map(|g| g.request_share(&seal_id, chain)).collect()
    };

    // Before finality — must be locked, gateways release nothing.
    let shares = collect(&chain, &gateways);
    let before = try_open(&item.envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &chain);
    transcript.push(json!({
        "step": "before_finality",
        "shares_released": shares.len(),
        "outcome": describe(&before),
    }));

    // Finalise on WCAHT → gateways release → opens.
    let _ = chain.finalize(&seal_id);
    let shares = collect(&chain, &gateways);
    let after = try_open(&item.envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &chain);
    transcript.push(json!({
        "step": "after_finality",
        "status": format!("{:?}", chain.status(&seal_id)),
        "shares_released": shares.len(),
        "outcome": describe(&after),
    }));

    json!({
        "demo": "denvion-splitseal",
        "protocol": "DSCP-1",
        "chain_id": CHAIN_ID,
        "transcript": transcript,
    })
    .to_string()
}

/// DSCP-2 FastSeal: a staked-gateway pre-confirmation quorum opens the item before
/// hard L1 finality (spec §4 fast path).
fn run_fast_demo() -> String {
    let sender_id = sc::SignId::generate();
    let sender_dev = sc::SignId::generate();
    let bob = sc::DeviceKey::generate();
    // Staked, signing gateways — they can issue slashable pre-confirmations.
    let signers: Vec<sc::SignId> = (0..3).map(|_| sc::SignId::generate()).collect();
    let gw_ids: Vec<[u8; 32]> = signers.iter().map(|s| s.public()).collect();
    let mut gateways: Vec<Gateway> = signers.into_iter().map(Gateway::with_identity).collect();
    let chain = MockSealChain::new(1000);
    let mut transcript: Vec<Value> = Vec::new();

    let item = match seal_text_with_mode(
        b"FastSeal: opened by gateway pre-confirmations, before L1 finality.",
        &sender_id,
        &sender_dev,
        &bob.public(),
        sc::random_32(),
        &gw_ids,
        2,
        chain.slot(),
        500,
        SealMode::FastSeal,
        0,
        0,
    ) {
        Ok(i) => i,
        Err(e) => return json!({ "error": e.to_string() }).to_string(),
    };
    for e in &item.share_envelopes {
        if let Some(g) = gateways.iter_mut().find(|g| g.id == e.gateway_id) {
            g.deposit(e.clone());
        }
    }
    let seal_id = item.signed_leaf.leaf.seal_id;

    // Fast-released shares are in hand, but with NO pre-confirmations yet → still locked.
    let shares: Vec<_> = gateways.iter().filter_map(|g| g.request_share_fast(&seal_id)).collect();
    let before = try_open_dscp2(&item.envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &[], &chain);
    transcript.push(json!({
        "step": "before_preconf",
        "mode": "FastSeal",
        "finalised": false,
        "outcome": describe(&before),
    }));

    // A quorum of gateway pre-confirmations arrives (sub-250ms) → opens WITHOUT finality.
    let preconfs: Vec<_> = gateways.iter().filter_map(|g| g.pre_confirm(&seal_id, chain.slot())).collect();
    let after = try_open_dscp2(&item.envelope, &item.signed_leaf, &sender_id.public(), &bob, &shares, &preconfs, &chain);
    transcript.push(json!({
        "step": "after_preconf_quorum",
        "preconfs": preconfs.len(),
        "finalised": chain.proof(&seal_id).is_some(),
        "outcome": describe(&after),
    }));

    json!({
        "demo": "denvion-splitseal",
        "protocol": "DSCP-2",
        "mode": "FastSeal",
        "chain_id": CHAIN_ID,
        "transcript": transcript,
    })
    .to_string()
}

// ───────────────── Android JNI bridge (Kotlin `com.denvion.splitseal.SealCore`) ─────────────────
// Kotlin declares: external fun nativeVersion(): String / external fun nativeRunDemo(): String
// The C ABI above serves the iOS/Swift side; these serve JVM/Android.

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(|v| v.into()).unwrap_or_default()
}
fn ret(env: JNIEnv, s: String) -> jstring {
    match env.new_string(s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeVersion<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let s = json!({ "protocol": "DSCP-2", "version": PROTOCOL_VERSION, "chain_id": CHAIN_ID }).to_string();
    match env.new_string(s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeRunDemo<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    match env.new_string(run_demo()) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeRunFastDemo<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    match env.new_string(run_fast_demo()) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeNewIdentity<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> jstring {
    let name = jstr(&mut env, &name);
    ret(env, new_identity_json(&name))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeCardFor<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    identity_seed: JString<'local>,
    device_seed: JString<'local>,
    name: JString<'local>,
) -> jstring {
    let i = jstr(&mut env, &identity_seed);
    let d = jstr(&mut env, &device_seed);
    let n = jstr(&mut env, &name);
    ret(env, card_for_json(&i, &d, &n))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeParseCard<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    code: JString<'local>,
) -> jstring {
    let code = jstr(&mut env, &code);
    ret(env, parse_card_json(&code))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativePhoneCommitment<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    phone: JString<'local>,
) -> jstring {
    let phone = jstr(&mut env, &phone);
    ret(env, phone_commitment_json(&phone))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeMailboxTag<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_pub: JString<'local>,
) -> jstring {
    let dp = jstr(&mut env, &device_pub);
    ret(env, mailbox_tag_json(&dp))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeSealTo<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_pub: JString<'local>,
    text: JString<'local>,
    fast: jni::sys::jboolean,
) -> jstring {
    let dp = jstr(&mut env, &device_pub);
    let t = jstr(&mut env, &text);
    ret(env, seal_to_json(&dp, &t, fast != 0))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeSealShippable<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    identity_seed: JString<'local>,
    sender_card: JString<'local>,
    device_pub: JString<'local>,
    text: JString<'local>,
    fast: jni::sys::jboolean,
    reveal_at: jni::sys::jlong,
    destroy_at: jni::sys::jlong,
    current_slot: jni::sys::jlong,
) -> jstring {
    let is = jstr(&mut env, &identity_seed);
    let sc = jstr(&mut env, &sender_card);
    let dp = jstr(&mut env, &device_pub);
    let t = jstr(&mut env, &text);
    ret(env, seal_shippable_json(&is, &sc, &dp, &t, fast != 0, reveal_at as i64, destroy_at as i64, current_slot as i64))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeOpenReceived<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_seed: JString<'local>,
    bundle: JString<'local>,
    shares: JString<'local>,
    current_slot: jni::sys::jlong,
) -> jstring {
    let ds = jstr(&mut env, &device_seed);
    let b = jstr(&mut env, &bundle);
    let s = jstr(&mut env, &shares);
    ret(env, open_received_json(&ds, &b, &s, current_slot as i64))
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeSealMediaFile<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    identity_seed: JString<'local>,
    sender_card: JString<'local>,
    device_pub: JString<'local>,
    in_path: JString<'local>,
    mime: JString<'local>,
    kind: JString<'local>,
    caption: JString<'local>,
    preview_path: JString<'local>,
    out_dir: JString<'local>,
    fast: jni::sys::jboolean,
    reveal_at: jni::sys::jlong,
    destroy_at: jni::sys::jlong,
    current_slot: jni::sys::jlong,
) -> jstring {
    let is = jstr(&mut env, &identity_seed);
    let card = jstr(&mut env, &sender_card);
    let dp = jstr(&mut env, &device_pub);
    let ip = jstr(&mut env, &in_path);
    let mt = jstr(&mut env, &mime);
    let kd = jstr(&mut env, &kind);
    let cap = jstr(&mut env, &caption);
    let pv = jstr(&mut env, &preview_path);
    let od = jstr(&mut env, &out_dir);
    ret(
        env,
        seal_media_file_json(&is, &card, &dp, &ip, &mt, &kd, &cap, &pv, &od, fast != 0, reveal_at as i64, destroy_at as i64, current_slot as i64),
    )
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeOpenMediaInfo<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_seed: JString<'local>,
    bundle: JString<'local>,
    shares: JString<'local>,
    preview_out: JString<'local>,
    current_slot: jni::sys::jlong,
) -> jstring {
    let ds = jstr(&mut env, &device_seed);
    let b = jstr(&mut env, &bundle);
    let s = jstr(&mut env, &shares);
    let p = jstr(&mut env, &preview_out);
    ret(env, open_media_info_json(&ds, &b, &s, &p, current_slot as i64))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeOpenMediaFile<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_seed: JString<'local>,
    bundle: JString<'local>,
    shares: JString<'local>,
    chunk_dir: JString<'local>,
    out_path: JString<'local>,
    current_slot: jni::sys::jlong,
) -> jstring {
    let ds = jstr(&mut env, &device_seed);
    let b = jstr(&mut env, &bundle);
    let s = jstr(&mut env, &shares);
    let cd = jstr(&mut env, &chunk_dir);
    let op = jstr(&mut env, &out_path);
    ret(env, open_media_file_json(&ds, &b, &s, &cd, &op, current_slot as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_shows_locked_then_opened() {
        let s = run_demo();
        assert!(s.contains("LOCKED"), "must be locked before finality: {s}");
        assert!(s.contains("OPENED"), "must open after finality: {s}");
        assert!(s.contains("Sealed by WCAHT before it opens."));
    }

    #[test]
    fn fast_demo_opens_before_finality() {
        let s = run_fast_demo();
        assert!(s.contains("LOCKED"), "locked before any pre-confirmations: {s}");
        assert!(s.contains("OPENED"), "opens on the pre-conf quorum: {s}");
        assert!(s.contains("FastSeal") && s.contains("DSCP-2"), "labels the fast path: {s}");
    }

    #[test]
    fn ffi_helpers_never_panic_on_untrusted_input() {
        // Anything the mobile fields can feed in must NEVER panic — a panic unwinding across
        // the JNI/C boundary aborts the whole app (the "app crash" the user hit).
        let long = "z".repeat(5000);
        let junk = [
            "", "   ", "denvion:", "denvion:!!!", "denvion:2g", "not a code at all",
            "D1os7GLQJUunBL", "😀🔒", "denvion:0OIl", "0x1234", "855 12 345 678",
            "+855123", "deadbeef", "denvion:z", long.as_str(),
        ];
        for s in junk {
            let _ = parse_card_json(s);
            let _ = mailbox_tag_json(s);
            let _ = phone_commitment_json(s);
            let _ = seal_to_json(s, s, false);
            let _ = seal_shippable_json(s, s, s, s, true, 0, 0, 0);
            let _ = card_for_json(s, s, s);
            let _ = open_received_json(s, s, s, 0);
            // the media entry points take PATHS from the platform — equally untrusted
            let _ = seal_media_file_json(s, s, s, s, s, s, s, s, s, false, 0, 0, 0);
            let _ = open_media_info_json(s, s, s, s, 0);
            let _ = open_media_file_json(s, s, s, s, s, 0);
        }
    }

    /// A scratch dir under the OS temp dir, removed on drop.
    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("ss-media-{tag}-{}", hex::encode(sc::random_32())));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn join(&self, n: &str) -> String {
            self.0.join(n).to_string_lossy().into_owned()
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn media_file_round_trips_through_the_ffi() {
        let tmp = Tmp::new("rt");
        let alice: Value = serde_json::from_str(&new_identity_json("Alice")).unwrap();
        let bob: Value = serde_json::from_str(&new_identity_json("Bob")).unwrap();

        // a payload big enough to span several 1 MiB chunks, with a partial last one
        let picture: Vec<u8> = (0..(2 * 1024 * 1024 + 12_345u32)).map(|i| (i % 251) as u8).collect();
        let src = tmp.join("in.jpg");
        std::fs::write(&src, &picture).unwrap();
        std::fs::write(tmp.join("preview.jpg"), b"blurred").unwrap();

        let chunks_dir = tmp.join("chunks");
        let sealed: Value = serde_json::from_str(&seal_media_file_json(
            alice["identity_seed"].as_str().unwrap(),
            alice["card"].as_str().unwrap(),
            bob["device_pub"].as_str().unwrap(),
            &src,
            "image/jpeg",
            "image",
            "a caption only the recipient can read",
            &tmp.join("preview.jpg"),
            &chunks_dir,
            false,
            0,
            0,
            0,
        ))
        .unwrap();
        assert_eq!(sealed["ok"], true, "{sealed}");
        assert_eq!(sealed["chunk_count"], 3);

        let bundle = sealed["bundle"].to_string();
        let shares = sealed["shares"].to_string();

        // step 1: the manifest tells the recipient what to fetch
        let info: Value =
            serde_json::from_str(&open_media_info_json(bob["device_seed"].as_str().unwrap(), &bundle, &shares, &tmp.join("pv.out"), 0))
                .unwrap();
        assert_eq!(info["ok"], true, "{info}");
        assert_eq!(info["mime_type"], "image/jpeg");
        assert_eq!(info["kind"], "image");
        assert_eq!(info["caption"], "a caption only the recipient can read");
        assert_eq!(info["chunk_count"], 3);
        assert_eq!(info["plaintext_size"], picture.len() as u64);
        assert_eq!(std::fs::read(tmp.join("pv.out")).unwrap(), b"blurred");

        // step 2: the app "downloads" the chunks — here, straight from where they were written
        let out = tmp.join("out.jpg");
        let done: Value = serde_json::from_str(&open_media_file_json(
            bob["device_seed"].as_str().unwrap(),
            &bundle,
            &shares,
            &chunks_dir,
            &out,
            0,
        ))
        .unwrap();
        assert_eq!(done["ok"], true, "{done}");
        assert_eq!(std::fs::read(&out).unwrap(), picture, "reassembled bytes must be identical");
    }

    #[test]
    fn timed_media_is_gated_the_same_way_text_is() {
        let tmp = Tmp::new("timed");
        let alice: Value = serde_json::from_str(&new_identity_json("Alice")).unwrap();
        let bob: Value = serde_json::from_str(&new_identity_json("Bob")).unwrap();
        let src = tmp.join("in.jpg");
        std::fs::write(&src, vec![3u8; 4096]).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

        let seal = |reveal: i64, destroy: i64, dir: &str| -> Value {
            serde_json::from_str(&seal_media_file_json(
                alice["identity_seed"].as_str().unwrap(),
                alice["card"].as_str().unwrap(),
                bob["device_pub"].as_str().unwrap(),
                &src, "image/jpeg", "image", "", "", &tmp.join(dir), false, reveal, destroy, 0,
            ))
            .unwrap()
        };
        let info = |m: &Value| -> Value {
            serde_json::from_str(&open_media_info_json(
                bob["device_seed"].as_str().unwrap(),
                &m["bundle"].to_string(),
                &m["shares"].to_string(),
                "",
                0,
            ))
            .unwrap()
        };

        // reveal_at in the future → LOCKED, and the caller is told when it opens
        let later = seal(now + 3600, 0, "c1");
        let r = info(&later);
        assert_eq!(r["ok"], false, "timelocked media must not open: {r}");
        assert_eq!(r["reason"], "locked");
        assert_eq!(r["reveal_at"], now + 3600, "app needs the deadline to run a countdown");

        // destroy_at in the past → DESTROYED, never openable again
        let gone = seal(0, now - 1, "c2");
        let r = info(&gone);
        assert_eq!(r["ok"], false, "self-destructed media must not open: {r}");
        assert_eq!(r["reason"], "destroyed");

        // inside the window → opens normally
        let live = seal(now - 10, now + 3600, "c3");
        let r = info(&live);
        assert_eq!(r["ok"], true, "media inside its window must open: {r}");
        assert_eq!(r["mime_type"], "image/jpeg");
    }

    #[test]
    fn a_media_seal_never_opens_through_the_text_path() {
        // Regression: a media envelope carries a bincode manifest. Opening it as text handed
        // the app binary, which rendered as a bubble of U+FFFD replacement characters.
        let tmp = Tmp::new("mixed");
        let alice: Value = serde_json::from_str(&new_identity_json("Alice")).unwrap();
        let bob: Value = serde_json::from_str(&new_identity_json("Bob")).unwrap();
        let src = tmp.join("in.jpg");
        std::fs::write(&src, vec![9u8; 2048]).unwrap();
        let sealed: Value = serde_json::from_str(&seal_media_file_json(
            alice["identity_seed"].as_str().unwrap(),
            alice["card"].as_str().unwrap(),
            bob["device_pub"].as_str().unwrap(),
            &src,
            "image/jpeg",
            "image",
            "",
            "",
            &tmp.join("chunks"),
            false,
            0,
            0,
            0,
        ))
        .unwrap();
        assert_eq!(sealed["ok"], true, "{sealed}");

        let opened: Value = serde_json::from_str(&open_received_json(
            bob["device_seed"].as_str().unwrap(),
            &sealed["bundle"].to_string(),
            &sealed["shares"].to_string(),
            0,
        ))
        .unwrap();
        assert_eq!(opened["ok"], false, "text path must refuse a media seal: {opened}");
        assert_eq!(opened["content_type"], "media");
        assert!(opened.get("plaintext").is_none(), "must never hand back manifest bytes");
    }

    #[test]
    fn wrong_device_cannot_open_media_and_a_missing_chunk_fails_loudly() {
        let tmp = Tmp::new("neg");
        let alice: Value = serde_json::from_str(&new_identity_json("Alice")).unwrap();
        let bob: Value = serde_json::from_str(&new_identity_json("Bob")).unwrap();
        let mallory: Value = serde_json::from_str(&new_identity_json("Mallory")).unwrap();

        let src = tmp.join("in.bin");
        std::fs::write(&src, vec![7u8; 4096]).unwrap();
        let chunks_dir = tmp.join("chunks");
        let sealed: Value = serde_json::from_str(&seal_media_file_json(
            alice["identity_seed"].as_str().unwrap(),
            alice["card"].as_str().unwrap(),
            bob["device_pub"].as_str().unwrap(),
            &src,
            "video/mp4",
            "video",
            "",
            "",
            &chunks_dir,
            false,
            0,
            0,
            0,
        ))
        .unwrap();
        assert_eq!(sealed["ok"], true, "{sealed}");
        let bundle = sealed["bundle"].to_string();
        let shares = sealed["shares"].to_string();

        // an item addressed to Bob must not open for Mallory, even holding every chunk
        let bad: Value =
            serde_json::from_str(&open_media_info_json(mallory["device_seed"].as_str().unwrap(), &bundle, &shares, "", 0)).unwrap();
        assert_eq!(bad["ok"], false, "media opened for the wrong device: {bad}");

        // and a chunk the relay never served fails rather than yielding a truncated file
        std::fs::remove_dir_all(&chunks_dir).unwrap();
        std::fs::create_dir_all(&chunks_dir).unwrap();
        let missing: Value = serde_json::from_str(&open_media_file_json(
            bob["device_seed"].as_str().unwrap(),
            &bundle,
            &shares,
            &chunks_dir,
            &tmp.join("out.bin"),
            0,
        ))
        .unwrap();
        assert_eq!(missing["ok"], false);
        assert!(missing["reason"].as_str().unwrap().contains("missing"), "{missing}");
    }

    #[test]
    fn mailbox_tag_is_deterministic_and_matches_shippable() {
        let bob: Value = serde_json::from_str(&new_identity_json("Bob")).unwrap();
        let device_pub = bob["device_pub"].as_str().unwrap();

        // a recipient computes its own inbox tag; it must be stable...
        let t1: Value = serde_json::from_str(&mailbox_tag_json(device_pub)).unwrap();
        let t2: Value = serde_json::from_str(&mailbox_tag_json(device_pub)).unwrap();
        assert_eq!(t1["ok"], true);
        assert_eq!(t1["mailbox_tag"], t2["mailbox_tag"]);

        // ...and equal to the tag the SENDER ships to, so polling it receives the seal.
        let seed = bob["identity_seed"].as_str().unwrap();
        let card = bob["card"].as_str().unwrap();
        let ship: Value = serde_json::from_str(&seal_shippable_json(seed, card, device_pub, "hi", false, 0, 0, 0)).unwrap();
        assert_eq!(ship["ok"], true);
        assert_eq!(ship["mailbox_tag"], t1["mailbox_tag"]);

        // a bad pubkey is rejected, not a panic.
        let bad: Value = serde_json::from_str(&mailbox_tag_json("nothex")).unwrap();
        assert_eq!(bad["ok"], false);
    }

    #[test]
    fn app_receive_roundtrip_opens_shipped_seal() {
        // mirrors the mobile path: sender seals to Bob's device_pub and ships {seal_id,bundle}
        // + shares; Bob rebuilds his device from his stored seed and opens.
        let bob: Value = serde_json::from_str(&new_identity_json("Bob")).unwrap();
        let device_pub = bob["device_pub"].as_str().unwrap();
        let device_seed = bob["device_seed"].as_str().unwrap();
        let alice: Value = serde_json::from_str(&new_identity_json("Alice")).unwrap();
        let alice_seed = alice["identity_seed"].as_str().unwrap();
        let alice_card = alice["card"].as_str().unwrap();

        let ship: Value = serde_json::from_str(&seal_shippable_json(alice_seed, alice_card, device_pub, "meet at 9", false, 0, 0, 0)).unwrap();
        // attribution: the bundle names ALICE's stable identity as the sender.
        assert_eq!(ship["bundle"]["sender_id_pub"], alice["identity_pub"]);
        // self-describing: the bundle carries Alice's card so Bob can reply without adding her first.
        let parsed: Value = serde_json::from_str(&parse_card_json(ship["bundle"]["sender_card"].as_str().unwrap())).unwrap();
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["identity_pub"], alice["identity_pub"]);
        assert_eq!(parsed["device_pub"], alice["device_pub"]);
        let bundle = ship["bundle"].to_string();
        let shares = ship["shares"].to_string();

        let opened: Value = serde_json::from_str(&open_received_json(device_seed, &bundle, &shares, 0)).unwrap();
        assert_eq!(opened["ok"], true, "should open: {opened}");
        assert_eq!(opened["plaintext"], "meet at 9");

        // the wrong device seed must NOT open it.
        let mallory: Value = serde_json::from_str(&new_identity_json("Mallory")).unwrap();
        let wrong_seed = mallory["device_seed"].as_str().unwrap();
        let denied: Value = serde_json::from_str(&open_received_json(wrong_seed, &bundle, &shares, 0)).unwrap();
        assert_eq!(denied["ok"], false, "wrong device must be denied: {denied}");
    }

    #[test]
    fn timelock_reveal_and_destroy_are_enforced_on_open() {
        let bob: Value = serde_json::from_str(&new_identity_json("Bob")).unwrap();
        let (dp, ds) = (bob["device_pub"].as_str().unwrap(), bob["device_seed"].as_str().unwrap());
        let alice: Value = serde_json::from_str(&new_identity_json("Alice")).unwrap();
        let (aseed, acard) = (alice["identity_seed"].as_str().unwrap(), alice["card"].as_str().unwrap());
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

        // reveal_at in the future -> LOCKED even with the shares in hand.
        let s1: Value = serde_json::from_str(&seal_shippable_json(aseed, acard, dp, "future", false, now + 3600, 0, 0)).unwrap();
        let o1: Value = serde_json::from_str(&open_received_json(ds, &s1["bundle"].to_string(), &s1["shares"].to_string(), 0)).unwrap();
        assert_eq!(o1["ok"], false);
        assert_eq!(o1["reason"], "locked");

        // destroy_at in the past -> DESTROYED, unopenable.
        let s2: Value = serde_json::from_str(&seal_shippable_json(aseed, acard, dp, "gone", false, 0, now - 10, 0)).unwrap();
        let o2: Value = serde_json::from_str(&open_received_json(ds, &s2["bundle"].to_string(), &s2["shares"].to_string(), 0)).unwrap();
        assert_eq!(o2["ok"], false);
        assert_eq!(o2["reason"], "destroyed");

        // a window that is open right now -> opens fine.
        let s3: Value = serde_json::from_str(&seal_shippable_json(aseed, acard, dp, "now", false, now - 10, now + 3600, 0)).unwrap();
        let o3: Value = serde_json::from_str(&open_received_json(ds, &s3["bundle"].to_string(), &s3["shares"].to_string(), 0)).unwrap();
        assert_eq!(o3["ok"], true, "open window should open: {o3}");
        assert_eq!(o3["plaintext"], "now");
    }
}
