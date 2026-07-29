//! Phone ↔ address directory service (privacy-preserving).
//!
//! Maps a phone COMMITMENT (`hash(phone)`, computed client-side) to the owner's
//! contact card. The server never sees the raw phone number and nothing here ever
//! touches the blockchain — this is the off-chain discovery layer.
//!
//! Wire contract:
//!   POST /register   body = { "commitment": <hex>, "card": "denvion:…" }  → 200
//!   GET  /lookup/<hex commitment>   → { "card": "denvion:…" }  or  404

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use seal_core::{phone_commitment, ContactCard};
use serde_json::Value;

type Store = Arc<Mutex<HashMap<String, String>>>; // commitment_hex -> card code

/// Run the directory server (blocking) on `addr`, e.g. `"0.0.0.0:9988"`.
///
/// Durability + replication (same model as the relay): each registration is appended to a disk
/// log and reloaded on startup, and a client register is GOSSIPED to peer directories
/// (`WCAHT_DIRECTORY_PEERS`, `X-Gossip: 1` to avoid loops) so all nodes converge and a lookup
/// resolves on any node. Env: `WCAHT_DIRECTORY_PEERS`, `WCAHT_DIRECTORY_DATA` (default `directory-data`).
pub fn serve_directory(addr: &str) -> Result<()> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("directory bind {addr}: {e}"))?;
    let store: Store = Arc::new(Mutex::new(HashMap::new()));

    let peers: Vec<String> = std::env::var("WCAHT_DIRECTORY_PEERS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let data_dir = std::env::var("WCAHT_DIRECTORY_DATA").unwrap_or_else(|_| "directory-data".to_string());
    std::fs::create_dir_all(&data_dir).ok();
    let log_path = format!("{data_dir}/register.log");
    load_registrations(&log_path, &store);
    let gossip_http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(4)).build().ok();

    for mut req in server.incoming_requests() {
        let from_gossip = req
            .headers()
            .iter()
            .any(|h| h.field.as_str().as_str().eq_ignore_ascii_case("X-Gossip"));
        let method = req.method().clone();
        let url = req.url().to_string();

        let (code, body): (u16, String) = match (method, url.as_str()) {
            (tiny_http::Method::Post, "/register") => {
                let mut buf = String::new();
                if req.as_reader().read_to_string(&mut buf).is_err() {
                    (400, err_json("unreadable body"))
                } else {
                    match serde_json::from_str::<Value>(&buf) {
                        Ok(v) => {
                            let commitment = v.get("commitment").and_then(Value::as_str).unwrap_or_default();
                            let card = v.get("card").and_then(Value::as_str).unwrap_or_default();
                            // only accept a well-formed card, and only its commitment key
                            if commitment.len() == 64 && ContactCard::decode(card).is_ok() {
                                let changed = {
                                    let mut s = store.lock().unwrap();
                                    s.insert(commitment.to_string(), card.to_string()) != Some(card.to_string())
                                };
                                if changed {
                                    persist(&log_path, commitment, card);
                                    if !from_gossip {
                                        if let Some(c) = &gossip_http {
                                            gossip(c.clone(), peers.clone(), commitment.to_string(), card.to_string());
                                        }
                                    }
                                }
                                (200, r#"{"status":"registered"}"#.to_string())
                            } else {
                                (400, err_json("bad commitment or card"))
                            }
                        }
                        Err(e) => (400, err_json(&format!("bad json: {e}"))),
                    }
                }
            }
            (tiny_http::Method::Get, path) if path.starts_with("/lookup/") => {
                let commitment = &path["/lookup/".len()..];
                match store.lock().unwrap().get(commitment) {
                    Some(card) => (200, serde_json::json!({ "card": card }).to_string()),
                    None => (404, err_json("not found")),
                }
            }
            _ => (404, err_json("not found")),
        };

        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
        let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(code).with_header(header));
    }
    Ok(())
}

/// Append a registration to the durable log (one JSON line: `{commitment, card}`).
fn persist(log_path: &str, commitment: &str, card: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let line = serde_json::json!({ "commitment": commitment, "card": card }).to_string();
        let _ = writeln!(f, "{line}");
    }
}

/// Reload registrations from the durable log on startup (last write wins).
fn load_registrations(log_path: &str, store: &Store) {
    let Ok(content) = std::fs::read_to_string(log_path) else { return };
    let mut s = store.lock().unwrap();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            let commitment = v.get("commitment").and_then(Value::as_str).unwrap_or_default();
            let card = v.get("card").and_then(Value::as_str).unwrap_or_default();
            if commitment.len() == 64 && !card.is_empty() {
                s.insert(commitment.to_string(), card.to_string());
            }
        }
    }
    if !s.is_empty() {
        println!("directory: reloaded {} registration(s)", s.len());
    }
}

/// Fan a client register out to peer directories (X-Gossip so they don't re-forward). Fire-and-forget.
fn gossip(client: reqwest::blocking::Client, peers: Vec<String>, commitment: String, card: String) {
    if peers.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let payload = serde_json::json!({ "commitment": commitment, "card": card });
        for p in &peers {
            let _ = client.post(format!("{p}/register")).header("X-Gossip", "1").json(&payload).send();
        }
    });
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "status": "error", "message": msg }).to_string()
}

/// Client for the directory. Computes the phone commitment locally, so the raw number
/// never leaves the device.
pub struct DirectoryClient {
    base: String,
    http: reqwest::blocking::Client,
}

impl DirectoryClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_string(),
            http: reqwest::blocking::Client::builder().timeout(Duration::from_secs(10)).build().expect("http client"),
        }
    }

    /// Publish `phone → card`. Sends only `hash(phone)` + the card, never the number.
    pub fn register(&self, phone: &str, card_code: &str) -> Result<()> {
        let commitment = hex::encode(phone_commitment(phone));
        let resp = self
            .http
            .post(format!("{}/register", self.base))
            .json(&serde_json::json!({ "commitment": commitment, "card": card_code }))
            .send()?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("register failed: {}", resp.status()))
        }
    }

    /// Resolve a phone number to its owner's card (address + device key), or `None`.
    pub fn lookup(&self, phone: &str) -> Result<Option<ContactCard>> {
        let commitment = hex::encode(phone_commitment(phone));
        let resp = self.http.get(format!("{}/lookup/{commitment}", self.base)).send()?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let v: Value = resp.error_for_status()?.json()?;
        let card = v.get("card").and_then(Value::as_str).ok_or_else(|| anyhow!("no card in response"))?;
        Ok(Some(ContactCard::decode(card)?))
    }
}
