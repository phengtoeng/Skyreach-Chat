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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use seal_core::EncryptedEnvelope;

type Store = Arc<Mutex<HashMap<[u8; 32], Vec<EncryptedEnvelope>>>>;

/// Run the relay server (blocking) on `addr`, e.g. `"127.0.0.1:9977"`.
pub fn serve_relay(addr: &str) -> Result<()> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("relay bind {addr}: {e}"))?;
    let store: Store = Arc::new(Mutex::new(HashMap::new()));

    for mut req in server.incoming_requests() {
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
            _ => (404, json_err("not found")),
        };

        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
        let response = tiny_http::Response::from_string(body).with_status_code(code).with_header(header);
        let _ = req.respond(response);
    }
    Ok(())
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
