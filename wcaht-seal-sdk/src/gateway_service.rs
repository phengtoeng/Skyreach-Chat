//! Seal gateway service (DSCP-2). Holds encrypted key-share envelopes and releases
//! them ONLY after the seal is finalised — the strict-release rule, as a real HTTP
//! service (one of the 3+ independent gateways a t-of-n seal is split across).
//!
//! Wire contract:
//!   POST /deposit                     body = KeyShareEnvelope JSON     → 200
//!   POST /finalize/<hex seal_id>      (the gateway observed L1 finality) → 200
//!        optional body { signed_leaf, sender_id_pub } installs the seal's timelock window,
//!        taken from the SIGNED leaf — a bare {reveal_at,destroy_at} is refused
//!   GET  /release/<hex seal_id>       → [KeyShareEnvelope]  (425 if not finalised yet)

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use seal_core::{KeyShareEnvelope, SignedLeaf};
use serde_json::Value;

#[derive(Default)]
struct GwStore {
    shares: HashMap<String, Vec<KeyShareEnvelope>>, // seal_id hex -> held shares
    finalised: HashSet<String>,
    // Optional timelock window per seal (unix secs, 0 = none). The gateway withholds the key
    // share BEFORE reveal_at and drops it AFTER destroy_at — so the recipient physically cannot
    // reconstruct the key outside the window. This is what makes timelock/self-destruct
    // cryptographic (key-level), not a client-side "please delete" policy.
    windows: HashMap<String, (i64, i64)>,
    /// Chain-time floor from the verified leaf: the slot the chain must have FINALISED past
    /// before this seal's share may be released. Wall-clock windows depend on somebody's
    /// system clock; this one depends on the chain having actually advanced.
    floors: HashMap<String, u64>,
    /// seal_id hex -> (anchor tx signature, slot). Served so a recipient can fetch the anchor
    /// and verify it against its OWN leaf, rather than taking this gateway's word for anything.
    anchors: HashMap<String, (String, u64)>,
}

/// Extract a timelock window from a `{signed_leaf, sender_id_pub}` body, but only if the leaf
/// really is the sender's and really is this seal's.
///
/// `Ok(None)` = the body carried no leaf (legacy/no-window finalise). `Err` = it carried
/// something that failed verification, which we refuse rather than silently ignore.
fn verify_window(body: &str, sid_hex: &str, store: &Arc<Mutex<GwStore>>) -> Result<Option<(i64, i64)>, String> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Err("unparseable finalize body".into());
    };
    // A bare {reveal_at, destroy_at} is exactly the forgeable thing we no longer accept.
    if v.get("signed_leaf").is_none() {
        if v.get("reveal_at").is_some() || v.get("destroy_at").is_some() {
            return Err("a timelock window must arrive inside a signed leaf, not as bare fields".into());
        }
        return Ok(None);
    }
    let leaf: SignedLeaf = serde_json::from_value(v["signed_leaf"].clone())
        .map_err(|e| format!("bad signed_leaf: {e}"))?;
    let sender_pub: [u8; 32] = v
        .get("sender_id_pub")
        .and_then(Value::as_str)
        .and_then(|s| hex::decode(s).ok())
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| "missing sender_id_pub".to_string())?;

    // the leaf must be for the seal named in the path
    if hex::encode(leaf.leaf.seal_id) != sid_hex {
        return Err("signed leaf is for a different seal".into());
    }
    // the pubkey must be the one the leaf commits to, and the signature must verify over it
    if seal_core::identity_commitment(&sender_pub) != leaf.leaf.sender_identity_commitment {
        return Err("sender_id_pub does not match the leaf's identity commitment".into());
    }
    let sig: [u8; 64] = leaf
        .sender_signature
        .as_slice()
        .try_into()
        .map_err(|_| "malformed sender signature".to_string())?;
    if !seal_crypto::verify_sig(&sender_pub, &leaf.leaf.canonical_bytes(), &sig) {
        return Err("invalid sender signature on the leaf".into());
    }
    // and it must be the SAME leaf our shares were issued against, so a valid leaf from
    // elsewhere cannot be pointed at this seal's shares
    {
        let s = store.lock().unwrap();
        if let Some(held) = s.shares.get(sid_hex) {
            let want = leaf.leaf.leaf_hash();
            if held.iter().any(|e| e.leaf_hash != want) {
                return Err("leaf does not match the shares held for this seal".into());
            }
        }
    }
    store
        .lock()
        .unwrap()
        .floors
        .insert(sid_hex.to_string(), leaf.leaf.not_before_finalized_slot);
    Ok(Some((leaf.leaf.reveal_at_unix as i64, leaf.leaf.destroy_at_unix as i64)))
}

