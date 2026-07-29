//! Register a test identity in the directory under a phone number (for demos/testing).
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-dir-register -- [url] [phone] [name]

use anyhow::Result;
use seal_core::Identity;
use wcaht_seal_sdk::directory::DirectoryClient;

fn main() -> Result<()> {
    let url = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:9988".to_string());
    let phone = std::env::args().nth(2).unwrap_or_else(|| "85512345678".to_string());
    let name = std::env::args().nth(3).unwrap_or_else(|| "Directory Test".to_string());

    let id = Identity::generate(&name);
    DirectoryClient::new(&url).register(&phone, &id.card().encode())?;
    println!("registered \"{name}\"  phone {phone}  ->  address {}", id.address());
    Ok(())
}
