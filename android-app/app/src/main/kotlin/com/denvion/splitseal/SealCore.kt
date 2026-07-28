package com.denvion.splitseal

/**
 * JNI bridge to the shared Rust `seal-core` (via `seal-ffi`, libseal_ffi.so).
 * The native method names map to `Java_com_denvion_splitseal_SealCore_native*`.
 */
object SealCore {
    init {
        System.loadLibrary("seal_ffi")
    }

    private external fun nativeVersion(): String
    private external fun nativeRunDemo(): String
    private external fun nativeRunFastDemo(): String

    /** `{ "protocol":"DSCP-2", "version":2, "chain_id":7789 }` */
    fun version(): String = nativeVersion()

    /** StrictSeal: seal -> (locked) -> L1 finalise -> (opened) transcript as JSON. */
    fun runDemo(): String = nativeRunDemo()

    /** FastSeal: seal -> (locked) -> gateway pre-confirmation quorum -> (opened before finality). */
    fun runFastDemo(): String = nativeRunFastDemo()
}
