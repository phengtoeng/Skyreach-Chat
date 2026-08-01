//! DSCP-1 protocol core (Denvion SplitSeal).
//!
//! Implements the "locked until finalised" promise (spec §2): a recipient cannot
//! render plaintext until (1) ciphertext arrived, (2) its hash matches the sealed
//! commitment, (3) enough key shares addressed to this device are available,
//! (4) the WCAHT seal is FINALISED, and (5) the item is not expired/revoked/replayed/
//! downgraded.
//!
//! This crate is UI-agnostic and platform-agnostic so it can be compiled into the
//! iOS/Android app (via `seal-ffi`) and reused by services.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use seal_crypto as sc;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const CHAIN_ID: u64 = 7789;
/// DSCP-2: adds FastSeal pre-confirmations alongside StrictSeal L1-finality vault mode.
pub const PROTOCOL_VERSION: u16 = 2;

/// Release discipline bound into every seal leaf (spec DSCP-2 §4). Because it lives in
/// the leaf, it is covered by the sender signature and the leaf hash — a seal cannot be
/// silently downgraded from StrictSeal to FastSeal.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealMode {
    /// Vault mode: opens ONLY at hard L1 finality. Pre-confirmations are ignored.
    StrictSeal,
    /// Fast mode: opens on a quorum of slashable gateway pre-confirmations (sub-250ms)
    /// OR at L1 finality, whichever lands first. Gateways stake a bond and are slashable
    /// if they pre-confirm a leaf that L1 later contradicts.
    FastSeal,
}

// ───────────────────────────── protocol objects ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Media,
    Document,
    CallSession,
}

/// The immutable, signed commitment to exactly one encrypted item (spec §8.3).
/// `sender_signature` is stored in [`SignedLeaf`], not here, so the signed bytes
/// are exactly `canonical_bytes(leaf)`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SealLeaf {
    pub protocol_version: u16,
    pub chain_id: u64,
    pub seal_id: [u8; 32],
    pub content_type: ContentType,
    pub ciphertext_hash: [u8; 32],
    pub manifest_root: [u8; 32],
    pub recipient_device_commitment: [u8; 32],
    pub sender_identity_commitment: [u8; 32],
    pub sender_device_commitment: [u8; 32],
    pub key_share_commitment_root: [u8; 32],
    pub threshold_t: u8,
    pub threshold_n: u8,
    pub not_before_finalized_slot: u64,
    pub expires_at_slot: u64,
    /// Timelock window, unix seconds, 0 = none. These live IN the leaf so they are covered by
    /// the sender signature and by the leaf hash that is anchored on-chain — a relay cannot
    /// move a deadline, and a patched client cannot ignore one (see `try_open_inner`).
    pub reveal_at_unix: u64,
    pub destroy_at_unix: u64,
    pub flags: u32,
    /// DSCP-2 release discipline (StrictSeal vault vs FastSeal fast-path).
    pub mode: SealMode,
}

impl SealLeaf {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("SealLeaf is always serializable")
    }
    pub fn leaf_hash(&self) -> [u8; 32] {
        sc::hash("DSCP-1/SEAL_LEAF", &self.canonical_bytes())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SignedLeaf {
    pub leaf: SealLeaf,
    /// Ed25519 signature (64 bytes) by the sender chat-identity key over `canonical_bytes(leaf)`.
    pub sender_signature: Vec<u8>,
}

/// Delivery-route object (spec §8.1). Carries the AEAD ciphertext of the content.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EncryptedEnvelope {
    pub protocol_version: u16,
    pub seal_id: [u8; 32],
    pub recipient_mailbox_tag: [u8; 32],
    pub aad_commitment: [u8; 32],
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

/// Serializable X25519 HPKE box (mirrors `seal_crypto::HpkeBox`; keeps serde out of the crypto crate).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ShareBox {
    pub eph_pub: [u8; 32],
    pub nonce: [u8; 24],
    pub ct: Vec<u8>,
}
impl From<sc::HpkeBox> for ShareBox {
    fn from(b: sc::HpkeBox) -> Self {
        Self { eph_pub: b.eph_pub, nonce: b.nonce, ct: b.ct }
    }
}
impl ShareBox {
    fn to_hpke(&self) -> sc::HpkeBox {
        sc::HpkeBox { eph_pub: self.eph_pub, nonce: self.nonce, ct: self.ct.clone() }
    }
}

/// Seal-route object (spec §8.4). One per gateway; encrypted to the recipient device.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KeyShareEnvelope {
    pub seal_id: [u8; 32],
    pub gateway_id: [u8; 32],
    pub share_index: u8,
    pub recipient_device_commitment: [u8; 32],
    pub encrypted_share: ShareBox,
    pub share_commitment: [u8; 32],
    pub leaf_hash: [u8; 32],
    pub expires_at_slot: u64,
}

// ───────────────────────────── commitments / AAD ────────────────────────────

/// Wall-clock seconds. Used only for the leaf's timelock window; every other gate is
/// slot-based and read from the chain.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn device_commitment(device_pub: &[u8; 32]) -> [u8; 32] {
    sc::hash("DSCP-1/device", device_pub)
}
pub fn identity_commitment(identity_pub: &[u8; 32]) -> [u8; 32] {
    sc::hash("DSCP-1/identity", identity_pub)
}
fn ciphertext_hash(nonce: &[u8; 24], ct: &[u8]) -> [u8; 32] {
    let mut b = Vec::with_capacity(24 + ct.len());
    b.extend_from_slice(nonce);
    b.extend_from_slice(ct);
    sc::hash("DSCP-1/ciphertext", &b)
}
fn content_type_tag(c: ContentType) -> u8 {
    match c {
        ContentType::Text => 1,
        ContentType::Media => 2,
        ContentType::Document => 3,
        ContentType::CallSession => 4,
    }
}

/// Associated data bound into every AEAD operation (spec §7.2). Uses only stable
/// identity/expiry fields, so sender and recipient derive the same value.
fn compute_aad(
    chain_id: u64,
    seal_id: &[u8; 32],
    content_type: ContentType,
    recipient_device_commitment: &[u8; 32],
    sender_identity_commitment: &[u8; 32],
    expires_at_slot: u64,
) -> [u8; 32] {
    let mut ctx = Vec::new();
    ctx.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    ctx.extend_from_slice(&chain_id.to_le_bytes());
    ctx.extend_from_slice(seal_id);
    ctx.push(content_type_tag(content_type));
    ctx.extend_from_slice(recipient_device_commitment);
    ctx.extend_from_slice(sender_identity_commitment);
    ctx.extend_from_slice(&expires_at_slot.to_le_bytes());
    sc::hash("DSCP-1/aad", &ctx)
}
fn leaf_aad(leaf: &SealLeaf) -> [u8; 32] {
    compute_aad(
        leaf.chain_id,
        &leaf.seal_id,
        leaf.content_type,
        &leaf.recipient_device_commitment,
        &leaf.sender_identity_commitment,
        leaf.expires_at_slot,
    )
}

// ───────────────────────── identity, address & contacts ─────────────────────

/// Contact-card wire version (bump when the layout changes).
pub const CONTACT_CARD_VERSION: u8 = 1;

/// A user's on-chain identity **address** = base58 of their Ed25519 identity public key.
/// This is a valid WCAHT address (a 32-byte ed25519 pubkey, base58-encoded), but it is
/// the *identity* key — kept separate from any spending wallet and any device key.
pub fn wcaht_address(identity_pub: &[u8; 32]) -> String {
    bs58::encode(identity_pub).into_string()
}

/// A self-custodied account: a chat-identity key (Ed25519) + a device key (X25519),
/// held as their 32-byte seeds so the app can persist them in the platform keystore and
/// reconstruct the keys deterministically. Never mixed with the WCAHT wallet key.
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub identity_seed: [u8; 32],
    pub device_seed: [u8; 32],
}

impl Identity {
    pub fn generate(name: &str) -> Self {
        Self { name: name.to_string(), identity_seed: sc::random_32(), device_seed: sc::random_32() }
    }
    pub fn from_seeds(name: &str, identity_seed: [u8; 32], device_seed: [u8; 32]) -> Self {
        Self { name: name.to_string(), identity_seed, device_seed }
    }
    pub fn sign_id(&self) -> sc::SignId {
        sc::SignId::from_seed(&self.identity_seed)
    }
    pub fn device(&self) -> sc::DeviceKey {
        sc::DeviceKey::from_seed(self.device_seed)
    }
    pub fn identity_pub(&self) -> [u8; 32] {
        self.sign_id().public()
    }
    pub fn device_pub(&self) -> [u8; 32] {
        self.device().public()
    }
    /// The WCAHT identity address for this account.
    pub fn address(&self) -> String {
        wcaht_address(&self.identity_pub())
    }
    /// The shareable card a QR / invite encodes (what a contact scans to reach you).
    pub fn card(&self) -> ContactCard {
        ContactCard {
            version: CONTACT_CARD_VERSION,
            chain_id: CHAIN_ID,
            name: self.name.clone(),
            identity_pub: self.identity_pub(),
            device_pub: self.device_pub(),
        }
    }
}

