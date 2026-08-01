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
    private external fun nativeSealShippable(identitySeed: String, senderCard: String, devicePub: String, text: String, fast: Boolean, revealAt: Long, destroyAt: Long): String
    private external fun nativeOpenReceived(deviceSeed: String, bundle: String, shares: String): String
    private external fun nativeSealMediaFile(
        identitySeed: String, senderCard: String, devicePub: String, inPath: String,
        mime: String, kind: String, previewPath: String, outDir: String,
        fast: Boolean, revealAt: Long, destroyAt: Long,
    ): String
    private external fun nativeOpenMediaInfo(deviceSeed: String, bundle: String, shares: String, previewOut: String): String
    private external fun nativeOpenMediaFile(deviceSeed: String, bundle: String, shares: String, chunkDir: String, outPath: String): String

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

    /** Seal (signed by MY identitySeed, with MY card embedded so the peer can reply) + return
     *  {ok, seal_id, mailbox_tag, bundle, shares, reveal_at, destroy_at}. revealAt/destroyAt are
     *  unix secs (0 = none) — a timelock window enforced by the gateways. */
    fun sealShippable(identitySeed: String, senderCard: String, devicePub: String, text: String, fast: Boolean, revealAt: Long = 0, destroyAt: Long = 0): String =
        nativeSealShippable(identitySeed, senderCard, devicePub, text, fast, revealAt, destroyAt)

    /** Open a collected message with the recipient's device seed. Returns {ok, plaintext} or {ok:false, reason}. */
    fun openReceived(deviceSeed: String, bundle: String, shares: String): String = nativeOpenReceived(deviceSeed, bundle, shares)

    // ── media ──
    // Media crosses this boundary as FILE PATHS, never as bytes: pushing a 40 MB video
    // through JNI would cost several copies of it in RAM. Rust reads the source file and
    // writes each encrypted chunk out as its own file; only JSON metadata crosses.

    /**
     * Seal a media file to a contact's device. Writes one encrypted chunk per file into
     * `outDir`, named by its ciphertext hash. `previewPath` must ALREADY be a blurred /
     * downscaled image — it is sealed inside the manifest and never uploaded on its own;
     * pass "" for none.
     *
     * Returns `{ok, seal_id, mailbox_tag, bundle, shares, chunk_count, chunks:[{index,hash,path,size}]}`
     * — the same bundle/shares shape a text seal produces, so `shipSeal` works unchanged.
     */
    fun sealMediaFile(
        identitySeed: String, senderCard: String, devicePub: String, inPath: String,
        mime: String, kind: String, previewPath: String, outDir: String,
        fast: Boolean = false, revealAt: Long = 0, destroyAt: Long = 0,
    ): String = nativeSealMediaFile(
        identitySeed, senderCard, devicePub, inPath, mime, kind, previewPath, outDir, fast, revealAt, destroyAt,
    )

    /**
     * Media step 1 — open the MANIFEST only, to learn what the item is and which chunks to
     * fetch: `{ok, mime_type, kind, chunk_count, chunks:[hash], plaintext_size}`.
     * Writes the locked preview to `previewOut` when the item carries one.
     */
    fun openMediaInfo(deviceSeed: String, bundle: String, shares: String, previewOut: String): String =
        nativeOpenMediaInfo(deviceSeed, bundle, shares, previewOut)

    /**
     * Media step 2 — every chunk having been downloaded into `chunkDir` (each file named by
     * its hex hash, exactly as `openMediaInfo` listed), decrypt and reassemble into `outPath`.
     * A missing or altered chunk fails here rather than producing a corrupt file.
     */
    fun openMediaFile(deviceSeed: String, bundle: String, shares: String, chunkDir: String, outPath: String): String =
        nativeOpenMediaFile(deviceSeed, bundle, shares, chunkDir, outPath)
}