/// The leaf hash of an already-verified `{signed_leaf, ...}` body. Only call after
/// `verify_window` returned Ok — this does no checking of its own.
fn verified_leaf_hash(body: &str) -> Option<[u8; 32]> {
    let v: Value = serde_json::from_str(body).ok()?;
    let leaf: SignedLeaf = serde_json::from_value(v.get("signed_leaf")?.clone()).ok()?;
    Some(leaf.leaf.leaf_hash())
}

/// Submit an anchor transfer committing `leaf_hash` as its recipient address, and return
/// `(signature, slot)` once the chain confirms it.
fn anchor_leaf(
    signer: &crate::AnchorSigner,
    rpc: &crate::WcahtRpc,
    leaf_hash: &[u8; 32],
    api_key: Option<&str>,
) -> Result<(String, u64)> {
    let (blockhash, slot) = rpc.recent_blockhash()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let fee: u64 = std::env::var("WCAHT_ANCHOR_FEE").ok().and_then(|v| v.parse().ok()).unwrap_or(25_000);
    let tx = signer.anchor_tx(leaf_hash, &blockhash, slot + 150, now, fee)?;
    let resp = rpc.submit_tx(&tx, api_key)?;
    let sig = tx.get("signature").and_then(Value::as_str).unwrap_or_default().to_string();
    if sig.is_empty() {
        return Err(anyhow!("anchor tx has no signature: {resp}"));
    }
    // The recipient re-verifies against the chain anyway; the slot here is a hint.
    Ok((sig, rpc.finalized_slot().unwrap_or(slot)))
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Run a gateway (blocking) on `addr`, e.g. `"0.0.0.0:9101"`.
pub fn serve_gateway(addr: &str) -> Result<()> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("gateway bind {addr}: {e}"))?;
    let store = Arc::new(Mutex::new(GwStore::default()));
    // WCAHT_RPC=http://host:8901 lets this gateway verify chain time itself.
    let chain_rpc = std::env::var("WCAHT_RPC").ok().map(|u| crate::WcahtRpc::new(&u));
    // Anchoring is opt-in: it costs a real fee, so it needs an explicitly funded account.
    let anchor_signer = std::env::var("WCAHT_ANCHOR_SEED")
        .ok()
        .and_then(|h| hex::decode(h.trim()).ok())
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .map(|seed| crate::AnchorSigner::from_seed(&seed));
    let anchor_api_key = std::env::var("WCAHT_API_KEY").ok();
    match (&anchor_signer, &chain_rpc) {
        (Some(sg), Some(_)) => println!("gateway: anchoring ON from {}", sg.address()),
        (Some(_), None) => println!("gateway: WCAHT_ANCHOR_SEED set but WCAHT_RPC is not — anchoring OFF"),
        (None, _) => println!("gateway: WCAHT_ANCHOR_SEED unset — seals will not be anchored on-chain"),
    }
    match &chain_rpc {
        Some(_) => println!("gateway: chain gate ON (WCAHT_RPC set) — finality and timelock both verified against the chain"),
        None => println!(
            "gateway: WCAHT_RPC unset — 'finalised' is TRUSTED FROM THE CALLER and the timelock              rests on the signed wall-clock window only. Set WCAHT_RPC in production."
        ),
    }

    for mut req in server.incoming_requests() {
        let method = req.method().clone();
        let url = req.url().to_string();

        let (code, body): (u16, String) = match (method, url.as_str()) {
            (tiny_http::Method::Post, "/deposit") => {
                let mut buf = String::new();
                if req.as_reader().read_to_string(&mut buf).is_err() {
                    (400, err_json("unreadable body"))
                } else {
                    match serde_json::from_str::<KeyShareEnvelope>(&buf) {
                        Ok(env) => {
                            let sid = hex::encode(env.seal_id);
                            store.lock().unwrap().shares.entry(sid).or_default().push(env);
                            (200, r#"{"status":"held"}"#.to_string())
                        }
                        Err(e) => (400, err_json(&format!("bad share: {e}"))),
                    }
                }
            }
            (tiny_http::Method::Post, path) if path.starts_with("/finalize/") => {
                let sid = path["/finalize/".len()..].to_string();
                // The timelock window is taken from the SIGNED leaf, never from a bare
                // {reveal_at, destroy_at} body: this endpoint has no authentication, so a
                // caller-supplied window could be set by anyone. Body (optional):
                //   { signed_leaf: SignedLeaf, sender_id_pub: hex }
                // We verify the sender signature, that the leaf is for THIS seal, and that it
                // is the same leaf our held shares were issued against — so the only window
                // anyone can install is the one the sender actually signed.
                let mut buf = String::new();
                let _ = req.as_reader().read_to_string(&mut buf);
                let mut window: Option<(i64, i64)> = None;
                let mut rejected: Option<String> = None;
                let mut leaf_hash_seen: Option<[u8; 32]> = None;
                if !buf.trim().is_empty() {
                    match verify_window(&buf, &sid, &store) {
                        Ok(Some(w)) => {
                            window = Some(w);
                            leaf_hash_seen = verified_leaf_hash(&buf);
                        }
                        Ok(None) => {}                      // no leaf supplied → no window
                        Err(e) => rejected = Some(e),
                    }
                }
                // "Finalised" used to be an unverified assertion: any caller could POST here
                // and the gateway would release. With a node configured we check the chain
                // ourselves — the seal's own slot floor must actually be finalised — and we
                // require the signed leaf that carries it.
                let chain_says_final = match (chain_rpc.as_ref(), store.lock().unwrap().floors.get(&sid).copied()) {
                    (None, _) => true, // no node configured: legacy trust, logged at startup
                    (Some(_), None) => false, // node configured but no signed leaf → not provable
                    (Some(rpc), Some(floor)) => rpc.finalized_slot().map(|fin| fin >= floor).unwrap_or(false),
                };
                if let Some(e) = rejected {
                    (400, err_json(&e))
                } else if !chain_says_final {
                    (425, err_json("chain has not finalised this seal — send the signed leaf and wait"))
                } else {
                    {
                        let mut s = store.lock().unwrap();
                        s.finalised.insert(sid.clone());
                        if let Some((r, d)) = window {
                            if r > 0 || d > 0 {
                                s.windows.insert(sid.clone(), (r, d));
                            }
                        }
                    }
                    // Anchor the verified leaf on-chain, once per seal. The leaf hash becomes
                    // the transfer's recipient address, so the resulting transaction IS the
                    // chain's attestation that this leaf existed — which the recipient can
                    // verify itself with ss_verify_anchor.
                    if let (Some(signer), Some(rpc), Some(lh)) =
                        (anchor_signer.as_ref(), chain_rpc.as_ref(), leaf_hash_seen)
                    {
                        let already = store.lock().unwrap().anchors.contains_key(&sid);
                        if !already {
                            match anchor_leaf(signer, rpc, &lh, anchor_api_key.as_deref()) {
                                Ok((sig, slot)) => {
                                    store.lock().unwrap().anchors.insert(sid.clone(), (sig, slot));
                                }
                                Err(e) => eprintln!("gateway: anchoring {sid} failed: {e}"),
                            }
                        }
                    }
                    (200, r#"{"status":"finalised"}"#.to_string())
                }
            }
            (tiny_http::Method::Get, path) if path.starts_with("/release/") => {
                let sid = &path["/release/".len()..];
                // Chain-time gate: when this gateway can see a WCAHT node, the seal's signed
                // slot floor is checked against REAL finality before anything is released, so
                // the timelock does not rest on a system clock. Without a node configured we
                // fall back to the wall-clock window alone (logged at startup).
                let chain_floor_ok = match (chain_rpc.as_ref(), store.lock().unwrap().floors.get(sid).copied()) {
                    (Some(rpc), Some(floor)) if floor > 0 => match rpc.finalized_slot() {
                        Ok(fin) => fin >= floor,
                        Err(_) => false, // cannot see the chain → refuse rather than release early
                    },
                    _ => true,
                };
                let mut s = store.lock().unwrap();
                if !s.finalised.contains(sid) {
                    (425, err_json("not finalised — no early release")) // 425 Too Early
                } else if !chain_floor_ok {
                    (425, err_json("chain has not finalised past the seal's slot floor")) // 425
                } else if let Some(&(reveal_at, destroy_at)) = s.windows.get(sid) {
                    let now = now_unix();
                    if destroy_at > 0 && now >= destroy_at {
                        s.shares.remove(sid); // self-destruct: the share is gone, key unrecoverable
                        (410, err_json("destroyed — window closed")) // 410 Gone
                    } else if reveal_at > 0 && now < reveal_at {
                        (425, err_json("timelocked — not yet revealable")) // 425 Too Early
                    } else {
                        let shares = s.shares.get(sid).cloned().unwrap_or_default();
                        (200, serde_json::to_string(&shares).unwrap_or_else(|_| "[]".into()))
                    }
                } else {
                    let shares = s.shares.get(sid).cloned().unwrap_or_default();
                    (200, serde_json::to_string(&shares).unwrap_or_else(|_| "[]".into()))
                }
            }
            (tiny_http::Method::Get, path) if path.starts_with("/anchor/") => {
                let sid = &path["/anchor/".len()..];
                match store.lock().unwrap().anchors.get(sid) {
                    Some((sig, slot)) => (200, serde_json::json!({ "signature": sig, "slot": slot }).to_string()),
                    None => (404, err_json("no anchor for this seal")),
                }
            }
            _ => (404, err_json("not found")),
        };

        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
        let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(code).with_header(header));
    }
    Ok(())
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "status": "error", "message": msg }).to_string()
}