/// The public half of an identity: everything needed to seal a message TO someone and
/// verify their seals. This is what you exchange (QR / invite code) to add a contact.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContactCard {
    pub version: u8,
    pub chain_id: u64,
    pub name: String,
    pub identity_pub: [u8; 32],
    pub device_pub: [u8; 32],
}

impl ContactCard {
    /// The contact's WCAHT identity address.
    pub fn address(&self) -> String {
        wcaht_address(&self.identity_pub)
    }

    /// Encode as a compact, scannable invite: `denvion:<base58(payload)>`.
    /// Payload = version | chain_id(LE) | identity_pub(32) | device_pub(32) | name.
    pub fn encode(&self) -> String {
        let mut v = Vec::with_capacity(80 + self.name.len());
        v.push(self.version);
        v.extend_from_slice(&self.chain_id.to_le_bytes());
        v.extend_from_slice(&self.identity_pub);
        v.extend_from_slice(&self.device_pub);
        v.extend_from_slice(self.name.as_bytes());
        format!("denvion:{}", bs58::encode(v).into_string())
    }

    /// Parse + validate an invite string. Rejects a wrong chain id or unknown version.
    pub fn decode(s: &str) -> Result<ContactCard> {
        let b58 = s.trim().strip_prefix("denvion:").unwrap_or(s.trim());
        let v = bs58::decode(b58).into_vec().map_err(|_| anyhow!("invalid contact code"))?;
        if v.len() < 1 + 8 + 32 + 32 {
            return Err(anyhow!("contact code too short"));
        }
        let version = v[0];
        if version != CONTACT_CARD_VERSION {
            return Err(anyhow!("unsupported card version {version}"));
        }
        let chain_id = u64::from_le_bytes(v[1..9].try_into().unwrap());
        if chain_id != CHAIN_ID {
            return Err(anyhow!("wrong chain id {chain_id} (expected {CHAIN_ID})"));
        }
        let mut identity_pub = [0u8; 32];
        identity_pub.copy_from_slice(&v[9..41]);
        let mut device_pub = [0u8; 32];
        device_pub.copy_from_slice(&v[41..73]);
        let name = String::from_utf8_lossy(&v[73..]).to_string();
        Ok(ContactCard { version, chain_id, name, identity_pub, device_pub })
    }
}

// ───────────────── phone ↔ address directory (privacy-preserving) ───────────

/// Normalize a phone number to its digits (dropping `+`, spaces, and punctuation), so
/// different formattings of the same number map to the same commitment. Numbers should
/// include the country code (E.164), e.g. `+1 555 123 4567` and `15551234567` are equal.
pub fn normalize_phone(raw: &str) -> String {
    raw.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// A privacy-preserving commitment to a phone number. This — never the raw number and
/// never anything on-chain — is the directory key that resolves to a WCAHT address.
pub fn phone_commitment(phone: &str) -> [u8; 32] {
    sc::hash("DSCP-2/phone", normalize_phone(phone).as_bytes())
}

/// Deterministic delivery-relay mailbox address for a device. The sender derives it
/// from the recipient's device pubkey to address the ciphertext; the recipient derives
/// the same value from their own key to poll for it. (v1: derivable from the device key
/// — rotating/blinded tags are a privacy follow-up.)
pub fn mailbox_tag(device_pub: &[u8; 32]) -> [u8; 32] {
    sc::hash("DSCP-2/mailbox", device_pub)
}

/// Off-chain phone directory: maps a phone COMMITMENT to the owner's contact card
/// (address + device key). The raw number is never stored and never goes on-chain. The
/// production directory is a registered service (SMS proof-of-ownership + a signed
/// binding); this mock demonstrates the resolution.
#[derive(Default)]
pub struct MockDirectory {
    by_phone: std::sync::Mutex<HashMap<[u8; 32], ContactCard>>,
}

impl MockDirectory {
    pub fn new() -> Self {
        Self::default()
    }
    /// Publish `phone → your card`. Production requires SMS proof + your signature.
    pub fn register(&self, phone: &str, card: ContactCard) {
        self.by_phone.lock().unwrap().insert(phone_commitment(phone), card);
    }
    /// Resolve a phone number to its owner's card (address + device key), or `None`.
    pub fn lookup(&self, phone: &str) -> Option<ContactCard> {
        self.by_phone.lock().unwrap().get(&phone_commitment(phone)).cloned()
    }
}

// ─────────────────────────── mock WCAHT seal chain ──────────────────────────

/// Lifecycle exposed by RPC (spec §9.3). The client opens ONLY at `Finalised`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealStatus {
    Unknown,
    Submitted,
    Finalising,
    Finalised,
    Expired,
    Revoked,
    Invalid,
}

/// Serializable so a batcher can serve it over HTTP and a recipient can verify it off the wire.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SealProof {
    pub seal_id: [u8; 32],
    pub leaf_hash: [u8; 32],
    /// Sibling hashes bottom-up. Empty means the root IS the leaf (a one-leaf batch).
    pub merkle_path: Vec<[u8; 32]>,
    /// Position of this leaf in the batch — decides left/right at each level (spec §8.5).
    pub leaf_index: u32,
    /// How many leaves the batch held (spec §9.2). Needed to replay level widths.
    pub leaf_count: u32,
    pub seal_root: [u8; 32],
    pub finalized_slot: u64,
}

struct SealRecord {
    leaf_hash: [u8; 32],
    root: [u8; 32],
    /// Position + siblings within the batch this leaf was anchored in. Empty path + index 0
    /// is the single-leaf case, where the root IS the leaf.
    merkle_path: Vec<[u8; 32]>,
    leaf_index: u32,
    leaf_count: u32,
    finalized_slot: Option<u64>,
    revoked: bool,
}

/// In-memory stand-in for the native WCAHT seal transactions/finality. The real
/// `wcaht-seal-sdk` will implement the same surface against chain ID 7789.
pub struct MockSealChain {
    current_slot: u64,
    seals: HashMap<[u8; 32], SealRecord>,
    seen_seal_ids: HashSet<[u8; 32]>, // replay protection (spec §18)
}

impl MockSealChain {
    pub fn new(start_slot: u64) -> Self {
        Self { current_slot: start_slot, seals: HashMap::new(), seen_seal_ids: HashSet::new() }
    }
    pub fn slot(&self) -> u64 {
        self.current_slot
    }
    pub fn advance(&mut self, slots: u64) {
        self.current_slot = self.current_slot.saturating_add(slots);
    }

    /// Submit a signed leaf. Validates signature, chain id, version, and rejects
    /// replays. Returns the `seal_id`. The chain never sees plaintext (spec §9).
    pub fn submit_leaf(&mut self, signed: &SignedLeaf, sender_identity_pub: &[u8; 32]) -> Result<[u8; 32]> {
        let leaf = &signed.leaf;
        if leaf.chain_id != CHAIN_ID {
            return Err(anyhow!("wrong chain id"));
        }
        if leaf.protocol_version != PROTOCOL_VERSION {
            return Err(anyhow!("unknown protocol version"));
        }
        if leaf.sender_identity_commitment != identity_commitment(sender_identity_pub) {
            return Err(anyhow!("sender identity commitment mismatch"));
        }
        let sig: [u8; 64] = signed
            .sender_signature
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("signature must be 64 bytes"))?;
        if !sc::verify_sig(sender_identity_pub, &leaf.canonical_bytes(), &sig) {
            return Err(anyhow!("invalid sender signature"));
        }
        if !self.seen_seal_ids.insert(leaf.seal_id) {
            return Err(anyhow!("replay: seal_id already submitted"));
        }
        let leaf_hash = leaf.leaf_hash();
        self.seals.insert(
            leaf.seal_id,
            SealRecord {
                leaf_hash,
                // a lone seal is simply a one-leaf batch, so its root is the batch root of
                // itself — keeping one verification rule for batched and unbatched alike
                root: seal_batch_root(&[leaf_hash]),
                merkle_path: Vec::new(),
                leaf_index: 0,
                leaf_count: 1,
                finalized_slot: None,
                revoked: false,
            },
        );
        Ok(leaf.seal_id)
    }

    /// Simulate the batch reaching FINALISED at the current slot.
    /// Submit a whole BATCH under one root — the SEAL_ROOT shape. Every leaf keeps its own
    /// merkle path, so each message is still individually provable; only the anchoring
    /// transaction is shared. This is what makes "a capsule per message" affordable.
    pub fn submit_batch(&mut self, signed: &[(SignedLeaf, [u8; 32])]) -> Result<[u8; 32]> {
        let leaf_hashes: Vec<[u8; 32]> = signed.iter().map(|(s, _)| s.leaf.leaf_hash()).collect();
        let root = seal_batch_root(&leaf_hashes);
        for (i, (sl, sender_identity_pub)) in signed.iter().enumerate() {
            let leaf_hash = leaf_hashes[i];
            let sig: [u8; 64] = sl
                .sender_signature
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("malformed sender signature"))?;
            if !sc::verify_sig(sender_identity_pub, &sl.leaf.canonical_bytes(), &sig) {
                return Err(anyhow!("invalid sender signature in batch at index {i}"));
            }
            let path = seal_batch_proof(&leaf_hashes, i).ok_or_else(|| anyhow!("no proof for index {i}"))?;
            self.seals.insert(
                sl.leaf.seal_id,
                SealRecord {
                    leaf_hash,
                    root,
                    merkle_path: path,
                    leaf_index: i as u32,
                    leaf_count: signed.len() as u32,
                    finalized_slot: None,
                    revoked: false,
                },
            );
        }
        Ok(root)
    }

    pub fn finalize(&mut self, seal_id: &[u8; 32]) -> Result<()> {
        let slot = self.current_slot;
        let rec = self.seals.get_mut(seal_id).ok_or_else(|| anyhow!("unknown seal"))?;
        rec.finalized_slot = Some(slot);
        Ok(())
    }

    pub fn revoke(&mut self, seal_id: &[u8; 32]) {
        if let Some(rec) = self.seals.get_mut(seal_id) {
            rec.revoked = true;
        }
    }

    pub fn status(&self, seal_id: &[u8; 32]) -> SealStatus {
        match self.seals.get(seal_id) {
            None => SealStatus::Unknown,
            Some(r) if r.revoked => SealStatus::Revoked,
            Some(r) => match r.finalized_slot {
                Some(_) => SealStatus::Finalised,
                None => SealStatus::Finalising,
            },
        }
    }

    pub fn proof(&self, seal_id: &[u8; 32]) -> Option<SealProof> {
        let r = self.seals.get(seal_id)?;
        Some(SealProof {
            seal_id: *seal_id,
            leaf_hash: r.leaf_hash,
            merkle_path: r.merkle_path.clone(),
            leaf_index: r.leaf_index,
            leaf_count: r.leaf_count,
            seal_root: r.root,
            finalized_slot: r.finalized_slot?,
        })
    }
}

