//! Skyreach live status dashboard — aggregates the seal network + WCAHT chain metrics.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-stats -- [0.0.0.0:9301]
//!
//! Env: WCAHT_STATS_NODES (comma IPs), WCAHT_STATS_CHAIN (validator /health URL).
//!
//! Port 9301, not 9300: the batcher owns 9300 on every node because both apps derive their
//! batcher URL as `http://<node>:9300` with no way to configure it. These two collided on N5.

use anyhow::Result;

fn main() -> Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:9301".to_string());
    wcaht_seal_sdk::stats::serve_stats(&addr)
}
