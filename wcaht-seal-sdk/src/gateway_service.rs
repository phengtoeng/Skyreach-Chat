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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use seal_core::KeyShareEnvelope;
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
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
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
                // optional JSON body {reveal_at, destroy_at} sets the timelock window for this seal.
                let mut buf = String::new();
                let _ = req.as_reader().read_to_string(&mut buf);
                let (reveal_at, destroy_at) = serde_json::from_str::<Value>(&buf)
                    .ok()
                    .map(|v| {
                        (
                            v.get("reveal_at").and_then(Value::as_i64).unwrap_or(0),
                            v.get("destroy_at").and_then(Value::as_i64).unwrap_or(0),
                        )
                    })
                    .unwrap_or((0, 0));
                let mut s = store.lock().unwrap();
                s.finalised.insert(sid.clone());
                if reveal_at > 0 || destroy_at > 0 {
                    s.windows.insert(sid, (reveal_at, destroy_at));
                }
                (200, r#"{"status":"finalised"}"#.to_string())
            }
            (tiny_http::Method::Get, path) if path.starts_with("/release/") => {
                let sid = &path["/release/".len()..];
                let mut s = store.lock().unwrap();
                if !s.finalised.contains(sid) {
                    (425, err_json("not finalised — no early release")) // 425 Too Early
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