/// The read-only chain surface `try_open` needs. `MockSealChain` implements it, and
/// so does `WcahtSealChain` (real WCAHT finality) in the `wcaht-seal-sdk` crate — so
/// the same `try_open` works against either without changes.
pub trait SealChain {
    fn slot(&self) -> u64;
    fn status(&self, seal_id: &[u8; 32]) -> SealStatus;
    fn proof(&self, seal_id: &[u8; 32]) -> Option<SealProof>;
}

impl SealChain for MockSealChain {
    fn slot(&self) -> u64 {
        MockSealChain::slot(self)
    }
    fn status(&self, seal_id: &[u8; 32]) -> SealStatus {
        MockSealChain::status(self, seal_id)
    }
    fn proof(&self, seal_id: &[u8; 32]) -> Option<SealProof> {
        MockSealChain::proof(self, seal_id)
    }
}

// ───────────────────────── seal gateway (strict release) ────────────────────

/// A seal gateway holds an encrypted key share and releases it ONLY after it
/// independently verifies the seal is FINALISED (the strict-release rule, §6.4).
/// This is what makes StrictSeal more than "an honest app that waits".
///
/// A gateway created [`with_identity`](Gateway::with_identity) also has a signing key,
/// so it can issue slashable [`PreConfirmation`]s and release its share on the FastSeal
/// fast path — before L1 finality — staking its bond on the seal being honest.
pub struct Gateway {
    pub id: [u8; 32],
    signer: Option<sc::SignId>,
    held: HashMap<[u8; 32], KeyShareEnvelope>,
}

impl Gateway {
    /// A strict (StrictSeal-only) gateway identified by an opaque id — no pre-confs.
    pub fn new(id: [u8; 32]) -> Self {
        Self { id, signer: None, held: HashMap::new() }
    }
    /// A staked FastSeal gateway. Its `id` is its signing public key, so its
    /// pre-confirmations are verifiable (and its equivocations attributable/slashable).
    pub fn with_identity(signer: sc::SignId) -> Self {
        Self { id: signer.public(), signer: Some(signer), held: HashMap::new() }
    }
    pub fn deposit(&mut self, env: KeyShareEnvelope) {
        self.held.insert(env.seal_id, env);
    }
    /// Release the share ONLY when the chain reports FINALISED (StrictSeal / vault).
    pub fn request_share(&self, seal_id: &[u8; 32], chain: &impl SealChain) -> Option<KeyShareEnvelope> {
        if chain.status(seal_id) == SealStatus::Finalised {
            self.held.get(seal_id).cloned()
        } else {
            None // strict: no early release
        }
    }
    /// FastSeal fast-path release: hand over the share on the gateway's own sequencing,
    /// WITHOUT waiting for L1 finality. Only staked (signing) gateways do this, because
    /// they simultaneously stake a slashable pre-confirmation on it.
    pub fn request_share_fast(&self, seal_id: &[u8; 32]) -> Option<KeyShareEnvelope> {
        self.signer.as_ref()?;
        self.held.get(seal_id).cloned()
    }
    /// Issue a slashable pre-confirmation for a seal this gateway holds a share for.
    pub fn pre_confirm(&self, seal_id: &[u8; 32], sequenced_slot: u64) -> Option<PreConfirmation> {
        let signer = self.signer.as_ref()?;
        let env = self.held.get(seal_id)?;
        Some(PreConfirmation::create(signer, *seal_id, env.leaf_hash, sequenced_slot, env.expires_at_slot))
    }
}

// ─────────────────── DSCP-2 gateway pre-confirmations (FastSeal) ─────────────

/// A staked gateway's signed promise that it has sequenced `seal_id` committing to
/// exactly `leaf_hash`, and that it stakes its bond on L1 finalising that same leaf.
/// A quorum of these lets a FastSeal item open before hard finality; if L1 later
/// finalises a DIFFERENT leaf for this seal_id, the pre-confirmation is fraud evidence.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PreConfirmation {
    pub chain_id: u64,
    pub seal_id: [u8; 32],
    pub leaf_hash: [u8; 32],
    pub gateway_id: [u8; 32],
    pub sequenced_slot: u64,
    pub expiry_slot: u64,
    /// Ed25519 signature (64 bytes) by the gateway over the canonical pre-conf preimage.
    pub signature: Vec<u8>,
}

impl PreConfirmation {
    fn preimage(
        chain_id: u64,
        seal_id: &[u8; 32],
        leaf_hash: &[u8; 32],
        gateway_id: &[u8; 32],
        sequenced_slot: u64,
        expiry_slot: u64,
    ) -> [u8; 32] {
        let mut v = Vec::with_capacity(96);
        v.extend_from_slice(&chain_id.to_le_bytes());
        v.extend_from_slice(seal_id);
        v.extend_from_slice(leaf_hash);
        v.extend_from_slice(gateway_id);
        v.extend_from_slice(&sequenced_slot.to_le_bytes());
        v.extend_from_slice(&expiry_slot.to_le_bytes());
        sc::hash("DSCP-2/PRECONF", &v)
    }

    pub fn create(
        signer: &sc::SignId,
        seal_id: [u8; 32],
        leaf_hash: [u8; 32],
        sequenced_slot: u64,
        expiry_slot: u64,
    ) -> Self {
        let gateway_id = signer.public();
        let msg = Self::preimage(CHAIN_ID, &seal_id, &leaf_hash, &gateway_id, sequenced_slot, expiry_slot);
        Self {
            chain_id: CHAIN_ID,
            seal_id,
            leaf_hash,
            gateway_id,
            sequenced_slot,
            expiry_slot,
            signature: signer.sign(&msg).to_vec(),
        }
    }

    /// Verify the gateway's signature over its own claim (never panics).
    pub fn verify(&self) -> bool {
        let Ok(sig): std::result::Result<[u8; 64], _> = self.signature.as_slice().try_into() else {
            return false;
        };
        let msg = Self::preimage(
            self.chain_id,
            &self.seal_id,
            &self.leaf_hash,
            &self.gateway_id,
            self.sequenced_slot,
            self.expiry_slot,
        );
        sc::verify_sig(&self.gateway_id, &msg, &sig)
    }
}

/// Fraud proof: a valid gateway pre-confirmation that commits to a different leaf than
/// the one L1 finalised for the same seal_id → equivocation → slashable.
#[derive(Clone, Debug)]
pub struct SlashingEvidence {
    pub gateway_id: [u8; 32],
    pub seal_id: [u8; 32],
    pub pre_confirmed_leaf_hash: [u8; 32],
    pub finalized_leaf_hash: [u8; 32],
    pub pre_confirmation: PreConfirmation,
}

