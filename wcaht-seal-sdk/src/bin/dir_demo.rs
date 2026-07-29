//! Proves the phone→address directory end-to-end over real HTTP.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-dir-demo
//!
//! Spins up the directory, registers "Alice" under her phone, then resolves a
//! differently-formatted version of that same number back to her WCAHT address —
//! all while the server only ever stores hash(phone).

use std::{thread, time::Duration};

use anyhow::{anyhow, Result};
use seal_core::Identity;
use wcaht_seal_sdk::directory::{serve_directory, DirectoryClient};

fn main() -> Result<()> {
    let addr = "127.0.0.1:9988";
    thread::spawn(move || {
        let _ = serve_directory(addr);
    });
    thread::sleep(Duration::from_millis(250));
    let dir = DirectoryClient::new(&format!("http://{addr}"));

    println!("== phone ↔ address directory (privacy-preserving) ==\n");

    // Alice publishes her number → her card.
    let alice = Identity::generate("Alice");
    let alice_phone = "+855 12 345 678";
    dir.register(alice_phone, &alice.card().encode())?;
    println!("Alice registered   phone {alice_phone}");
    println!("  (server stores only the hash, resolves to address {})", alice.address());

    // Bob types Alice's number (formatted differently) and resolves her address.
    let typed = "855-12345678"; // same digits as "+855 12 345 678", different formatting
    match dir.lookup(typed)? {
        Some(card) => {
            println!("\nlook up \"{typed}\"  →  {} @ {}", card.name, card.address());
            if card.address() != alice.address() {
                return Err(anyhow!("resolved the wrong address!"));
            }
            println!("  ✓ resolves to Alice's real WCAHT address + device key");
        }
        None => return Err(anyhow!("expected to resolve Alice")),
    }

    // An unregistered number resolves to nothing.
    println!("\nlook up \"+855 99 000 000\"  →  {:?}", dir.lookup("+855 99 000 000")?.map(|c| c.name));
    println!("\ndone: typing a phone number resolves to a WCAHT address, no raw number stored.");
    Ok(())
}
