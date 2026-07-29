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
    private external fun nativeNewIdentity(name: String): String
    private external fun nativeCardFor(identitySeed: String, deviceSeed: String, name: String): String
    private external fun nativeParseCard(code: String): String
    private external fun nativePhoneCommitment(phone: String): String
    private external fun nativeMailboxTag(devicePub: String): String
    private external fun nativeSealTo(devicePub: String, text: String, fast: Boolean): String
    private external fun nativeSealShippable(devicePub: String, text: String, fast: Boolean): String
    private external fun nativeOpenReceived(deviceSeed: String, bundle: String, shares: String): String

    /** `{ "protocol":"DSCP-2", "version":2, "chain_id":7789 }` */
    fun version(): String = nativeVersion()

    /** StrictSeal: seal -> (locked) -> L1 finalise -> (opened) transcript as JSON. */
    fun runDemo(): String = nativeRunDemo()

    /** FastSeal: seal -> (locked) -> gateway pre-confirmation quorum -> (opened before finality). */
    fun runFastDemo(): String = nativeRunFastDemo()

    /** New account. Returns JSON {name,address,identity_seed,device_seed,identity_pub,device_pub,card}. Persist the seeds! */
    fun newIdentity(name: String): String = nativeNewIdentity(name)

    /** Rebuild {name,address,identity_pub,device_pub,card} from stored seeds. */
    fun cardFor(identitySeed: String, deviceSeed: String, name: String): String = nativeCardFor(identitySeed, deviceSeed, name)

    /** Validate a scanned/pasted contact code. Returns {ok,name,address,identity_pub,device_pub} or {ok:false,error}. */
    fun parseCard(code: String): String = nativeParseCard(code)

    /** Directory key for a phone number: {normalized, phone_commitment}. hash(phone) → address (off-chain). */
    fun phoneCommitment(phone: String): String = nativePhoneCommitment(phone)

    /** My inbox tag {ok, mailbox_tag} from my device pubkey — poll the relay here for inbound seals. */
    fun mailboxTag(devicePub: String): String = nativeMailboxTag(devicePub)

    /** Seal text to a contact's device pubkey (hex). Returns {ok, seal_id, recipient_device_commitment, ciphertext_len}. */
    fun sealTo(devicePub: String, text: String, fast: Boolean): String = nativeSealTo(devicePub, text, fast)

    /** Seal + return {ok, seal_id, mailbox_tag, bundle, shares} to ship over the relay + gateways. */
    fun sealShippable(devicePub: String, text: String, fast: Boolean): String = nativeSealShippable(devicePub, text, fast)

    /** Open a collected message with the recipient's device seed. Returns {ok, plaintext} or {ok:false, reason}. */
    fun openReceived(deviceSeed: String, bundle: String, shares: String): String = nativeOpenReceived(deviceSeed, bundle, shares)
}