impl SlashingEvidence {
    /// Re-check the fraud proof stands on its own: signed by the accused gateway AND
    /// genuinely conflicting with the finalised leaf.
    pub fn is_valid(&self) -> bool {
        self.pre_confirmation.verify()
            && self.pre_confirmation.gateway_id == self.gateway_id
            && self.pre_confirmation.leaf_hash == self.pre_confirmed_leaf_hash
            && self.pre_confirmed_leaf_hash != self.finalized_leaf_hash
    }
}

/// Detect gateway equivocation. Returns evidence when `pc` is a valid pre-confirmation
/// that commits to a leaf other than the one L1 finalised for the same seal_id.
pub fn detect_equivocation(pc: &PreConfirmation, finalized_leaf_hash: &[u8; 32]) -> Option<SlashingEvidence> {
    if !pc.verify() || &pc.leaf_hash == finalized_leaf_hash {
        return None;
    }
    Some(SlashingEvidence {
        gateway_id: pc.gateway_id,
        seal_id: pc.seal_id,
        pre_confirmed_leaf_hash: pc.leaf_hash,
        finalized_leaf_hash: *finalized_leaf_hash,
        pre_confirmation: pc.clone(),
    })
}

// ───────────────────── gateway staking / slashing registry ──────────────────

/// A gateway's economic standing. FastSeal only trusts pre-confirmations from a gateway
/// that is [`Active`](GatewayStanding::Active) here — bonded and not yet slashed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayStanding {
    /// Bonded and in good standing; its pre-confirmations count toward a FastSeal quorum.
    Active { bond: u128 },
    /// Caught equivocating: bond forfeited, pre-confirmations no longer count.
    Slashed { forfeited: u128 },
    /// Never bonded; pre-confirmations must not be trusted.
    Unbonded,
}

/// The economic layer over [`PreConfirmation`] / [`SlashingEvidence`]. Tracks which
/// gateways have staked a bond (each backed by a real on-chain stake anchor — see
/// `wcaht-seal-sdk`) and enforces slashing when a valid equivocation proof arrives.
#[derive(Default)]
pub struct GatewayRegistry {
    min_bond: u128,
    standing: HashMap<[u8; 32], GatewayStanding>,
}

impl GatewayRegistry {
    pub fn new(min_bond: u128) -> Self {
        Self { min_bond, standing: HashMap::new() }
    }
    pub fn min_bond(&self) -> u128 {
        self.min_bond
    }

    /// Bond a gateway. Rejects a below-minimum bond, and refuses to re-activate a gateway
    /// that has already been slashed.
    pub fn stake(&mut self, gateway_id: [u8; 32], bond: u128) -> Result<()> {
        if bond < self.min_bond {
            return Err(anyhow!("bond {bond} below minimum {}", self.min_bond));
        }
        if matches!(self.standing.get(&gateway_id), Some(GatewayStanding::Slashed { .. })) {
            return Err(anyhow!("gateway is slashed and cannot re-bond"));
        }
        self.standing.insert(gateway_id, GatewayStanding::Active { bond });
        Ok(())
    }

    /// Apply a fraud proof: if the evidence verifies AND the accused gateway is currently
    /// bonded, forfeit its bond and mark it slashed. Returns the forfeited amount.
    pub fn slash(&mut self, evidence: &SlashingEvidence) -> Result<u128> {
        if !evidence.is_valid() {
            return Err(anyhow!("invalid slashing evidence"));
        }
        match self.standing.get(&evidence.gateway_id) {
            Some(GatewayStanding::Active { bond }) => {
                let forfeited = *bond;
                self.standing.insert(evidence.gateway_id, GatewayStanding::Slashed { forfeited });
                Ok(forfeited)
            }
            Some(GatewayStanding::Slashed { .. }) => Err(anyhow!("gateway already slashed")),
            _ => Err(anyhow!("gateway is not bonded")),
        }
    }

    pub fn standing(&self, gateway_id: &[u8; 32]) -> GatewayStanding {
        self.standing.get(gateway_id).cloned().unwrap_or(GatewayStanding::Unbonded)
    }
    pub fn is_active(&self, gateway_id: &[u8; 32]) -> bool {
        matches!(self.standing.get(gateway_id), Some(GatewayStanding::Active { .. }))
    }

    /// Keep only pre-confirmations from active, bonded gateways — what a recipient passes
    /// to [`try_open_dscp2`] so a slashed or unbonded gateway can never help open an item.
    pub fn filter_active(&self, pre_confirmations: &[PreConfirmation]) -> Vec<PreConfirmation> {
        pre_confirmations.iter().filter(|pc| self.is_active(&pc.gateway_id)).cloned().collect()
    }

    pub fn total_bonded(&self) -> u128 {
        self.standing
            .values()
            .filter_map(|s| match s {
                GatewayStanding::Active { bond } => Some(*bond),
                _ => None,
            })
            .sum()
    }
}

// ─────────────────── delivery relay (payload prefetch, §8.1) ────────────────

/// The fast delivery route. It moves ONLY [`EncryptedEnvelope`]s (ciphertext), never
/// keys or plaintext — so it is an untrusted transport. Its job is to let a recipient
/// **prefetch the locked ciphertext** before the seal is openable, so that when the
/// release gate finally opens (pre-confirmation quorum or finality), unlocking is a
/// local operation with no network round-trip — the DSCP-2 sub-250ms fast path.
pub trait DeliveryRelay {
    /// Sender hands the relay a ciphertext envelope for later prefetch.
    fn post(&self, envelope: EncryptedEnvelope);
    /// Recipient pulls all locked ciphertext addressed to its mailbox tag.
    fn prefetch(&self, recipient_mailbox_tag: &[u8; 32]) -> Vec<EncryptedEnvelope>;
}

/// In-process store-and-forward relay: a ciphertext mailbox keyed by the recipient's
/// (unlinkable, random) mailbox tag. The real `wcaht-seal-sdk` relay speaks the same
/// contract over HTTP.
#[derive(Default)]
pub struct MockDeliveryRelay {
    by_tag: std::sync::Mutex<HashMap<[u8; 32], Vec<EncryptedEnvelope>>>,
}

impl MockDeliveryRelay {
    pub fn new() -> Self {
        Self::default()
    }
}

impl DeliveryRelay for MockDeliveryRelay {
    fn post(&self, envelope: EncryptedEnvelope) {
        self.by_tag.lock().unwrap().entry(envelope.recipient_mailbox_tag).or_default().push(envelope);
    }
    fn prefetch(&self, recipient_mailbox_tag: &[u8; 32]) -> Vec<EncryptedEnvelope> {
        self.by_tag.lock().unwrap().get(recipient_mailbox_tag).cloned().unwrap_or_default()
    }
}

// ───────────────────────────── sender flow ──────────────────────────────────

/// Everything produced for one sealed item, routed to the three networks.
pub struct SealedItem {
    pub seal_id: [u8; 32],
    /// -> delivery relay
    pub envelope: EncryptedEnvelope,
    /// -> WCAHT chain
    pub signed_leaf: SignedLeaf,
    /// -> seal gateways (one per gateway)
    pub share_envelopes: Vec<KeyShareEnvelope>,
}

// ─────────────────────────────── media items ────────────────────────────────
//
// An image or video is NEVER carried in the envelope, and never lands anywhere readable.
// The envelope carries an encrypted [`MediaManifest`]; the pixels live in separately
// encrypted chunks that the relay stores as opaque blobs. The chain sees only
// `manifest_root` — 32 bytes with no filename, mime, size, or thumbnail in it.
//
//   sender ──encrypted chunks───────────────▶ relay    (opaque; unopenable without shares)
//          ──encrypted manifest (envelope)──▶ relay    (mime/size/dimensions all sealed)
//          ──signed leaf (manifest_root)────▶ WCAHT    (32 bytes)
//          ──key shares────────────────────▶ gateways (encrypted to the recipient device)
//
// Every chunk key is a KDF subkey of `K_content`, which is Shamir-split across gateways.
// So a recipient can pre-download an entire video that remains cryptographically
// unopenable until the release gate opens — the same gate that drives time-reveal and
// time-destroy for text.

/// Default chunk size (spec §10.3: 1 MiB, configurable after real-device testing).
pub const DEFAULT_CHUNK_SIZE: u32 = 1024 * 1024;

/// Bound on a decoded manifest, so untrusted input cannot ask us to allocate absurdly.
const MAX_CHUNK_COUNT: u32 = 100_000;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
    Image,
    Audio,
    File,
    Video,
}

/// What may be rendered BEFORE the release gate opens (spec §10.3). A locked preview is
/// only ever the blurred bytes sealed inside the manifest — a readable thumbnail is never
/// uploaded as its own fetchable object.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewPolicy {
    None,
    LockedBlur,
}

