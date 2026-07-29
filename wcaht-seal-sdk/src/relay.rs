//! HTTP payload-prefetch delivery relay (DSCP-2 fast path).
//!
//! An untrusted store-and-forward for [`EncryptedEnvelope`]s. It carries only
//! ciphertext — never keys or plaintext — so the recipient can PREFETCH the locked
//! ciphertext ahead of the release gate. When the gate opens (pre-confirmation quorum
//! or L1 finality), unlocking is a purely local operation with no network round-trip.
//!
//! Wire contract:
//!   POST /mailbox                     body = EncryptedEnvelope JSON  → 200
//!   GET  /mailbox/<hex mailbox_tag>   → JSON array of EncryptedEnvelope

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use seal_core::EncryptedEnvelope;
use serde_json::Value;

type Store = Arc<Mutex<HashMap<[u8; 32], Vec<EncryptedEnvelope>>>>;

/// Run the relay server (blocking) on `addr`, e.g. `"127.0.0.1:9977"`.
///
/// Durability + replication for the `/inbox` bundle store (the mobile delivery path):
///   * every accepted item is appended to a disk log and reloaded on startup, so a relay
///     restart/crash does not lose queued messages;
///   * a client write is GOSSIPED to peer relays (`WCAHT_RELAY_PEERS`, comma-separated base
///     URLs) with an `X-Gossip: 1` header, so a message that reached ANY one relay propagates
///     to the others — it survives even if the relay it first landed on later dies.
/// Env: `WCAHT_RELAY_PEERS` (peer base URLs), `WCAHT_RELAY_DATA` (data dir, default `relay-data`).
pub fn serve_relay(addr: &str) -> Result<()> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("relay bind {addr}: {e}"))?;
    let store: Store = Arc::new(Mutex::new(HashMap::new()));
    // generic bundle inbox: hex mailbox tag -> [bundle JSON string], for full delivery.
    let inbox: Arc<Mutex<HashMap<String, Vec<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    // per-tag set of seen seal-ids, so replay/gossip/replicated writes never duplicate.
    let seen: Arc<Mutex<HashMap<String, HashSet<String>>>> = Arc::new(Mutex::new(HashMap::new()));

    let peers: Vec<String> = std::env::var("WCAHT_RELAY_PEERS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let data_dir = std::env::var("WCAHT_RELAY_DATA").unwrap_or_else(|_| "relay-data".to_string());
    std::fs::create_dir_all(&data_dir).ok();
    let log_path = format!("{data_dir}/inbox.log");
    // live metrics for the status dashboard: cumulative total + per-minute buckets of new messages.
    let total = Arc::new(Mutex::new(0u64));
    let buckets: Arc<Mutex<BTreeMap<i64, u64>>> = Arc::new(Mutex::new(BTreeMap::new()));
    *total.lock().unwrap() = load_inbox(&log_path, &inbox, &seen) as u64;
    if !peers.is_empty() {
        println!("relay: {} peer(s) for gossip: {}", peers.len(), peers.join(", "));
    }
    println!("relay: persisting to {log_path}");
    let gossip_http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(4)).build().ok();

    for mut req in server.incoming_requests() {
        // an X-Gossip header marks a replicated write from a peer relay → store but do NOT re-gossip.
        let from_gossip = req
            .headers()
            .iter()
            .any(|h| h.field.as_str().as_str().eq_ignore_ascii_case("X-Gossip"));
        let method = req.method().clone();
        let url = req.url().to_string();

        let (code, body): (u16, String) = match (method, url.as_str()) {
            (tiny_http::Method::Post, "/mailbox") => {
                let mut buf = String::new();
                if req.as_reader().read_to_string(&mut buf).is_err() {
                    (400, json_err("unreadable body"))
                } else {
                    match serde_json::from_str::<EncryptedEnvelope>(&buf) {
                        Ok(env) => {
                            store.lock().unwrap().entry(env.recipient_mailbox_tag).or_default().push(env);
                            (200, r#"{"status":"stored"}"#.to_string())
                        }
                        Err(e) => (400, json_err(&format!("bad envelope: {e}"))),
                    }
                }
            }
            (tiny_http::Method::Get, path) if path.starts_with("/mailbox/") => {
                let hex_tag = &path["/mailbox/".len()..];
                match decode_tag(hex_tag) {
                    Ok(tag) => {
                        let items = store.lock().unwrap().get(&tag).cloned().unwrap_or_default();
                        match serde_json::to_string(&items) {
                            Ok(j) => (200, j),
                            Err(e) => (500, json_err(&format!("encode: {e}"))),
                        }
                    }
                    Err(e) => (400, json_err(&e.to_string())),
                }
            }
            // full-delivery bundle inbox (opaque JSON, keyed by hex mailbox tag)
            (tiny_http::Method::Post, path) if path.starts_with("/inbox/") => {
                let tag = path["/inbox/".len()..].to_string();
                let mut buf = String::new();
                if req.as_reader().read_to_string(&mut buf).is_err() {
                    (400, json_err("unreadable body"))
                } else {
                    let sid = seal_id_of(&buf);
                    let newly = {
                        let mut sn = seen.lock().unwrap();
                        if sn.entry(tag.clone()).or_default().insert(sid) {
                            inbox.lock().unwrap().entry(tag.clone()).or_default().push(buf.clone());
                            true
                        } else {
                            false
                        }
                    };
                    if newly {
                        persist(&log_path, &tag, &buf); // survive a restart
                        {
                            *total.lock().unwrap() += 1;
                            let minute = (now_unix() / 60) * 60;
                            *buckets.lock().unwrap().entry(minute).or_default() += 1;
                        }
                        if !from_gossip {
                            if let Some(c) = &gossip_http {
                                gossip(c.clone(), peers.clone(), tag.clone(), buf.clone()); // spread to peers
                            }
                        }
                    }
                    (200, r#"{"status":"delivered"}"#.to_string())
                }
            }
            (tiny_http::Method::Get, path) if path.starts_with("/inbox/") => {
                let tag = &path["/inbox/".len()..];
                let items = inbox.lock().unwrap().get(tag).cloned().unwrap_or_default();
                (200, format!("[{}]", items.join(",")))
            }
            // live metrics for the status dashboard (this node's view).
            (tiny_http::Method::Get, "/stats") => {
                let now_min = (now_unix() / 60) * 60;
                let per_minute: Vec<Value> = {
                    let b = buckets.lock().unwrap();
                    (0..60)
                        .rev()
                        .map(|i| {
                            let t = now_min - i * 60;
                            serde_json::json!({ "t": t, "n": b.get(&t).copied().unwrap_or(0) })
                        })
                        .collect()
                };
                (
                    200,
                    serde_json::json!({
                        "node_total": *total.lock().unwrap(),
                        "mailboxes": inbox.lock().unwrap().len(),
                        "per_minute": per_minute,
                    })
                    .to_string(),
                )
            }
            _ => (404, json_err("not found")),
        };

        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
        let response = tiny_http::Response::from_string(body).with_status_code(code).with_header(header);
        let _ = req.respond(response);
    }
    Ok(())
}

/// A stable dedup key for an inbox item: its `seal_id` if present, else a hash of the bytes.
fn seal_id_of(item: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(item) {
        if let Some(s) = v.get("seal_id").and_then(Value::as_str) {
            return s.to_string();
        }
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    item.hash(&mut h);
    format!("h{:x}", h.finish())
}

/// Append an accepted item to the durable log (one JSON line: `{tag, item}`).
fn persist(log_path: &str, tag: &str, item: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let line = serde_json::json!({ "tag": tag, "item": item }).to_string();
        let _ = writeln!(f, "{line}");
    }
}

/// Reload the durable log into memory on startup (deduped by seal-id). Returns the count loaded.
fn load_inbox(
    log_path: &str,
    inbox: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    seen: &Arc<Mutex<HashMap<String, HashSet<String>>>>,
) -> usize {
    let Ok(content) = std::fs::read_to_string(log_path) else { return 0 };
    let mut ib = inbox.lock().unwrap();
    let mut sn = seen.lock().unwrap();
    let mut n = 0usize;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            let tag = v.get("tag").and_then(Value::as_str).unwrap_or_default().to_string();
            let item = v.get("item").and_then(Value::as_str).unwrap_or_default().to_string();
            if tag.is_empty() || item.is_empty() {
                continue;
            }
            if sn.entry(tag.clone()).or_default().insert(seal_id_of(&item)) {
                ib.entry(tag).or_default().push(item);
                n += 1;
            }
        }
    }
    if n > 0 {
        println!("relay: reloaded {n} persisted message(s)");
    }
    n
}

