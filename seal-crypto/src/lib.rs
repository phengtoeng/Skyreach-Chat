//! DSCP-1 crypto facade — a NARROW wrapper over audited, maintained crates.
//!
//! Per spec §7.1 we do NOT invent cryptography. Everything here delegates to:
//!   - BLAKE3           domain-separated hashing
//!   - Ed25519 (dalek)  identity / device / leaf signatures
//!   - X25519 (dalek)   hybrid public-key encryption to a recipient device key
//!   - XChaCha20-Poly1305  AEAD for content and key-share envelopes
//!   - Shamir (sharks)  t-of-n threshold split of the content key
//!
//! The rest of the system depends ONLY on this interface, so the primitives can
//! be swapped/audited without touching protocol logic.

use anyhow::{anyhow, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    Key, XChaCha20Poly1305, XNonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use sharks::{Share, Sharks};
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::Zeroize;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const SIG_LEN: usize = 64;

/// 32 cryptographically-secure random bytes (OS CSPRNG).
pub fn random_32() -> [u8; 32] {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    b
}

/// Domain-separated BLAKE3 hash: `BLAKE3(domain || 0x1F || data)`.
/// Every protocol object hashes under its own domain string (spec §7.2).
pub fn hash(domain: &str, data: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(domain.as_bytes());
    h.update(&[0x1f]); // unit separator, unambiguous framing
    h.update(data);
    *h.finalize().as_bytes()
}

// ───────────────────────── Ed25519 signing identity ─────────────────────────

/// A signing key (chat identity, device, or leaf signer). Private key never leaves.
pub struct SignId {
    sk: SigningKey,
}

impl SignId {
    pub fn generate() -> Self {
        Self { sk: SigningKey::from_bytes(&random_32()) }
    }
    /// Deterministic key from a 32-byte seed (tests / key derivation only).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self { sk: SigningKey::from_bytes(seed) }
    }
    pub fn public(&self) -> [u8; 32] {
        self.sk.verifying_key().to_bytes()
    }
    pub fn sign(&self, msg: &[u8]) -> [u8; SIG_LEN] {
        self.sk.sign(msg).to_bytes()
    }
}

/// Verify an Ed25519 signature. Returns false on any malformed input (never panics).
pub fn verify_sig(pubkey: &[u8; 32], msg: &[u8], sig: &[u8; SIG_LEN]) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(pubkey) else {
        return false;
    };
    vk.verify(msg, &Signature::from_bytes(sig)).is_ok()
}

// ───────────────────────── XChaCha20-Poly1305 AEAD ──────────────────────────

/// AEAD-seal `plaintext` with `key`, binding `aad` (associated data). Returns
/// `(ciphertext, nonce)`. Nonce is random 24 bytes (XChaCha extended nonce).
pub fn aead_seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, [u8; NONCE_LEN])> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad })
        .map_err(|_| anyhow!("aead seal failed"))?;
    Ok((ct, nonce))
}

/// AEAD-open. Fails (auth error) if the key, nonce, aad, or ciphertext were tampered.
pub fn aead_open(key: &[u8; 32], nonce: &[u8; NONCE_LEN], aad: &[u8], ct: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| anyhow!("aead open failed: authentication tag mismatch"))
}

// ─────────────────── X25519 hybrid PKE to a device key ──────────────────────

/// A per-device X25519 key. Receives encrypted content-key shares (spec §5.2).
pub struct DeviceKey {
    sk: StaticSecret,
}

impl DeviceKey {
    pub fn generate() -> Self {
        Self { sk: StaticSecret::from(random_32()) }
    }
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { sk: StaticSecret::from(seed) }
    }
    pub fn public(&self) -> [u8; 32] {
        XPublicKey::from(&self.sk).to_bytes()
    }
}

/// Sealed envelope produced by [`hpke_seal`]: an ephemeral pubkey + AEAD nonce + ciphertext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HpkeBox {
    pub eph_pub: [u8; 32],
    pub nonce: [u8; NONCE_LEN],
    pub ct: Vec<u8>,
}

