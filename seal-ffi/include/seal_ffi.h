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
 * identity_seed = the SENDER's stored identity seed; bundle.sender_id_pub = sender identity_pub. */
char *ss_seal_shippable(const char *identity_seed, const char *device_pub, const char *text, int fast);

/* Open a message COLLECTED from the services:
 *   { ok, plaintext }  or  { ok:false, reason }  (device_seed = recipient's stored seed). */
char *ss_open_received(const char *device_seed, const char *bundle, const char *shares);

/* Free a string returned by any ss_* function. Safe on NULL. */
void ss_free(char *ptr);

#ifdef __cplusplus
}
#endif

#endif /* SEAL_FFI_H */
