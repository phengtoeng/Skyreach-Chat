//! Seal batcher — one anchoring transaction per interval, a capsule per message.
//!
//! Every message still gets its own signed leaf and its own merkle proof; what it does NOT
//! get is its own transaction. Leaves arriving in a window are committed under a single
//! `SEAL_ROOT`, anchored once, and each sender/recipient can then fetch a proof that THEIR
//! leaf is inside that finalised root.
//!
//! Why this and not a transaction per message: per-message anchoring caps out at the chain's
//! throughput (~2,500 TPS) and, worse, mints a new address per message — state growth no
//! amount of hardware fixes. Batching is flat in both: ~1 transaction per interval regardless
//! of volume.
//!
//! The fee is paid by the batcher's own account (gas sponsorship). Users have no chain
//! account, no balance and no key that could sign a transaction — chat identity keys and
//! wallet keys are deliberately separate (spec §5.2), so a user cannot be charged.
//!
//! Wire contract:
//!   POST /leaf    body = { signed_leaf, sender_id_pub }  → { queued, seal_id }
//!   GET  /proof/<hex seal_id>  → SealProof JSON, or 425 while it is still pending
//!   GET  /stats   → { pending, batches, anchored, last_root }

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use seal_core::{seal_batch_proof, seal_batch_root, SealProof, SignedLeaf};
use serde_json::Value;

/// A leaf waiting to be committed in the next root.
struct Pending {
    leaf: SignedLeaf,
    leaf_hash: [u8; 32],
}

#[derive(Default)]
struct BatcherStore {
    pending: Vec<Pending>,
    /// seal_id hex -> the proof, once its batch has been anchored.
    proofs: HashMap<String, SealProof>,
    batches: u64,
    anchored: u64,
    last_root: Option<[u8; 32]>,
    /// Last observed treasury balance in kak, and when. `None` until a poll succeeds — an
    /// unknown balance is reported as unknown, never as healthy.
    treasury_kak: Option<u128>,
    treasury_checked_unix: u64,
    /// Consecutive failed anchor attempts. Non-zero means messages are queueing but not
    /// reaching the chain — which is how a dry treasury actually presents.
    anchor_failures: u64,
    last_anchor_error: Option<String>,
    /// Leaves refused because the queue was full. Refusing is deliberate — see MAX_PENDING.
    dropped_overflow: u64,
}

/// Warn well before anchoring becomes impossible. Default leaves ~200k anchors of headroom;
/// override with `WCAHT_TREASURY_WARN_KAK`.
const TREASURY_WARN_DEFAULT: u128 = 1_000_000_000;

/// Hard ceiling on queued-but-unanchored leaves. Without it, a treasury that has run dry — or
/// any sustained submit failure — grows `pending` without bound until the process is OOM
/// killed, taking every already-anchored proof in memory with it. Refusing new leaves is
/// strictly better: the sender still delivers the message, it just is not batched, and both
/// apps already treat batching as best-effort (`submitLeafForBatching` is fire-and-forget).
const MAX_PENDING: usize = 200_000;

fn treasury_warn_threshold() -> u128 {
    std::env::var("WCAHT_TREASURY_WARN_KAK").ok().and_then(|v| v.parse().ok()).unwrap_or(TREASURY_WARN_DEFAULT)
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Accept a `{signed_leaf, sender_id_pub}` body, verifying the sender's signature before it
/// is allowed anywhere near a batch. An unverified leaf must never reach a root: the root is
/// what the chain attests to, so anything inside it has to have been genuinely authored.
fn parse_and_verify(body: &str) -> Result<(SignedLeaf, [u8; 32]), String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad body: {e}"))?;
    let leaf: SignedLeaf = serde_json::from_value(
        v.get("signed_leaf").cloned().ok_or_else(|| "missing signed_leaf".to_string())?,
    )
    .map_err(|e| format!("bad signed_leaf: {e}"))?;
    let sender_pub: [u8; 32] = v
        .get("sender_id_pub")
        .and_then(Value::as_str)
        .and_then(|s| hex::decode(s).ok())
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| "missing or malformed sender_id_pub".to_string())?;

    if seal_core::identity_commitment(&sender_pub) != leaf.leaf.sender_identity_commitment {
        return Err("sender_id_pub does not match the leaf's identity commitment".into());
    }
    let sig: [u8; 64] = leaf
        .sender_signature
        .as_slice()
        .try_into()
        .map_err(|_| "malformed sender signature".to_string())?;
    if !seal_crypto::verify_sig(&sender_pub, &leaf.leaf.canonical_bytes(), &sig) {
        return Err("invalid sender signature".into());
    }
    Ok((leaf, sender_pub))
}