/// Spec §8.2. Encrypted before upload: mime, size, dimensions and the chunk list are all
/// sealed inside the envelope, so neither the relay nor the chain learns them. Only
/// `manifest_root` — the merkle root over the chunk hashes — is public, in the leaf.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MediaManifest {
    pub protocol_version: u16,
    pub seal_id: [u8; 32],
    pub content_kind: ContentKind,
    pub mime_type: String,
    pub plaintext_size: u64,
    pub chunk_size: u32,
    pub chunk_count: u32,
    pub ciphertext_chunk_hashes: Vec<[u8; 32]>,
    pub preview_policy: PreviewPolicy,
    /// Text the sender attached to the photo/video. Sealed with the rest of the manifest, so
    /// neither the relay nor the chain ever sees it — a caption is as private as the pixels.
    pub caption: String,
    /// Locally blurred preview bytes, encrypted with the rest of the manifest.
    /// Empty unless `preview_policy` is `LockedBlur`.
    pub preview: Vec<u8>,
    /// Display hints, 0 when unknown. Encrypted along with everything else here.
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
}

/// One encrypted chunk as it travels to, and rests on, the relay.
/// `bytes` is `nonce || ciphertext`; `ciphertext_hash` covers exactly those bytes.
#[derive(Clone, Debug)]
pub struct MediaChunk {
    pub index: u32,
    pub ciphertext_hash: [u8; 32],
    pub bytes: Vec<u8>,
}

/// A sealed media item: the three routes of [`SealedItem`], plus the opaque chunks to upload.
pub struct SealedMedia {
    pub item: SealedItem,
    pub chunks: Vec<MediaChunk>,
}

/// Merkle root over the chunk hashes → the leaf's `manifest_root`.
///
/// Leaves and interior nodes hash under different domains, so an interior node can never be
/// reinterpreted as a leaf. An odd node is PROMOTED rather than duplicated, avoiding the
/// duplicate-node root collision that node-doubling designs suffer from.
fn manifest_root_of(chunk_hashes: &[[u8; 32]]) -> [u8; 32] {
    if chunk_hashes.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<[u8; 32]> =
        chunk_hashes.iter().map(|h| sc::hash("DSCP-1/manifest-leaf", h)).collect();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            if pair.len() == 2 {
                let mut buf = [0u8; 64];
                buf[..32].copy_from_slice(&pair[0]);
                buf[32..].copy_from_slice(&pair[1]);
                next.push(sc::hash("DSCP-1/manifest-node", &buf));
            } else {
                next.push(pair[0]); // promote, never duplicate
            }
        }
        level = next;
    }
    level[0]
}

// ─────────────────────── seal batches (spec §8.5, §9.2) ────────────────────
//
// A capsule per message does NOT mean a transaction per message. Many seal leaves are
// batched under one `SEAL_ROOT`; each message then carries a merkle path proving THIS leaf
// is inside THAT finalised root. One transaction anchors thousands of capsules, and the
// recipient still verifies its own message rather than trusting the batcher.

/// Merkle root over a batch of seal leaf hashes.
///
/// Leaves and interior nodes hash under different domains so an interior node can never be
/// replayed as a leaf. An odd node is PROMOTED rather than duplicated — node-doubling lets
/// two different leaf sets produce the same root.
pub fn seal_batch_root(leaf_hashes: &[[u8; 32]]) -> [u8; 32] {
    if leaf_hashes.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<[u8; 32]> =
        leaf_hashes.iter().map(|h| sc::hash("DSCP-1/batch-leaf", h)).collect();
    while level.len() > 1 {
        level = pair_up(&level);
    }
    level[0]
}

fn pair_up(level: &[[u8; 32]]) -> Vec<[u8; 32]> {
    let mut next = Vec::with_capacity(level.len().div_ceil(2));
    for pair in level.chunks(2) {
        if pair.len() == 2 {
            let mut buf = [0u8; 64];
            buf[..32].copy_from_slice(&pair[0]);
            buf[32..].copy_from_slice(&pair[1]);
            next.push(sc::hash("DSCP-1/batch-node", &buf));
        } else {
            next.push(pair[0]); // promote, never duplicate
        }
    }
    next
}

/// The sibling path proving `index` is in the batch. Bottom-up; a promoted (odd) node
/// contributes no sibling at that level.
pub fn seal_batch_proof(leaf_hashes: &[[u8; 32]], index: usize) -> Option<Vec<[u8; 32]>> {
    if index >= leaf_hashes.len() {
        return None;
    }
    let mut level: Vec<[u8; 32]> =
        leaf_hashes.iter().map(|h| sc::hash("DSCP-1/batch-leaf", h)).collect();
    let mut i = index;
    let mut path = Vec::new();
    while level.len() > 1 {
        let sibling = if i % 2 == 0 { i + 1 } else { i - 1 };
        if sibling < level.len() {
            path.push(level[sibling]);
        }
        // else: this node was promoted — nothing to combine with at this level
        level = pair_up(&level);
        i /= 2;
    }
    Some(path)
}

/// Recompute the batch root from a leaf + its path and check it matches.
///
/// This is what makes a batched capsule verifiable BY THE RECIPIENT: the batcher cannot
/// claim a message is in a root it is not in, because the arithmetic has to come out.
///
/// `leaf_count` is required, not decorative. Because an odd node is PROMOTED rather than
/// duplicated, some levels contribute no sibling — so the path length alone does not tell you
/// which level you are on. Replaying the level widths from the batch size is what keeps the
/// left/right decision in step with how the root was built. (Getting this wrong verified
/// every leaf except the promoted ones, which is exactly the sort of bug that would only
/// show up on odd-sized batches in production.)
pub fn verify_seal_inclusion(
    leaf_hash: &[u8; 32],
    merkle_path: &[[u8; 32]],
    leaf_index: u32,
    leaf_count: u32,
    root: &[u8; 32],
) -> bool {
    if leaf_count == 0 || leaf_index >= leaf_count {
        return false;
    }
    let mut node = sc::hash("DSCP-1/batch-leaf", leaf_hash);
    let mut i = leaf_index as usize;
    let mut width = leaf_count as usize;
    let mut used = 0usize;
    while width > 1 {
        let sibling_idx = if i % 2 == 0 { i + 1 } else { i.wrapping_sub(1) };
        if sibling_idx < width {
            let Some(sibling) = merkle_path.get(used) else { return false };
            used += 1;
            let mut buf = [0u8; 64];
            if i % 2 == 0 {
                buf[..32].copy_from_slice(&node);
                buf[32..].copy_from_slice(sibling);
            } else {
                buf[..32].copy_from_slice(sibling);
                buf[32..].copy_from_slice(&node);
            }
            node = sc::hash("DSCP-1/batch-node", &buf);
        }
        // else: promoted this level — the node carries up unchanged, no sibling consumed
        i /= 2;
        width = width.div_ceil(2);
    }
    // a path with unused siblings is not the path for this leaf
    used == merkle_path.len() && node == *root
}

/// Per-chunk key: a KDF subkey of `K_content`, distinct for every (seal, index).
/// Distinct keys mean the random per-chunk nonce can never collide into a reuse.
fn chunk_key(k_content: &[u8; 32], seal_id: &[u8; 32], index: u32) -> [u8; 32] {
    let mut material = Vec::with_capacity(68);
    material.extend_from_slice(k_content);
    material.extend_from_slice(seal_id);
    material.extend_from_slice(&index.to_le_bytes());
    sc::derive_key("DSCP-1/media-chunk-key/v1", &material)
}

/// AAD binding a chunk to its seal AND its position, so chunks cannot be reordered or
/// transplanted between items. Computed before the leaf exists, so it cannot use the leaf hash.
fn chunk_aad(seal_id: &[u8; 32], index: u32, chunk_count: u32) -> [u8; 32] {
    let mut ctx = Vec::with_capacity(48);
    ctx.extend_from_slice(&CHAIN_ID.to_le_bytes());
    ctx.extend_from_slice(seal_id);
    ctx.extend_from_slice(&index.to_le_bytes());
    ctx.extend_from_slice(&chunk_count.to_le_bytes());
    sc::hash("DSCP-1/media-chunk-aad", &ctx)
}

/// Encrypt `plaintext` into positionally-bound chunks.
fn encrypt_chunks(
    plaintext: &[u8],
    k_content: &[u8; 32],
    seal_id: &[u8; 32],
    chunk_size: u32,
) -> Result<Vec<MediaChunk>> {
    if chunk_size == 0 {
        return Err(anyhow!("chunk_size must be non-zero"));
    }
    let count = plaintext.len().div_ceil(chunk_size as usize);
    if count == 0 {
        return Err(anyhow!("refusing to seal empty media"));
    }
    if count as u64 > MAX_CHUNK_COUNT as u64 {
        return Err(anyhow!("media too large: {count} chunks exceeds cap {MAX_CHUNK_COUNT}"));
    }
    let chunk_count = count as u32;
    let mut out = Vec::with_capacity(count);
    for (i, piece) in plaintext.chunks(chunk_size as usize).enumerate() {
        let index = i as u32;
        let key = chunk_key(k_content, seal_id, index);
        let aad = chunk_aad(seal_id, index, chunk_count);
        let (ct, nonce) = sc::aead_seal(&key, &aad, piece)?;
        let mut bytes = Vec::with_capacity(nonce.len() + ct.len());
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&ct);
        let ciphertext_hash = sc::hash("DSCP-1/media-chunk", &bytes);
        out.push(MediaChunk { index, ciphertext_hash, bytes });
    }
    Ok(out)
}

