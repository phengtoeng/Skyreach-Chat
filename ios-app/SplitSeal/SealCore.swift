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

    /// Copy a Rust-owned C string into a Swift String and free the original.
    private static func copy(_ ptr: UnsafeMutablePointer<CChar>?) -> String {
        guard let ptr = ptr else { return "{}" }
        defer { ss_free(ptr) }
        return String(cString: ptr)
    }
}
