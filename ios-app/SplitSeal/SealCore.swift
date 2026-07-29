import Foundation

/// Swift wrapper over the Rust `seal-core` C ABI (from `seal-ffi`, libseal_ffi.a).
/// The C functions (`ss_version`, `ss_run_demo`, `ss_run_fast_demo`, `ss_free`) are
/// exposed to Swift via the bridging header (`#import "seal_ffi.h"`).
enum SealCore {
    /// `{ "protocol":"DSCP-2", "version":2, "chain_id":7789 }`
    static func version() -> String { copy(ss_version()) }

    /// StrictSeal: seal -> (locked) -> L1 finalise -> (opened) transcript as JSON.
    static func runDemo() -> String { copy(ss_run_demo()) }

    /// FastSeal: seal -> (locked) -> gateway pre-confirmation quorum -> (opened before finality).
    static func runFastDemo() -> String { copy(ss_run_fast_demo()) }

    /// New account. Returns JSON {name,address,identity_seed,device_seed,identity_pub,device_pub,card}. Persist the seeds!
    static func newIdentity(_ name: String) -> String {
        name.withCString { copy(ss_new_identity($0)) }
    }

    /// Rebuild {name,address,identity_pub,device_pub,card} from stored seeds.
    static func cardFor(_ identitySeed: String, _ deviceSeed: String, _ name: String) -> String {
        identitySeed.withCString { i in deviceSeed.withCString { d in name.withCString { n in copy(ss_card_for(i, d, n)) } } }
    }

    /// Validate a scanned/pasted contact code. Returns {ok,name,address,identity_pub,device_pub} or {ok:false,error}.
    static func parseCard(_ code: String) -> String {
        code.withCString { copy(ss_parse_card($0)) }
    }

    /// Copy a Rust-owned C string into a Swift String and free the original.
    private static func copy(_ ptr: UnsafeMutablePointer<CChar>?) -> String {
        guard let ptr = ptr else { return "{}" }
        defer { ss_free(ptr) }
        return String(cString: ptr)
    }
}