/// Seal an image, video, voice note, or document (spec §8.2, §10.3, §11).
///
/// The bytes are chunked and encrypted locally; the envelope carries only the encrypted
/// manifest. `preview` must ALREADY be blurred/downscaled by the caller — it is sealed
/// inside the manifest and is never separately fetchable.
#[allow(clippy::too_many_arguments)]
pub fn seal_media_with_mode(
    plaintext: &[u8],
    kind: ContentKind,
    mime_type: &str,
    caption: &str,
    preview: &[u8],
    preview_policy: PreviewPolicy,
    dimensions: (u32, u32),
    duration_ms: u64,
    chunk_size: u32,
    sender_identity: &sc::SignId,
    sender_device: &sc::SignId,
    recipient_device_pub: &[u8; 32],
    recipient_mailbox_tag: [u8; 32],
    gateway_ids: &[[u8; 32]],
    threshold_t: u8,
    current_slot: u64,
    ttl_slots: u64,
    mode: SealMode,
    reveal_at_unix: u64,
    destroy_at_unix: u64,
) -> Result<SealedMedia> {
    let seal_id = sc::random_32();
    let k_content = sc::random_32();

    let chunks = encrypt_chunks(plaintext, &k_content, &seal_id, chunk_size)?;
    let chunk_hashes: Vec<[u8; 32]> = chunks.iter().map(|c| c.ciphertext_hash).collect();
    let manifest_root = manifest_root_of(&chunk_hashes);

    let manifest = MediaManifest {
        protocol_version: PROTOCOL_VERSION,
        seal_id,
        content_kind: kind,
        mime_type: mime_type.to_string(),
        plaintext_size: plaintext.len() as u64,
        chunk_size,
        chunk_count: chunks.len() as u32,
        ciphertext_chunk_hashes: chunk_hashes,
        preview_policy,
        caption: caption.to_string(),
        preview: if preview_policy == PreviewPolicy::LockedBlur { preview.to_vec() } else { Vec::new() },
        width: dimensions.0,
        height: dimensions.1,
        duration_ms,
    };
    let manifest_bytes = bincode::serialize(&manifest)?;

    let item = seal_payload(
        &manifest_bytes,
        ContentType::Media,
        manifest_root,
        seal_id,
        k_content,
        sender_identity,
        sender_device,
        recipient_device_pub,
        recipient_mailbox_tag,
        gateway_ids,
        threshold_t,
        current_slot,
        ttl_slots,
        mode,
        reveal_at_unix,
        destroy_at_unix,
    )?;
    Ok(SealedMedia { item, chunks })
}

/// Seal a text item: encrypt locally, split the content key t-of-n, encrypt each
/// share to the recipient device, and sign a leaf committing to it all (spec §6.2).
///
/// StrictSeal convenience wrapper (DSCP-1 behaviour: opens only at L1 finality).
#[allow(clippy::too_many_arguments)]
pub fn seal_text(
    plaintext: &[u8],
    sender_identity: &sc::SignId,
    sender_device: &sc::SignId,
    recipient_device_pub: &[u8; 32],
    recipient_mailbox_tag: [u8; 32],
    gateway_ids: &[[u8; 32]],
    threshold_t: u8,
    current_slot: u64,
    ttl_slots: u64,
) -> Result<SealedItem> {
    seal_text_with_mode(
        plaintext,
        sender_identity,
        sender_device,
        recipient_device_pub,
        recipient_mailbox_tag,
        gateway_ids,
        threshold_t,
        current_slot,
        ttl_slots,
        SealMode::StrictSeal,
        0,
        0,
    )
}

/// Seal a text item in an explicit [`SealMode`] (StrictSeal vault or FastSeal fast-path).
#[allow(clippy::too_many_arguments)]
pub fn seal_text_with_mode(
    plaintext: &[u8],
    sender_identity: &sc::SignId,
    sender_device: &sc::SignId,
    recipient_device_pub: &[u8; 32],
    recipient_mailbox_tag: [u8; 32],
    gateway_ids: &[[u8; 32]],
    threshold_t: u8,
    current_slot: u64,
    ttl_slots: u64,
    mode: SealMode,
    reveal_at_unix: u64,
    destroy_at_unix: u64,
) -> Result<SealedItem> {
    // Text has no chunk list, so `manifest_root` stays zero (spec §8.3).
    seal_payload(
        plaintext,
        ContentType::Text,
        [0u8; 32],
        sc::random_32(),
        sc::random_32(),
        sender_identity,
        sender_device,
        recipient_device_pub,
        recipient_mailbox_tag,
        gateway_ids,
        threshold_t,
        current_slot,
        ttl_slots,
        mode,
        reveal_at_unix,
        destroy_at_unix,
    )
}

/// The sealing core, shared by every content type. `payload` is what the envelope carries:
/// the message itself for text, the encrypted [`MediaManifest`] for media. `seal_id` and
/// `k_content` are passed in because media must chunk-encrypt with them before the leaf exists.
#[allow(clippy::too_many_arguments)]
fn seal_payload(
    payload: &[u8],
    content_type: ContentType,
    manifest_root: [u8; 32],
    seal_id: [u8; 32],
    k_content: [u8; 32],
    sender_identity: &sc::SignId,
    sender_device: &sc::SignId,
    recipient_device_pub: &[u8; 32],
    recipient_mailbox_tag: [u8; 32],
    gateway_ids: &[[u8; 32]],
    threshold_t: u8,
    current_slot: u64,
    ttl_slots: u64,
    mode: SealMode,
    reveal_at_unix: u64,
    destroy_at_unix: u64,
) -> Result<SealedItem> {
    let n = gateway_ids.len() as u8;
    if threshold_t == 0 || threshold_t > n {
        return Err(anyhow!("invalid threshold t={threshold_t} n={n}"));
    }
    let plaintext = payload;

    let recip_dev_commit = device_commitment(recipient_device_pub);
    let sender_id_commit = identity_commitment(&sender_identity.public());
    let sender_dev_commit = device_commitment(&sender_device.public());
    let expires_at_slot = current_slot.saturating_add(ttl_slots);

    let aad = compute_aad(
        CHAIN_ID,
        &seal_id,
        content_type,
        &recip_dev_commit,
        &sender_id_commit,
        expires_at_slot,
    );

    // Encrypt content locally.
    let (ct, nonce) = sc::aead_seal(&k_content, &aad, plaintext)?;
    let ct_hash = ciphertext_hash(&nonce, &ct);

    // Split the content key and encrypt each share to the recipient device.
    let shares = sc::shamir_split(&k_content, threshold_t, n);
    let mut share_envelopes = Vec::with_capacity(n as usize);
    let mut commitments = Vec::new();
    for (i, share) in shares.iter().enumerate() {
        let share_commitment = sc::hash("DSCP-1/share", share);
        commitments.push(share_commitment);
        let boxed = sc::hpke_seal(recipient_device_pub, &aad, share)?;
        share_envelopes.push(KeyShareEnvelope {
            seal_id,
            gateway_id: gateway_ids[i],
            share_index: i as u8,
            recipient_device_commitment: recip_dev_commit,
            encrypted_share: boxed.into(),
            share_commitment,
            leaf_hash: [0u8; 32], // filled after the leaf is built
            expires_at_slot,
        });
    }
    let mut kscr_input = Vec::new();
    for c in &commitments {
        kscr_input.extend_from_slice(c);
    }
    let key_share_commitment_root = sc::hash("DSCP-1/kscr", &kscr_input);

    let leaf = SealLeaf {
        protocol_version: PROTOCOL_VERSION,
        chain_id: CHAIN_ID,
        seal_id,
        content_type,
        ciphertext_hash: ct_hash,
        manifest_root,
        recipient_device_commitment: recip_dev_commit,
        sender_identity_commitment: sender_id_commit,
        sender_device_commitment: sender_dev_commit,
        key_share_commitment_root,
        threshold_t,
        threshold_n: n,
        not_before_finalized_slot: current_slot,
        expires_at_slot,
        reveal_at_unix,
        destroy_at_unix,
        flags: 0,
        mode,
    };
    let leaf_hash = leaf.leaf_hash();
    for env in &mut share_envelopes {
        env.leaf_hash = leaf_hash;
    }
    let sender_signature = sender_identity.sign(&leaf.canonical_bytes()).to_vec();

    Ok(SealedItem {
        seal_id,
        envelope: EncryptedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            seal_id,
            recipient_mailbox_tag,
            aad_commitment: aad,
            nonce,
            ciphertext: ct,
        },
        signed_leaf: SignedLeaf { leaf, sender_signature },
        share_envelopes,
    })
}

