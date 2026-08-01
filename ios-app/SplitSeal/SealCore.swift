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

    /// Directory key for a phone number: {normalized, phone_commitment}. hash(phone) → address (off-chain).
    static func phoneCommitment(_ phone: String) -> String {
        phone.withCString { copy(ss_phone_commitment($0)) }
    }

    /// My inbox tag {ok, mailbox_tag} from my device pubkey — poll the relay here for inbound seals.
    static func mailboxTag(_ devicePub: String) -> String {
        devicePub.withCString { copy(ss_mailbox_tag($0)) }
    }

    /// Seal text to a contact's device pubkey (hex). Returns {ok, seal_id, recipient_device_commitment, ciphertext_len}.
    static func sealTo(_ devicePub: String, _ text: String, _ fast: Bool) -> String {
        devicePub.withCString { d in text.withCString { t in copy(ss_seal_to(d, t, fast ? 1 : 0)) } }
    }

    /// Seal (signed by MY identitySeed, with MY card embedded so the peer can reply) + return
    /// {ok, seal_id, mailbox_tag, bundle, shares, reveal_at, destroy_at}. revealAt/destroyAt are
    /// unix secs (0 = none) — a timelock window enforced by the gateways.
    static func sealShippable(_ identitySeed: String, _ senderCard: String, _ devicePub: String, _ text: String, _ fast: Bool, _ revealAt: Int64 = 0, _ destroyAt: Int64 = 0, _ currentSlot: Int64 = 0) -> String {
        identitySeed.withCString { i in senderCard.withCString { c in devicePub.withCString { d in text.withCString { t in copy(ss_seal_shippable(i, c, d, t, fast ? 1 : 0, revealAt, destroyAt, currentSlot)) } } } }
    }

    /// Open a collected message with the recipient's device seed. Returns {ok, plaintext} or {ok:false, reason}.
    static func openReceived(_ deviceSeed: String, _ bundle: String, _ shares: String, _ currentSlot: Int64 = 0) -> String {
        deviceSeed.withCString { s in bundle.withCString { b in shares.withCString { sh in copy(ss_open_received(s, b, sh, currentSlot)) } } }
    }

    // ── media ──
    // Media crosses this boundary as FILE PATHS, never as bytes: pushing a 40 MB video
    // through the bridge would cost several copies of it in RAM. Rust reads the source file
    // and writes each encrypted chunk out as its own file; only JSON metadata comes back.

    /// Seal a media file to a contact's device. Writes one encrypted chunk per file into
    /// `outDir`, named by its ciphertext hash. `previewPath` must ALREADY be a blurred /
    /// downscaled image — it is sealed inside the manifest and never uploaded on its own;
    /// pass "" for none. `kind` is "image" | "video" | "audio" | "file".
    ///
    /// Returns `{ok, seal_id, mailbox_tag, bundle, shares, chunk_count, chunks:[{index,hash,path,size}]}`
    /// — the same bundle/shares shape a text seal produces, so `shipSeal` works unchanged.
    static func sealMediaFile(
        _ identitySeed: String, _ senderCard: String, _ devicePub: String, _ inPath: String,
        _ mime: String, _ kind: String, _ caption: String, _ previewPath: String, _ outDir: String,
        _ fast: Bool = false, _ revealAt: Int64 = 0, _ destroyAt: Int64 = 0, _ currentSlot: Int64 = 0
    ) -> String {
        identitySeed.withCString { i in senderCard.withCString { c in devicePub.withCString { d in
        inPath.withCString { p in mime.withCString { m in kind.withCString { k in
        caption.withCString { cap in previewPath.withCString { pv in outDir.withCString { o in
            copy(ss_seal_media_file(i, c, d, p, m, k, cap, pv, o, fast ? 1 : 0, revealAt, destroyAt, currentSlot))
        } } } } } } } } }
    }

    /// Media step 1 — open the MANIFEST only, to learn what the item is and which chunks to
    /// fetch: `{ok, mime_type, kind, chunk_count, chunks:[hash], plaintext_size}`.
    /// Writes the locked preview to `previewOut` when the item carries one.
    static func openMediaInfo(_ deviceSeed: String, _ bundle: String, _ shares: String, _ previewOut: String, _ currentSlot: Int64 = 0) -> String {
        deviceSeed.withCString { s in bundle.withCString { b in shares.withCString { sh in
        previewOut.withCString { pv in copy(ss_open_media_info(s, b, sh, pv, currentSlot)) } } } }
    }

    /// Media step 2 — every chunk having been downloaded into `chunkDir` (each file named by
    /// its hex hash, exactly as `openMediaInfo` listed), decrypt and reassemble into `outPath`.
    /// A missing or altered chunk fails here rather than producing a corrupt file.
    static func openMediaFile(_ deviceSeed: String, _ bundle: String, _ shares: String, _ chunkDir: String, _ outPath: String, _ currentSlot: Int64 = 0) -> String {
        deviceSeed.withCString { s in bundle.withCString { b in shares.withCString { sh in
        chunkDir.withCString { c in outPath.withCString { o in copy(ss_open_media_file(s, b, sh, c, o, currentSlot)) } } } } }
    }

    /// Verify a REAL on-chain anchor against a bundle's own leaf: `{ok, anchor_slot}`.
    /// `txJson` is the body of `GET /transaction/<anchor_sig>` from a WCAHT node. The anchor
    /// commits the leaf hash as the recipient address, so a confirmed tx paying that address
    /// is the chain attesting that this leaf existed by that slot — checked here against bytes
    /// the recipient already holds, trusting neither the relay nor the gateways.
    static func verifyAnchor(_ bundle: String, _ txJson: String) -> String {
        bundle.withCString { b in txJson.withCString { t in copy(ss_verify_anchor(b, t)) } }
    }

    /// Copy a Rust-owned C string into a Swift String and free the original.
    private static func copy(_ ptr: UnsafeMutablePointer<CChar>?) -> String {
        guard let ptr = ptr else { return "{}" }
        defer { ss_free(ptr) }
        return String(cString: ptr)
    }
}
