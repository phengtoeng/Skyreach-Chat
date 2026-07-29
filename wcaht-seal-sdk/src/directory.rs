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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use seal_core::{phone_commitment, ContactCard};
use serde_json::Value;

type Store = Arc<Mutex<HashMap<String, String>>>; // commitment_hex -> card code

/// Run the directory server (blocking) on `addr`, e.g. `"0.0.0.0:9988"`.
pub fn serve_directory(addr: &str) -> Result<()> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("directory bind {addr}: {e}"))?;
    let store: Store = Arc::new(Mutex::new(HashMap::new()));

    for mut req in server.incoming_requests() {
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
                                store.lock().unwrap().insert(commitment.to_string(), card.to_string());
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