// ──────────────────────────── recipient flow ────────────────────────────────

/// Client-visible item state (subset of spec §13.5), computed from the routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemState {
    DeliveredLocked,
    SealFinalising,
    KeySharesReleasing,
    Unlocked,
    Expired,
    Revoked,
    FailedFinal,
}

#[derive(Debug)]
pub enum OpenOutcome {
    /// Cannot open yet — the app must show a locked state (spec §2).
    Locked { state: ItemState, reason: String },
    /// All five conditions met; plaintext recovered locally.
    Opened { plaintext: Vec<u8> },
    /// Permanently invalid (tamper, wrong recipient, replay, wrong chain, …).
    Rejected { reason: String },
}

/// Outcome of the mode-aware release gate.
enum Gate {
    Open,
    Locked { state: ItemState, reason: String },
    Rejected { reason: String },
}

/// Count valid, distinct-gateway pre-confirmations for `leaf`. A pre-conf only counts
/// if it is signed by a gateway that actually holds a share for this seal, commits to
/// THIS exact leaf hash (anti-equivocation/anti-downgrade), is bound to the seal expiry,
/// and has not itself expired.
fn count_valid_preconfs(
    leaf: &SealLeaf,
    pre_confirmations: &[PreConfirmation],
    authorized_gateways: &HashSet<[u8; 32]>,
    current_slot: u64,
) -> usize {
    let leaf_hash = leaf.leaf_hash();
    let mut distinct = HashSet::new();
    for pc in pre_confirmations {
        if pc.chain_id != leaf.chain_id
            || pc.seal_id != leaf.seal_id
            || pc.leaf_hash != leaf_hash
            || pc.expiry_slot != leaf.expires_at_slot
            || current_slot > pc.expiry_slot
            || !authorized_gateways.contains(&pc.gateway_id)
            || !pc.verify()
        {
            continue;
        }
        distinct.insert(pc.gateway_id);
    }
    distinct.len()
}

/// The DSCP-2 release gate. L1 finality opens either mode. Absent finality, StrictSeal
/// stays locked (vault), while FastSeal opens on a `threshold_t` quorum of valid gateway
/// pre-confirmations.
fn release_gate(
    leaf: &SealLeaf,
    pre_confirmations: &[PreConfirmation],
    collected_shares: &[KeyShareEnvelope],
    chain: &impl SealChain,
) -> Gate {
    // Strongest path: hard L1 finality (accepted in both modes).
    if let Some(proof) = chain.proof(&leaf.seal_id) {
        if proof.finalized_slot >= leaf.not_before_finalized_slot {
            // The proof must be for OUR leaf, and the leaf must actually reconstruct the
            // finalised root. An empty path means a one-leaf batch, where the root IS the leaf.
            if proof.leaf_hash != leaf.leaf_hash() {
                return Gate::Rejected { reason: "seal proof is for a different leaf".into() };
            }
            if !verify_seal_inclusion(
                &proof.leaf_hash,
                &proof.merkle_path,
                proof.leaf_index,
                proof.leaf_count,
                &proof.seal_root,
            ) {
                return Gate::Rejected { reason: "seal inclusion proof does not reconstruct the root".into() };
            }
            return Gate::Open;
        }
    }

    match leaf.mode {
        SealMode::StrictSeal => Gate::Locked {
            state: ItemState::SealFinalising,
            reason: "waiting for WCAHT finality (StrictSeal vault mode)".into(),
        },
        SealMode::FastSeal => {
            let authorized: HashSet<[u8; 32]> = collected_shares
                .iter()
                .filter(|s| s.seal_id == leaf.seal_id)
                .map(|s| s.gateway_id)
                .collect();
            let valid = count_valid_preconfs(leaf, pre_confirmations, &authorized, chain.slot());
            if valid as u8 >= leaf.threshold_t {
                Gate::Open
            } else {
                Gate::Locked {
                    state: ItemState::SealFinalising,
                    reason: format!(
                        "fast-path: {}/{} gateway pre-confirmations (or awaiting L1 finality)",
                        valid, leaf.threshold_t
                    ),
                }
            }
        }
    }
}

/// StrictSeal / DSCP-1 entry point: supplies no pre-confirmations, so a FastSeal item
/// opened through here still requires finality. Use [`try_open_dscp2`] to pass gateway
/// pre-confirmations and enable the fast path.
pub fn try_open(
    envelope: &EncryptedEnvelope,
    signed_leaf: &SignedLeaf,
    sender_identity_pub: &[u8; 32],
    recipient_device: &sc::DeviceKey,
    collected_shares: &[KeyShareEnvelope],
    chain: &impl SealChain,
) -> OpenOutcome {
    try_open_dscp2(
        envelope,
        signed_leaf,
        sender_identity_pub,
        recipient_device,
        collected_shares,
        &[],
        chain,
    )
}

/// The heart of the promise: verify all conditions, then — and only then — reconstruct
/// the key and decrypt. Honours the leaf's [`SealMode`]: StrictSeal opens only at L1
/// finality; FastSeal also opens on a quorum of valid gateway `pre_confirmations`.
#[allow(clippy::too_many_arguments)]
pub fn try_open_dscp2(
    envelope: &EncryptedEnvelope,
    signed_leaf: &SignedLeaf,
    sender_identity_pub: &[u8; 32],
    recipient_device: &sc::DeviceKey,
    collected_shares: &[KeyShareEnvelope],
    pre_confirmations: &[PreConfirmation],
    chain: &impl SealChain,
) -> OpenOutcome {
    let mut discard = None;
    try_open_inner(
        envelope,
        signed_leaf,
        sender_identity_pub,
        recipient_device,
        collected_shares,
        pre_confirmations,
        chain,
        &mut discard,
    )
}

/// Shared open path. `out_key` receives the reconstructed content key on success — media
/// needs it to derive per-chunk keys, text does not. It is an out-parameter rather than a
/// return value so that every early-return verification check below stays untouched.
#[allow(clippy::too_many_arguments)]
fn try_open_inner(
    envelope: &EncryptedEnvelope,
    signed_leaf: &SignedLeaf,
    sender_identity_pub: &[u8; 32],
    recipient_device: &sc::DeviceKey,
    collected_shares: &[KeyShareEnvelope],
    pre_confirmations: &[PreConfirmation],
    chain: &impl SealChain,
    out_key: &mut Option<[u8; 32]>,
) -> OpenOutcome {
    let leaf = &signed_leaf.leaf;

    // (5a) version / chain binding.
    if leaf.protocol_version != PROTOCOL_VERSION {
        return OpenOutcome::Rejected { reason: "unknown/downgraded protocol version".into() };
    }
    if leaf.chain_id != CHAIN_ID {
        return OpenOutcome::Rejected { reason: "wrong chain id".into() };
    }
    // sender signature over the leaf.
    let Ok(sig): std::result::Result<[u8; 64], _> = signed_leaf.sender_signature.as_slice().try_into()
    else {
        return OpenOutcome::Rejected { reason: "malformed sender signature".into() };
    };
    if leaf.sender_identity_commitment != identity_commitment(sender_identity_pub) {
        return OpenOutcome::Rejected { reason: "sender identity commitment mismatch".into() };
    }
    if !sc::verify_sig(sender_identity_pub, &leaf.canonical_bytes(), &sig) {
        return OpenOutcome::Rejected { reason: "invalid sender signature".into() };
    }
    // this item must be addressed to THIS device (spec §18: wrong recipient).
    if leaf.recipient_device_commitment != device_commitment(&recipient_device.public()) {
        return OpenOutcome::Rejected { reason: "item is addressed to a different device".into() };
    }
    // envelope must match the leaf.
    if envelope.seal_id != leaf.seal_id {
        return OpenOutcome::Rejected { reason: "envelope/leaf seal_id mismatch".into() };
    }

    // (5c) Timelock window — read from the LEAF, which the sender signed and whose hash is
    // anchored on-chain. Enforcing it here rather than in the app is what makes it more than
    // an overlay: a relay cannot move the deadline without breaking the signature, and the
    // gateways withholding shares means a patched client still has no key to use.
    let now = now_unix();
    if leaf.destroy_at_unix > 0 && now >= leaf.destroy_at_unix {
        return OpenOutcome::Locked {
            state: ItemState::Expired,
            reason: "self-destructed — the window has closed".into(),
        };
    }
    if leaf.reveal_at_unix > 0 && now < leaf.reveal_at_unix {
        return OpenOutcome::Locked {
            state: ItemState::DeliveredLocked,
            reason: format!("timelocked until {}", leaf.reveal_at_unix),
        };
    }

    // (5b) revocation / expiry.
    match chain.status(&leaf.seal_id) {
        SealStatus::Revoked => return OpenOutcome::Locked { state: ItemState::Revoked, reason: "seal revoked".into() },
        SealStatus::Invalid | SealStatus::Expired => {
            return OpenOutcome::Locked { state: ItemState::FailedFinal, reason: "seal invalid/expired".into() }
        }
        _ => {}
    }
    if chain.slot() > leaf.expires_at_slot {
        return OpenOutcome::Locked { state: ItemState::Expired, reason: "item expired".into() };
    }

    // (4) Release gate — mode-aware (spec DSCP-2 §4). StrictSeal opens only at hard L1
    // finality; FastSeal also opens on a quorum of slashable gateway pre-confirmations.
    match release_gate(leaf, pre_confirmations, collected_shares, chain) {
        Gate::Open => {}
        Gate::Locked { state, reason } => return OpenOutcome::Locked { state, reason },
        Gate::Rejected { reason } => return OpenOutcome::Rejected { reason },
    }

    // (2) ciphertext hash must match the sealed commitment.
    if ciphertext_hash(&envelope.nonce, &envelope.ciphertext) != leaf.ciphertext_hash {
        return OpenOutcome::Rejected { reason: "ciphertext hash mismatch (content altered)".into() };
    }

    // (3) enough key shares addressed to this device.
    let aad = leaf_aad(leaf);
    let mut recovered_shares: Vec<Vec<u8>> = Vec::new();
    let mut used_indices = HashSet::new();
    for env in collected_shares {
        if env.seal_id != leaf.seal_id || env.recipient_device_commitment != leaf.recipient_device_commitment {
            continue;
        }
        if !used_indices.insert(env.share_index) {
            continue; // ignore duplicates
        }
        match sc::hpke_open(recipient_device, &env.encrypted_share.to_hpke(), &aad) {
            Ok(share) => {
                if sc::hash("DSCP-1/share", &share) != env.share_commitment {
                    continue; // share does not match its commitment
                }
                recovered_shares.push(share);
            }
            Err(_) => continue, // not decryptable by this device
        }
    }
    if (recovered_shares.len() as u8) < leaf.threshold_t {
        return OpenOutcome::Locked {
            state: ItemState::KeySharesReleasing,
            reason: format!(
                "have {}/{} required key shares (gateways release only after finality)",
                recovered_shares.len(),
                leaf.threshold_t
            ),
        };
    }

    // Reconstruct the content key and decrypt locally.
    let k_content = match sc::shamir_combine(leaf.threshold_t, &recovered_shares[..leaf.threshold_t as usize]) {
        Ok(k) => k,
        Err(e) => return OpenOutcome::Rejected { reason: format!("key reconstruction failed: {e}") },
    };
    let k: [u8; 32] = match k_content.as_slice().try_into() {
        Ok(k) => k,
        Err(_) => return OpenOutcome::Rejected { reason: "reconstructed key wrong length".into() },
    };
    match sc::aead_open(&k, &envelope.nonce, &aad, &envelope.ciphertext) {
        Ok(plaintext) => {
            *out_key = Some(k);
            OpenOutcome::Opened { plaintext }
        }
        Err(_) => OpenOutcome::Rejected { reason: "content authentication failed".into() },
    }
}

