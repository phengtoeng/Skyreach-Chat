//! Denvion SplitSeal delivery relay — an untrusted ciphertext mailbox (DSCP-2).
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-relay -- [127.0.0.1:9977]
//!
//! Carries only EncryptedEnvelopes (ciphertext); never keys or plaintext.

use anyhow::Result;

fn main() -> Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:9977".to_string());
    println!("SplitSeal delivery relay listening on http://{addr}");
    println!("  POST /mailbox                    store a ciphertext envelope");
    println!("  GET  /mailbox/<hex mailbox_tag>  prefetch locked ciphertext");
    wcaht_seal_sdk::relay::serve_relay(&addr)
}
