//! Seal batcher service — one anchoring transaction per interval, a capsule per message.
//!
//!   WCAHT_RPC=http://<node>:8901 \
//!   WCAHT_BATCHER_SEED=<32-byte hex of the paying account> \
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-batcher -- 0.0.0.0:9300
//!
//! Without RPC + seed it runs queue-only (accepts leaves, submits nothing), which is the
//! safe default: it will not silently pretend messages were anchored.

fn main() -> anyhow::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:9300".to_string());
    println!("seal batcher listening on {addr}");
    wcaht_seal_sdk::batcher::serve_batcher(&addr)
}