// ──────────────────────── recipient flow: media ─────────────────────────────

/// Result of opening a media seal.
pub enum MediaOutcome {
    /// The gate has not opened, or the item is not yet fully verifiable.
    Locked { state: ItemState, reason: String },
    /// Manifest recovered. Chunks may now be fetched and decrypted through the opener.
    Opened { manifest: MediaManifest, opener: MediaOpener },
    Rejected { reason: String },
}

/// Holds the reconstructed content key for one media item so chunks can be decrypted
/// on demand. The key never leaves this struct, and is zeroised on drop.
pub struct MediaOpener {
    seal_id: [u8; 32],
    chunk_count: u32,
    chunk_hashes: Vec<[u8; 32]>,
    k_content: [u8; 32],
}

impl Drop for MediaOpener {
    fn drop(&mut self) {
        self.k_content.zeroize();
    }
}

impl MediaOpener {
    /// Decrypt one chunk fetched from the relay. `blob` is exactly the bytes the relay
    /// served (`nonce || ciphertext`).
    ///
    /// The chunk hash is checked against the manifest FIRST, so a relay that alters or
    /// substitutes a blob is caught before any decryption is attempted — and the AEAD tag
    /// catches it again afterwards.
    pub fn decrypt_chunk(&self, index: u32, blob: &[u8]) -> Result<Vec<u8>> {
        if index >= self.chunk_count {
            return Err(anyhow!("chunk index {index} out of range (count {})", self.chunk_count));
        }
        if sc::hash("DSCP-1/media-chunk", blob) != self.chunk_hashes[index as usize] {
            return Err(anyhow!("chunk {index} does not match its manifest hash (content altered)"));
        }
        if blob.len() < sc::NONCE_LEN {
            return Err(anyhow!("chunk {index} truncated"));
        }
        let (nonce_bytes, ct) = blob.split_at(sc::NONCE_LEN);
        let nonce: [u8; sc::NONCE_LEN] = nonce_bytes.try_into().expect("checked length above");
        let key = chunk_key(&self.k_content, &self.seal_id, index);
        let aad = chunk_aad(&self.seal_id, index, self.chunk_count);
        sc::aead_open(&key, &nonce, &aad, ct)
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunk_count
    }
}

/// StrictSeal media entry point (no pre-confirmations → requires L1 finality).
pub fn try_open_media(
    envelope: &EncryptedEnvelope,
    signed_leaf: &SignedLeaf,
    sender_identity_pub: &[u8; 32],
    recipient_device: &sc::DeviceKey,
    collected_shares: &[KeyShareEnvelope],
    chain: &impl SealChain,
) -> MediaOutcome {
    try_open_media_dscp2(envelope, signed_leaf, sender_identity_pub, recipient_device, collected_shares, &[], chain)
}

/// Open a media seal: run the same five-condition gate as text, then recover the manifest
/// and bind it to the leaf. Returns an opener for the chunks.
///
/// The recipient may hold every chunk already — they are useless until this succeeds.
#[allow(clippy::too_many_arguments)]
pub fn try_open_media_dscp2(
    envelope: &EncryptedEnvelope,
    signed_leaf: &SignedLeaf,
    sender_identity_pub: &[u8; 32],
    recipient_device: &sc::DeviceKey,
    collected_shares: &[KeyShareEnvelope],
    pre_confirmations: &[PreConfirmation],
    chain: &impl SealChain,
) -> MediaOutcome {
    let leaf = &signed_leaf.leaf;
    if leaf.content_type != ContentType::Media {
        return MediaOutcome::Rejected { reason: "leaf is not a media item".into() };
    }
    let mut key: Option<[u8; 32]> = None;
    let manifest_bytes = match try_open_inner(
        envelope,
        signed_leaf,
        sender_identity_pub,
        recipient_device,
        collected_shares,
        pre_confirmations,
        chain,
        &mut key,
    ) {
        OpenOutcome::Opened { plaintext } => plaintext,
        OpenOutcome::Locked { state, reason } => return MediaOutcome::Locked { state, reason },
        OpenOutcome::Rejected { reason } => return MediaOutcome::Rejected { reason },
    };
    let Some(k_content) = key else {
        return MediaOutcome::Rejected { reason: "content key unavailable after open".into() };
    };

    let manifest: MediaManifest = match bincode::deserialize(&manifest_bytes) {
        Ok(m) => m,
        Err(e) => return MediaOutcome::Rejected { reason: format!("malformed media manifest: {e}") },
    };

    // The manifest is authenticated by the envelope AEAD, but it must also match the leaf
    // the sender signed — otherwise a manifest from one seal could be paired with another.
    if manifest.protocol_version != PROTOCOL_VERSION {
        return MediaOutcome::Rejected { reason: "unknown/downgraded manifest version".into() };
    }
    if manifest.seal_id != leaf.seal_id {
        return MediaOutcome::Rejected { reason: "manifest/leaf seal_id mismatch".into() };
    }
    if manifest.chunk_count as usize != manifest.ciphertext_chunk_hashes.len() {
        return MediaOutcome::Rejected { reason: "manifest chunk_count does not match hash list".into() };
    }
    if manifest.chunk_count == 0 || manifest.chunk_count > MAX_CHUNK_COUNT {
        return MediaOutcome::Rejected { reason: "manifest chunk_count out of range".into() };
    }
    // THE binding: the chunk list must reproduce the root the sender signed into the leaf.
    if manifest_root_of(&manifest.ciphertext_chunk_hashes) != leaf.manifest_root {
        return MediaOutcome::Rejected { reason: "manifest_root mismatch (chunk list altered)".into() };
    }

    let opener = MediaOpener {
        seal_id: leaf.seal_id,
        chunk_count: manifest.chunk_count,
        chunk_hashes: manifest.ciphertext_chunk_hashes.clone(),
        k_content,
    };
    MediaOutcome::Opened { manifest, opener }
}

#[cfg(test)]
mod tests;
