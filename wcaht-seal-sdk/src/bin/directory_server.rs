//! Denvion phone↔address directory server (privacy-preserving).
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-directory -- [0.0.0.0:9988]
//!
//! Stores only hash(phone) → contact card. The raw phone number never reaches it.

use anyhow::Result;

fn main() -> Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:9988".to_string());
    println!("Denvion directory listening on http://{addr}");
    println!("  POST /register            {{ commitment:<hex>, card:\"denvion:…\" }}");
    println!("  GET  /lookup/<commitment> → {{ card }}  (raw phone never sent)");
    wcaht_seal_sdk::directory::serve_directory(&addr)
}
