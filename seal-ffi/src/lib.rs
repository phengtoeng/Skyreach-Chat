//! C ABI over `seal-core` for the Flutter iOS/Android app (called via `dart:ffi`).
//!
//! Every function returns a heap-allocated, NUL-terminated JSON C string that the
//! Dart side must release with [`ss_free`]. This first cut exposes a self-contained
//! on-device demo of the sealed-message flow; the granular per-operation surface
//! (keys, seal, submit, open against real relay/gateway/WCAHT services) is the next
//! phase and slots in behind the same ABI.

use std::ffi::{c_char, CString};

use seal_core::*;
use seal_crypto as sc;
use serde_json::{json, Value};

fn to_c(s: String) -> *mut c_char {
    CString::new(s).unwrap_or_else(|_| CString::new("{}").unwrap()).into_raw()
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

use jni::objects::JClass;
use jni::sys::jstring;
use jni::JNIEnv;

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
}