/// Seconds since the Unix epoch.
fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Fan a client write out to peer relays (marked X-Gossip so they don't re-forward). Fire-and-forget.
fn gossip(client: reqwest::blocking::Client, peers: Vec<String>, tag: String, body: String) {
    if peers.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for p in &peers {
            let _ = client.post(format!("{p}/inbox/{tag}")).header("X-Gossip", "1").body(body.clone()).send();
        }
    });
}

fn json_err(msg: &str) -> String {
    serde_json::json!({ "status": "error", "message": msg }).to_string()
}

fn decode_tag(hex_tag: &str) -> Result<[u8; 32]> {
    hex::decode(hex_tag)?.try_into().map_err(|_| anyhow!("mailbox tag must be 32 bytes hex"))
}

/// Client for the delivery relay. Posts ciphertext and prefetches by mailbox tag.
pub struct DeliveryRelayClient {
    base: String,
    http: reqwest::blocking::Client,
}

impl DeliveryRelayClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_string(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("http client"),
        }
    }

    /// Sender posts a ciphertext envelope for later prefetch.
    pub fn post(&self, envelope: &EncryptedEnvelope) -> Result<()> {
        let resp = self.http.post(format!("{}/mailbox", self.base)).json(envelope).send()?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("relay post failed: {}", resp.status()))
        }
    }

    /// Recipient prefetches all locked ciphertext addressed to its mailbox tag.
    pub fn prefetch(&self, mailbox_tag: &[u8; 32]) -> Result<Vec<EncryptedEnvelope>> {
        let resp = self
            .http
            .get(format!("{}/mailbox/{}", self.base, hex::encode(mailbox_tag)))
            .send()?
            .error_for_status()?;
        Ok(resp.json()?)
    }
}
