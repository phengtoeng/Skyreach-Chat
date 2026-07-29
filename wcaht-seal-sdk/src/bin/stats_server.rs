//! Skyreach live status dashboard — aggregates the seal network + WCAHT chain metrics.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-stats -- [0.0.0.0:9300]
//!
//! Env: WCAHT_STATS_NODES (comma IPs), WCAHT_STATS_CHAIN (validator /health URL).

use anyhow::Result;

fn main() -> Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:9300".to_string());
    wcaht_seal_sdk::stats::serve_stats(&addr)
}
