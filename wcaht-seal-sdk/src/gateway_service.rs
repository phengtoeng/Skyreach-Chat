//! Seal gateway service (DSCP-2). Holds encrypted key-share envelopes and releases
//! them ONLY after the seal is finalised — the strict-release rule, as a real HTTP
//! service (one of the 3+ independent gateways a t-of-n seal is split across).
//!
//! Wire contract:
//!   POST /deposit                     body = KeyShareEnvelope JSON     → 200
//!   POST /finalize/<hex seal_id>      (the gateway observed L1 finality) → 200
//!   GET  /release/<hex seal_id>       → [KeyShareEnvelope]  (425 if not finalised yet)

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use seal_core::KeyShareEnvelope;

#[derive(Default)]
struct GwStore {
    shares: HashMap<String, Vec<KeyShareEnvelope>>, // seal_id hex -> held shares
    finalised: HashSet<String>,
}

/// Run a gateway (blocking) on `addr`, e.g. `"0.0.0.0:9101"`.
pub fn serve_gateway(addr: &str) -> Result<()> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("gateway bind {addr}: {e}"))?;
    let store = Arc::new(Mutex::new(GwStore::default()));

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
                store.lock().unwrap().finalised.insert(sid);
                (200, r#"{"status":"finalised"}"#.to_string())
            }
            (tiny_http::Method::Get, path) if path.starts_with("/release/") => {
                let sid = &path["/release/".len()..];
                let s = store.lock().unwrap();
                if s.finalised.contains(sid) {
                    let shares = s.shares.get(sid).cloned().unwrap_or_default();
                    (200, serde_json::to_string(&shares).unwrap_or_else(|_| "[]".into()))
                } else {
                    (425, err_json("not finalised — no early release")) // 425 Too Early
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