/// Commit everything pending under one root and anchor it on-chain.
///
/// Returns the number of leaves committed. Proofs are only published AFTER the anchoring
/// transaction is accepted — a proof for a root that never landed would be a lie.
fn flush_batch(
    store: &Arc<Mutex<BatcherStore>>,
    signer: &crate::AnchorSigner,
    rpc: &crate::WcahtRpc,
    api_key: Option<&str>,
    max_leaves: usize,
) -> Result<usize> {
    let batch: Vec<Pending> = {
        let mut s = store.lock().unwrap();
        if s.pending.is_empty() {
            return Ok(0);
        }
        let take = s.pending.len().min(max_leaves);
        s.pending.drain(..take).collect()
    };

    let leaf_hashes: Vec<[u8; 32]> = batch.iter().map(|p| p.leaf_hash).collect();
    let root = seal_batch_root(&leaf_hashes);

    // Anchor the ROOT — one transaction for the whole batch. The root becomes the recipient
    // address, exactly as a single-leaf anchor does, so the same verification works.
    let (blockhash, slot) = rpc.recent_blockhash()?;
    let fee = std::env::var("WCAHT_ANCHOR_FEE").ok().and_then(|v| v.parse().ok()).unwrap_or(crate::ANCHOR_MIN_FEE);
    let tx = signer.anchor_tx(&root, &blockhash, slot + 150, now_unix(), fee)?;
    // A failed submit must NEVER lose messages — put the whole batch back and let the next
    // tick retry. (This restore used to sit after a `?`, so any submit error silently dropped
    // every leaf in the batch.)
    let submitted = rpc.submit_tx(&tx, api_key);
    let sig = tx.get("signature").and_then(Value::as_str).unwrap_or_default().to_string();
    if submitted.is_err() || sig.is_empty() {
        let why = match submitted {
            Err(e) => format!("{e}"),
            Ok(_) => "submitted but the signed tx carried no signature".to_string(),
        };
        let mut s = store.lock().unwrap();
        for p in batch {
            s.pending.push(p);
        }
        s.anchor_failures += 1;
        s.last_anchor_error = Some(why.clone());
        return Err(anyhow!("anchor submit failed: {why}"));
    }
    let anchored_slot = rpc.finalized_slot().unwrap_or(slot);

    let n = batch.len();
    let mut s = store.lock().unwrap();
    for (i, p) in batch.iter().enumerate() {
        let Some(path) = seal_batch_proof(&leaf_hashes, i) else { continue };
        s.proofs.insert(
            hex::encode(p.leaf.leaf.seal_id),
            SealProof {
                seal_id: p.leaf.leaf.seal_id,
                leaf_hash: p.leaf_hash,
                merkle_path: path,
                leaf_index: i as u32,
                leaf_count: n as u32,
                seal_root: root,
                finalized_slot: anchored_slot,
            },
        );
    }
    s.batches += 1;
    s.anchored += n as u64;
    s.last_root = Some(root);
    s.anchor_failures = 0;
    s.last_anchor_error = None;
    println!("batcher: anchored {n} leaves under root {} (tx {sig})", hex::encode(&root[..8]));
    Ok(n)
}

