//! Denvion seal gateway server (DSCP-2). Holds key shares, releases after finality.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-gateway -- [0.0.0.0:9101]

use anyhow::Result;

fn main() -> Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:9101".to_string());
    println!("Denvion seal gateway on http://{addr}");
    println!("  POST /deposit  POST /finalize/<seal_id>  GET /release/<seal_id>");
    wcaht_seal_sdk::gateway_service::serve_gateway(&addr)
}