/// Encrypt `plaintext` TO `recipient_pub` (X25519). Ephemeral-static ECDH → KDF → AEAD.
/// Only the holder of the matching device private key can open it.
pub fn hpke_seal(recipient_pub: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<HpkeBox> {
    let eph = StaticSecret::from(random_32());
    let eph_pub = XPublicKey::from(&eph).to_bytes();
    let mut shared = eph.diffie_hellman(&XPublicKey::from(*recipient_pub)).to_bytes();
    let key = hash("DSCP-1/hpke-kdf", &shared);
    shared.zeroize();
    let (ct, nonce) = aead_seal(&key, aad, plaintext)?;
    Ok(HpkeBox { eph_pub, nonce, ct })
}

/// Open an [`HpkeBox`] with the recipient device key.
pub fn hpke_open(recipient: &DeviceKey, boxed: &HpkeBox, aad: &[u8]) -> Result<Vec<u8>> {
    let mut shared = recipient.sk.diffie_hellman(&XPublicKey::from(boxed.eph_pub)).to_bytes();
    let key = hash("DSCP-1/hpke-kdf", &shared);
    shared.zeroize();
    aead_open(&key, &boxed.nonce, aad, &boxed.ct)
}

// ───────────────────────── Shamir t-of-n threshold ──────────────────────────

/// Split a 32-byte secret into `n` shares; any `t` reconstruct it (spec §6.2).
pub fn shamir_split(secret: &[u8; 32], t: u8, n: u8) -> Vec<Vec<u8>> {
    let sharks = Sharks(t);
    sharks.dealer(secret).take(n as usize).map(|s| Vec::from(&s)).collect()
}

/// Reconstruct a secret from at least `t` shares. Fewer than `t` cannot recover it.
pub fn shamir_combine(t: u8, shares: &[Vec<u8>]) -> Result<Vec<u8>> {
    let parsed: std::result::Result<Vec<Share>, _> =
        shares.iter().map(|b| Share::try_from(b.as_slice())).collect();
    let parsed = parsed.map_err(|e| anyhow!("malformed share: {e}"))?;
    Sharks(t)
        .recover(parsed.as_slice())
        .map_err(|e| anyhow!("shamir recover failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aead_roundtrip_and_aad_binding() {
        let key = random_32();
        let (ct, nonce) = aead_seal(&key, b"aad", b"hello").unwrap();
        assert_eq!(aead_open(&key, &nonce, b"aad", &ct).unwrap(), b"hello");
        // wrong AAD must fail (binds context)
        assert!(aead_open(&key, &nonce, b"other", &ct).is_err());
        // tampered ciphertext must fail
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert!(aead_open(&key, &nonce, b"aad", &bad).is_err());
    }

    #[test]
    fn signatures_verify_and_reject() {
        let id = SignId::generate();
        let sig = id.sign(b"msg");
        assert!(verify_sig(&id.public(), b"msg", &sig));
        assert!(!verify_sig(&id.public(), b"tampered", &sig));
    }

    #[test]
    fn hpke_only_recipient_opens() {
        let bob = DeviceKey::generate();
        let mallory = DeviceKey::generate();
        let boxed = hpke_seal(&bob.public(), b"aad", b"share-bytes").unwrap();
        assert_eq!(hpke_open(&bob, &boxed, b"aad").unwrap(), b"share-bytes");
        assert!(hpke_open(&mallory, &boxed, b"aad").is_err());
    }

    #[test]
    fn shamir_threshold() {
        let secret = random_32();
        let shares = shamir_split(&secret, 2, 3);
        // any 2 recover
        assert_eq!(shamir_combine(2, &shares[0..2]).unwrap(), secret.to_vec());
        assert_eq!(shamir_combine(2, &[shares[0].clone(), shares[2].clone()]).unwrap(), secret.to_vec());
        // 1 share cannot recover the secret
        assert_ne!(shamir_combine(2, &shares[0..1]).map(|v| v == secret.to_vec()).unwrap_or(false), true);
    }
}