/// Run the batcher (blocking) on `addr`.
///
/// Env: `WCAHT_RPC` (node base URL), `WCAHT_BATCHER_SEED` (32-byte hex, the paying account),
/// `WCAHT_API_KEY`, `WCAHT_BATCH_INTERVAL_MS` (default 1000), `WCAHT_BATCH_MAX` (default 10000).
pub fn serve_batcher(addr: &str) -> Result<()> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("batcher bind {addr}: {e}"))?;
    let store = Arc::new(Mutex::new(BatcherStore::default()));

    let rpc = std::env::var("WCAHT_RPC").ok().map(|u| crate::WcahtRpc::new(&u));
    let signer = std::env::var("WCAHT_BATCHER_SEED")
        .ok()
        .and_then(|h| hex::decode(h.trim()).ok())
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .map(|seed| crate::AnchorSigner::from_seed(&seed));
    let api_key = std::env::var("WCAHT_API_KEY").ok();
    let interval_ms: u64 =
        std::env::var("WCAHT_BATCH_INTERVAL_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);
    let max_leaves: usize =
        std::env::var("WCAHT_BATCH_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(10_000);

    match (&rpc, &signer) {
        (Some(_), Some(sg)) => println!("batcher: anchoring from {} every {interval_ms}ms (max {max_leaves}/batch)", sg.address()),
        _ => println!(
            "batcher: WCAHT_RPC and WCAHT_BATCHER_SEED are both required to anchor — \
             running in QUEUE-ONLY mode, no roots will be submitted"
        ),
    }

    // the anchoring loop, independent of request handling
    if let (Some(rpc), Some(signer)) = (rpc, signer) {
        // Treasury watch. Anchoring is sponsored, so the treasury draining is the one failure
        // that stops every message being committed while everything still *looks* healthy —
        // the batcher keeps accepting leaves and keeps answering 425. Poll it on its own
        // thread (never in the anchor path, where a slow node would stall batching) and make
        // the state visible on /stats and /health for something outside to alert on.
        {
            let store_t = store.clone();
            let rpc_t = crate::WcahtRpc::new(rpc.base_url());
            let addr = signer.address().to_string();
            std::thread::spawn(move || loop {
                match rpc_t.balance(&addr) {
                    Ok(bal) => {
                        let warn = treasury_warn_threshold();
                        let anchors_left = bal / crate::ANCHOR_TOTAL_COST_KAK.max(1);
                        {
                            let mut s = store_t.lock().unwrap();
                            s.treasury_kak = Some(bal);
                            s.treasury_checked_unix = now_unix();
                        }
                        if bal < warn {
                            eprintln!(
                                "batcher: TREASURY LOW — {addr} holds {bal} kak, about {anchors_left} \
                                 anchors left (warn below {warn}). Top it up: when it empties, \
                                 messages stop being committed to any root."
                            );
                        }
                    }
                    // A failed poll is not itself a problem — the node may be restarting.
                    // Leave the last known value and say so rather than inventing one.
                    Err(e) => eprintln!("batcher: treasury balance check failed: {e}"),
                }
                std::thread::sleep(Duration::from_secs(60));
            });
        }

        let store_bg = store.clone();
        let key = api_key.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(interval_ms));
            let empty = store_bg.lock().unwrap().pending.is_empty();
            if empty {
                continue;
            }
            if let Err(e) = flush_batch(&store_bg, &signer, &rpc, key.as_deref(), max_leaves) {
                eprintln!("batcher: flush failed: {e}");
            }
        });
    }

    for mut req in server.incoming_requests() {
        let method = req.method().clone();
        let url = req.url().to_string();
        let (code, body): (u16, String) = match (method, url.as_str()) {
            (tiny_http::Method::Post, "/leaf") => {
                let mut buf = String::new();
                if req.as_reader().read_to_string(&mut buf).is_err() {
                    (400, err_json("unreadable body"))
                } else {
                    match parse_and_verify(&buf) {
                        Ok((leaf, _)) => {
                            let seal_id = hex::encode(leaf.leaf.seal_id);
                            let leaf_hash = leaf.leaf.leaf_hash();
                            let mut s = store.lock().unwrap();
                            let known = s.proofs.contains_key(&seal_id)
                                || s.pending.iter().any(|p| p.leaf.leaf.seal_id == leaf.leaf.seal_id);
                            if known {
                                (200, serde_json::json!({ "queued": true, "seal_id": seal_id }).to_string())
                            } else if s.pending.len() >= MAX_PENDING {
                                // Backlog ceiling — anchoring is not keeping up (a dry treasury
                                // looks exactly like this). Say so instead of queueing into an
                                // OOM; the message itself still gets delivered unbatched.
                                s.dropped_overflow += 1;
                                (503, err_json("batcher backlog full — leaf not queued"))
                            } else {
                                s.pending.push(Pending { leaf, leaf_hash });
                                (200, serde_json::json!({ "queued": true, "seal_id": seal_id }).to_string())
                            }
                        }
                        Err(e) => (400, err_json(&e)),
                    }
                }
            }
            (tiny_http::Method::Get, path) if path.starts_with("/proof/") => {
                let sid = &path["/proof/".len()..];
                match store.lock().unwrap().proofs.get(sid) {
                    Some(p) => (200, serde_json::to_string(p).unwrap_or_else(|_| "{}".into())),
                    // 425 Too Early: it is queued but its root has not been anchored yet
                    None => (425, err_json("not anchored yet")),
                }
            }
            (tiny_http::Method::Get, "/stats") => {
                let s = store.lock().unwrap();
                (
                    200,
                    serde_json::json!({
                        "pending": s.pending.len(),
                        "batches": s.batches,
                        "anchored": s.anchored,
                        "last_root": s.last_root.map(hex::encode),
                        "treasury_kak": s.treasury_kak.map(|b| b.to_string()),
                        "treasury_anchors_left": s.treasury_kak.map(|b| (b / crate::ANCHOR_TOTAL_COST_KAK.max(1)) as u64),
                        "treasury_checked_unix": s.treasury_checked_unix,
                        "anchor_failures": s.anchor_failures,
                        "last_anchor_error": s.last_anchor_error,
                        "dropped_overflow": s.dropped_overflow,
                        "anchor_cost_kak": crate::ANCHOR_TOTAL_COST_KAK as u64,
                    })
                    .to_string(),
                )
            }
            // Alerting endpoint: 200 healthy, 503 degraded. Something outside should page on
            // this — a batcher that cannot anchor keeps accepting leaves and answering 425,
            // so it fails silently unless the failure is exposed somewhere with a status code.
            (tiny_http::Method::Get, "/health") => {
                let s = store.lock().unwrap();
                let warn = treasury_warn_threshold();
                let mut problems: Vec<String> = Vec::new();
                if let Some(bal) = s.treasury_kak {
                    if bal < warn {
                        problems.push(format!(
                            "treasury low: {bal} kak (~{} anchors left)",
                            bal / crate::ANCHOR_TOTAL_COST_KAK.max(1)
                        ));
                    }
                }
                if s.anchor_failures > 0 {
                    problems.push(format!(
                        "{} consecutive anchor failures: {}",
                        s.anchor_failures,
                        s.last_anchor_error.as_deref().unwrap_or("unknown")
                    ));
                }
                if s.dropped_overflow > 0 {
                    problems.push(format!("{} leaves refused, backlog full", s.dropped_overflow));
                }
                let ok = problems.is_empty();
                (
                    if ok { 200 } else { 503 },
                    serde_json::json!({
                        "ok": ok,
                        "problems": problems,
                        "pending": s.pending.len(),
                        "treasury_kak": s.treasury_kak.map(|b| b.to_string()),
                    })
                    .to_string(),
                )
            }
            _ => (404, err_json("not found")),
        };
        let hdr = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
            .expect("static header");
        let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(code).with_header(hdr));
    }
    Ok(())
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "error": msg }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use seal_core::{seal_text_with_mode, verify_seal_inclusion, SealMode};
    use seal_crypto as sc;

    fn a_leaf() -> (SignedLeaf, [u8; 32]) {
        let id = sc::SignId::generate();
        let dev = sc::SignId::generate();
        let bob = sc::DeviceKey::generate();
        let gw: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
        let item = seal_text_with_mode(
            b"hi", &id, &dev, &bob.public(), sc::random_32(), &gw, 2, 100, 100_000,
            SealMode::StrictSeal, 0, 0,
        )
        .unwrap();
        (item.signed_leaf, id.public())
    }

    fn body_for(leaf: &SignedLeaf, pubk: &[u8; 32]) -> String {
        serde_json::json!({
            "signed_leaf": serde_json::to_value(leaf).unwrap(),
            "sender_id_pub": hex::encode(pubk),
        })
        .to_string()
    }

    #[test]
    fn only_genuinely_signed_leaves_can_enter_a_batch() {
        let (leaf, pubk) = a_leaf();
        assert!(parse_and_verify(&body_for(&leaf, &pubk)).is_ok());

        // someone else's key does not vouch for this leaf
        let stranger = sc::SignId::generate().public();
        let e = parse_and_verify(&body_for(&leaf, &stranger)).unwrap_err();
        assert!(e.contains("identity commitment"), "{e}");

        // and an edited leaf no longer verifies under its own key
        let mut tampered = leaf.clone();
        tampered.leaf.expires_at_slot += 1;
        let e = parse_and_verify(&body_for(&tampered, &pubk)).unwrap_err();
        assert!(e.contains("invalid sender signature"), "{e}");
    }

    #[test]
    fn a_batch_gives_every_leaf_a_proof_that_verifies() {
        // what the flush loop builds, checked the way a recipient would check it
        let leaves: Vec<(SignedLeaf, [u8; 32])> = (0..9).map(|_| a_leaf()).collect();
        let hashes: Vec<[u8; 32]> = leaves.iter().map(|(l, _)| l.leaf.leaf_hash()).collect();
        let root = seal_batch_root(&hashes);

        for (i, h) in hashes.iter().enumerate() {
            let path = seal_batch_proof(&hashes, i).expect("path");
            assert!(
                verify_seal_inclusion(h, &path, i as u32, hashes.len() as u32, &root),
                "leaf {i} of {} failed to verify",
                hashes.len()
            );
        }
        // 9 messages, one root — that is the whole point
        assert_eq!(hashes.len(), 9);
    }
}
