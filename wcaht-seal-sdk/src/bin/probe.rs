//! Proves the SDK reads REAL WCAHT state from a live node.
//!
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-probe            # default N1 :8901
//!   cargo run -p wcaht-seal-sdk --bin wcaht-seal-probe http://HOST:PORT
//!
//! Reads are unauthenticated GETs — no keys, no state change on the chain.

use anyhow::Result;
use wcaht_seal_sdk::WcahtRpc;

fn main() -> Result<()> {
    let base = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8901".to_string());
    println!("Probing live WCAHT node: {base}\n");

    let rpc = WcahtRpc::new(&base);

    let fin = rpc.finalized_slot()?;
    println!("  REAL finalized_slot = {fin}   <- this is the SplitSeal FINALISED gate");

    match rpc.wall_clock_slot() {
        Ok(w) => println!("  wall_clock_slot     = {w}   (lead over finality: {})", w.saturating_sub(fin)),
        Err(e) => println!("  wall_clock_slot     = (unavailable: {e})"),
    }

    match rpc.recent_blockhash() {
        Ok((bh, lvs)) => println!("  recent_blockhash    = {bh}  (last_valid_slot {lvs})"),
        Err(e) => println!("  recent_blockhash    = (unavailable: {e})"),
    }

    println!("\nA WcahtSealChain built on this node opens a seal only once finalized_slot");
    println!("advances past the slot its anchor was recorded at — real L1 finality, no mock.");
    Ok(())
}
