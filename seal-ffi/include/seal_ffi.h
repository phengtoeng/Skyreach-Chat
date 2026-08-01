/* Denvion SplitSeal — seal-ffi C ABI (DSCP-2).
 * Every ss_* function returns a heap JSON C string owned by the caller;
 * release it with ss_free. */
#ifndef SEAL_FFI_H
#define SEAL_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

/* { "protocol":"DSCP-2", "version":2, "chain_id":7789 } */
char *ss_version(void);

/* StrictSeal: full seal -> (locked) -> L1 finalise -> (opened) transcript as JSON. */
char *ss_run_demo(void);

/* FastSeal: seal -> (locked) -> gateway pre-confirmation quorum -> (opened, pre-finality). */
char *ss_run_fast_demo(void);

/* Create a new account (identity + device keys). Returns JSON:
 *   { name, address, identity_seed, device_seed, identity_pub, device_pub, card }
 * The app MUST persist identity_seed + device_seed (platform keystore). */
char *ss_new_identity(const char *name);

/* Rebuild { name, address, identity_pub, device_pub, card } from stored seeds. */
char *ss_card_for(const char *identity_seed, const char *device_seed, const char *name);

/* Validate a scanned/pasted contact code ("denvion:…"):
 *   { ok:true, name, address, identity_pub, device_pub }  or  { ok:false, error }. */
char *ss_parse_card(const char *code);

/* Privacy-preserving directory key for a phone number: { normalized, phone_commitment }.
 * The raw number never goes on-chain; hash(phone) resolves to an address in the directory. */
char *ss_phone_commitment(const char *phone);

/* Inbox tag for a device pubkey (hex): { ok, mailbox_tag }.
 * A recipient computes this from its OWN device pubkey and polls the relay at that tag. */
char *ss_mailbox_tag(const char *device_pub);

/* Seal text to a contact's device public key (hex). Openable only by that device.
 *   { ok, seal_id, recipient_device_commitment, ciphertext_len }  (fast: 1=FastSeal, 0=StrictSeal) */
char *ss_seal_to(const char *device_pub, const char *text, int fast);

/* Seal + return artifacts to SHIP over the services (real cross-device delivery):
 *   { ok, seal_id, mailbox_tag, bundle, shares }  (bundle → relay, shares[i] → gateway i)
 * identity_seed = the SENDER's stored identity seed; bundle.sender_id_pub = sender identity_pub.
 * sender_card = the SENDER's own contact card, embedded in the bundle (bundle.sender_card) so the
 * recipient can identify + REPLY without adding the sender first (self-describing message). */
/* reveal_at / destroy_at are unix seconds (0 = none): a timelock window. The gateways withhold
 * the key shares before reveal_at and drop them after destroy_at (self-destruct), so the recipient
 * cannot reconstruct the key outside the window; open_received enforces it too (defense-in-depth). */
char *ss_seal_shippable(const char *identity_seed, const char *sender_card, const char *device_pub, const char *text, int fast, long long reveal_at, long long destroy_at);

/* Open a message COLLECTED from the services:
 *   { ok, plaintext }  or  { ok:false, reason }  (device_seed = recipient's stored seed). */
char *ss_open_received(const char *device_seed, const char *bundle, const char *shares);

/* ── media ────────────────────────────────────────────────────────────────────
 * Media crosses this boundary as FILE PATHS, never as bytes: pushing a 40 MB video
 * through the bridge would cost several copies of it in RAM. Rust reads the source
 * file and writes each encrypted chunk out as its own file.
 *
 * The envelope carries only an encrypted manifest; the pixels live in the chunks,
 * which the relay stores as opaque blobs. The chain sees just `manifest_root`. */

/* Seal a media FILE (image/video/audio/document). Writes one encrypted chunk per file into
 * out_dir, named by its ciphertext hash, and returns the SAME {bundle, shares} shape a text
 * seal produces — so the existing ship/collect plumbing is unchanged:
 *   { ok, seal_id, mailbox_tag, bundle, shares, chunk_count, chunks:[{index,hash,path,size}] }
 * kind = "image" | "video" | "audio" | "file".
 * preview_path must ALREADY be blurred/downscaled — it is sealed INSIDE the manifest and is
 * never uploaded on its own (spec §10.3); pass "" for none. */
char *ss_seal_media_file(const char *identity_seed, const char *sender_card, const char *device_pub,
                         const char *in_path, const char *mime, const char *kind,
                         const char *preview_path, const char *out_dir,
                         int fast, long long reveal_at, long long destroy_at);

/* Media step 1 — open the MANIFEST only, to learn what the item is and which chunks to fetch:
 *   { ok, mime_type, kind, plaintext_size, chunk_count, chunks:[hash], width, height, duration_ms }
 * Writes the locked preview to preview_out when the item carries one ("" to skip). */
char *ss_open_media_info(const char *device_seed, const char *bundle, const char *shares,
                         const char *preview_out);

/* Media step 2 — with every chunk downloaded into chunk_dir (each file named by its hex hash,
 * exactly as ss_open_media_info listed), decrypt and reassemble into out_path:
 *   { ok, out_path, mime_type, kind, bytes }  or  { ok:false, reason }.
 * A missing or altered chunk fails here rather than producing a corrupt file. */
char *ss_open_media_file(const char *device_seed, const char *bundle, const char *shares,
                         const char *chunk_dir, const char *out_path);

/* Free a string returned by any ss_* function. Safe on NULL. */
void ss_free(char *ptr);

#ifdef __cplusplus
}
#endif

#endif /* SEAL_FFI_H */
