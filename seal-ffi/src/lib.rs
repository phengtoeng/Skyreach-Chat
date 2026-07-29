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
    match seal_text_with_mode(text.as_bytes(), &sender_id, &sender_dev, &device_pub, sc::random_32(), &gw_ids, 2, 100, 100_000, mode) {
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
fn seal_shippable_json(identity_seed_hex: &str, device_pub_hex: &str, text: &str, fast: bool) -> String {
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
    let item = match seal_text_with_mode(text.as_bytes(), &sender_id, &sender_dev, &device_pub, tag, &gw_ids, 2, 100, 100_000, mode) {
        Ok(i) => i,
        Err(e) => return json!({ "ok": false, "error": e.to_string() }).to_string(),
    };
    let bundle = json!({
        "envelope": serde_json::to_value(&item.envelope).unwrap_or(Value::Null),
        "signed_leaf": serde_json::to_value(&item.signed_leaf).unwrap_or(Value::Null),
        "sender_id_pub": hex::encode(sender_id.public()),
    });
    let shares: Vec<Value> = item.share_envelopes.iter().filter_map(|s| serde_json::to_value(s).ok()).collect();
    json!({
        "ok": true,
        "seal_id": hex::encode(item.signed_leaf.leaf.seal_id),
        "mailbox_tag": hex::encode(tag),
        "bundle": bundle,
        "shares": shares,
    })
    .to_string()
}

/// Open a message the recipient COLLECTED from the services: reconstruct the recipient
/// device from `device_seed`, then open the `bundle` (from the relay) with the `shares`
/// (from the gateways). By the time shares are in hand the gateways have already gated
/// on finality, so this treats the seal as finalised.
fn open_received_json(device_seed_hex: &str, bundle_str: &str, shares_str: &str) -> String {
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

    // gateways already enforced finality before releasing, so mark the seal finalised.
    let mut chain = MockSealChain::new(1000);
    let _ = chain.submit_leaf(&signed_leaf, &sender_pub);
    let _ = chain.finalize(&signed_leaf.leaf.seal_id);

    match try_open(&envelope, &signed_leaf, &sender_pub, &recipient, &shares, &chain) {
        OpenOutcome::Opened { plaintext } => json!({ "ok": true, "plaintext": String::from_utf8_lossy(&plaintext) }).to_string(),
        OpenOutcome::Locked { reason, .. } => json!({ "ok": false, "reason": reason }).to_string(),
        OpenOutcome::Rejected { reason } => json!({ "ok": false, "reason": reason }).to_string(),
    }
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
pub unsafe extern "C" fn ss_seal_shippable(identity_seed: *const c_char, device_pub: *const c_char, text: *const c_char, fast: i32) -> *mut c_char {
    to_c(seal_shippable_json(&cstr(identity_seed), &cstr(device_pub), &cstr(text), fast != 0))
}

/// Open a collected message: `{ ok, plaintext }` or `{ ok:false, reason }`.
///
/// # Safety
/// All arguments must be null or valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn ss_open_received(device_seed: *const c_char, bundle: *const c_char, shares: *const c_char) -> *mut c_char {
    to_c(open_received_json(&cstr(device_seed), &cstr(bundle), &cstr(shares)))
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
    device_pub: JString<'local>,
    text: JString<'local>,
    fast: jni::sys::jboolean,
) -> jstring {
    let is = jstr(&mut env, &identity_seed);
    let dp = jstr(&mut env, &device_pub);
    let t = jstr(&mut env, &text);
    ret(env, seal_shippable_json(&is, &dp, &t, fast != 0))
}

#[no_mangle]
pub extern "system" fn Java_com_denvion_splitseal_SealCore_nativeOpenReceived<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    device_seed: JString<'local>,
    bundle: JString<'local>,
    shares: JString<'local>,
) -> jstring {
    let ds = jstr(&mut env, &device_seed);
    let b = jstr(&mut env, &bundle);
    let s = jstr(&mut env, &shares);
    ret(env, open_received_json(&ds, &b, &s))
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
            let _ = seal_shippable_json(s, s, s, true);
            let _ = card_for_json(s, s, s);
            let _ = open_received_json(s, s, s);
        }
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
        let ship: Value = serde_json::from_str(&seal_shippable_json(seed, device_pub, "hi", false)).unwrap();
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

        let ship: Value = serde_json::from_str(&seal_shippable_json(alice_seed, device_pub, "meet at 9", false)).unwrap();
        // attribution: the bundle names ALICE's stable identity as the sender.
        assert_eq!(ship["bundle"]["sender_id_pub"], alice["identity_pub"]);
        let bundle = ship["bundle"].to_string();
        let shares = ship["shares"].to_string();

        let opened: Value = serde_json::from_str(&open_received_json(device_seed, &bundle, &shares)).unwrap();
        assert_eq!(opened["ok"], true, "should open: {opened}");
        assert_eq!(opened["plaintext"], "meet at 9");

        // the wrong device seed must NOT open it.
        let mallory: Value = serde_json::from_str(&new_identity_json("Mallory")).unwrap();
        let wrong_seed = mallory["device_seed"].as_str().unwrap();
        let denied: Value = serde_json::from_str(&open_received_json(wrong_seed, &bundle, &shares)).unwrap();
        assert_eq!(denied["ok"], false, "wrong device must be denied: {denied}");
    }
}