/// Client for a gateway.
pub struct GatewayClient {
    base: String,
    http: reqwest::blocking::Client,
}

impl GatewayClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_string(),
            http: reqwest::blocking::Client::builder().timeout(Duration::from_secs(10)).build().expect("http client"),
        }
    }

    /// Sender: hand this gateway its key-share envelope.
    pub fn deposit(&self, env: &KeyShareEnvelope) -> Result<()> {
        let resp = self.http.post(format!("{}/deposit", self.base)).json(env).send()?;
        if resp.status().is_success() { Ok(()) } else { Err(anyhow!("deposit failed: {}", resp.status())) }
    }

    /// Signal the gateway that the seal finalised (production: it watches WCAHT itself).
    pub fn finalize(&self, seal_id: &[u8; 32]) -> Result<()> {
        let resp = self.http.post(format!("{}/finalize/{}", self.base, hex::encode(seal_id))).send()?;
        if resp.status().is_success() { Ok(()) } else { Err(anyhow!("finalize failed: {}", resp.status())) }
    }

    /// Recipient: request the share. Empty until the gateway has seen finality.
    pub fn release(&self, seal_id: &[u8; 32]) -> Result<Vec<KeyShareEnvelope>> {
        let resp = self.http.get(format!("{}/release/{}", self.base, hex::encode(seal_id))).send()?;
        if resp.status().as_u16() == 425 {
            return Ok(Vec::new());
        }
        Ok(resp.error_for_status()?.json()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seal_core::{seal_text_with_mode, SealMode};
    use seal_crypto as sc;

    /// Build a real sealed item so we have a genuinely signed leaf to work with.
    fn signed_item(reveal: u64, destroy: u64) -> (seal_core::SealedItem, [u8; 32]) {
        let id = sc::SignId::generate();
        let dev = sc::SignId::generate();
        let bob = sc::DeviceKey::generate();
        let gw: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
        let item = seal_text_with_mode(
            b"x", &id, &dev, &bob.public(), sc::random_32(), &gw, 2, 100, 100_000,
            SealMode::StrictSeal, reveal, destroy,
        )
        .unwrap();
        (item, id.public())
    }

    fn body(item: &seal_core::SealedItem, sender_pub: &[u8; 32]) -> String {
        serde_json::json!({
            "signed_leaf": serde_json::to_value(&item.signed_leaf).unwrap(),
            "sender_id_pub": hex::encode(sender_pub),
        })
        .to_string()
    }

    #[test]
    fn a_signed_leaf_installs_its_own_window() {
        let (item, pubk) = signed_item(1_900_000_000, 0);
        let sid = hex::encode(item.signed_leaf.leaf.seal_id);
        let store = Arc::new(Mutex::new(GwStore::default()));
        let w = verify_window(&body(&item, &pubk), &sid, &store).expect("verifies");
        assert_eq!(w, Some((1_900_000_000, 0)));
    }

    #[test]
    fn bare_numbers_are_refused_now() {
        // This is the old forgeable shape: anyone could POST it and move a deadline.
        let store = Arc::new(Mutex::new(GwStore::default()));
        let e = verify_window(r#"{"reveal_at":1,"destroy_at":2}"#, "aa", &store).unwrap_err();
        assert!(e.contains("signed leaf"), "{e}");
    }

    #[test]
    fn a_tampered_deadline_is_refused() {
        let (mut item, pubk) = signed_item(1_900_000_000, 0);
        let sid = hex::encode(item.signed_leaf.leaf.seal_id);
        item.signed_leaf.leaf.reveal_at_unix = 1; // shorten it — signature no longer covers this
        let store = Arc::new(Mutex::new(GwStore::default()));
        let e = verify_window(&body(&item, &pubk), &sid, &store).unwrap_err();
        assert!(e.contains("invalid sender signature"), "{e}");
    }

    #[test]
    fn a_valid_leaf_for_another_seal_is_refused() {
        let (item, pubk) = signed_item(1_900_000_000, 0);
        let store = Arc::new(Mutex::new(GwStore::default()));
        // genuine, correctly signed leaf — but aimed at a different seal id
        let e = verify_window(&body(&item, &pubk), &hex::encode([9u8; 32]), &store).unwrap_err();
        assert!(e.contains("different seal"), "{e}");
    }

    #[test]
    fn a_mismatched_sender_key_is_refused() {
        let (item, _) = signed_item(1_900_000_000, 0);
        let sid = hex::encode(item.signed_leaf.leaf.seal_id);
        let store = Arc::new(Mutex::new(GwStore::default()));
        let e = verify_window(&body(&item, &sc::SignId::generate().public()), &sid, &store).unwrap_err();
        assert!(e.contains("identity commitment"), "{e}");
    }
}
