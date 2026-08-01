//! Threat-model + happy-path tests (spec §18/§19). These are the runnable proof
//! that the core promise holds: nothing opens before a finalised seal + enough
//! post-finality shares, and every tamper/misuse is rejected.

use super::*;
use seal_crypto as sc;

struct World {
    sender_identity: sc::SignId,
    sender_device: sc::SignId,
    bob_device: sc::DeviceKey,
    gateways: Vec<Gateway>,
    gw_ids: Vec<[u8; 32]>,
    chain: MockSealChain,
}

fn world() -> World {
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let gateways = gw_ids.iter().map(|id| Gateway::new(*id)).collect();
    World {
        sender_identity: sc::SignId::generate(),
        sender_device: sc::SignId::generate(),
        bob_device: sc::DeviceKey::generate(),
        gateways,
        gw_ids,
        chain: MockSealChain::new(100),
    }
}

impl World {
    fn seal(&self, text: &[u8], t: u8, ttl: u64) -> SealedItem {
        seal_text(
            text,
            &self.sender_identity,
            &self.sender_device,
            &self.bob_device.public(),
            sc::random_32(),
            &self.gw_ids,
            t,
            self.chain.slot(),
            ttl,
        )
        .expect("seal")
    }
    fn deposit_all(&mut self, item: &SealedItem) {
        for env in &item.share_envelopes {
            let gw = self.gateways.iter_mut().find(|g| g.id == env.gateway_id).unwrap();
            gw.deposit(env.clone());
        }
    }
    /// Collect whatever the gateways are willing to release right now (strict rule).
    fn collect(&self, seal_id: &[u8; 32]) -> Vec<KeyShareEnvelope> {
        self.gateways
            .iter()
            .filter_map(|g| g.request_share(seal_id, &self.chain))
            .collect()
    }
    fn open(&self, item: &SealedItem, shares: &[KeyShareEnvelope]) -> OpenOutcome {
        try_open(
            &item.envelope,
            &item.signed_leaf,
            &self.sender_identity.public(),
            &self.bob_device,
            shares,
            &self.chain,
        )
    }

    // ── DSCP-2 FastSeal helpers ──
    fn seal_mode(&self, text: &[u8], t: u8, ttl: u64, mode: SealMode) -> SealedItem {
        seal_text_with_mode(
            text,
            &self.sender_identity,
            &self.sender_device,
            &self.bob_device.public(),
            sc::random_32(),
            &self.gw_ids,
            t,
            self.chain.slot(),
            ttl,
            mode,
            0,
            0,
        )
        .expect("seal")
    }
    /// FastSeal share release: gateways hand shares over on their own sequencing.
    fn collect_fast(&self, seal_id: &[u8; 32]) -> Vec<KeyShareEnvelope> {
        self.gateways.iter().filter_map(|g| g.request_share_fast(seal_id)).collect()
    }
    fn preconfs(&self, seal_id: &[u8; 32], slot: u64) -> Vec<PreConfirmation> {
        self.gateways.iter().filter_map(|g| g.pre_confirm(seal_id, slot)).collect()
    }
    fn open_fast(
        &self,
        item: &SealedItem,
        shares: &[KeyShareEnvelope],
        preconfs: &[PreConfirmation],
    ) -> OpenOutcome {
        try_open_dscp2(
            &item.envelope,
            &item.signed_leaf,
            &self.sender_identity.public(),
            &self.bob_device,
            shares,
            preconfs,
            &self.chain,
        )
    }

    // ── media helpers ──
    fn seal_media(&self, bytes: &[u8], chunk_size: u32, t: u8, ttl: u64) -> SealedMedia {
        seal_media_with_mode(
            bytes,
            ContentKind::Image,
            "image/jpeg",
            "caption",
            b"blurred-preview",
            PreviewPolicy::LockedBlur,
            (1920, 1080),
            0,
            chunk_size,
            &self.sender_identity,
            &self.sender_device,
            &self.bob_device.public(),
            sc::random_32(),
            &self.gw_ids,
            t,
            self.chain.slot(),
            ttl,
            SealMode::StrictSeal,
            0,
            0,
        )
        .expect("seal media")
    }
    fn open_media(&self, m: &SealedMedia, shares: &[KeyShareEnvelope]) -> MediaOutcome {
        try_open_media(
            &m.item.envelope,
            &m.item.signed_leaf,
            &self.sender_identity.public(),
            &self.bob_device,
            shares,
            &self.chain,
        )
    }
    /// Reassemble the whole item from its chunks, as a real client would.
    fn reassemble(&self, opener: &MediaOpener, chunks: &[MediaChunk]) -> Vec<u8> {
        let mut out = Vec::new();
        for c in chunks {
            out.extend_from_slice(&opener.decrypt_chunk(c.index, &c.bytes).expect("chunk decrypt"));
        }
        out
    }
}

/// A world whose gateways carry signing identities, so they can issue slashable
/// pre-confirmations and take the FastSeal fast path.
fn fast_world() -> World {
    let signers: Vec<sc::SignId> = (0..3).map(|_| sc::SignId::generate()).collect();
    let gw_ids: Vec<[u8; 32]> = signers.iter().map(|s| s.public()).collect();
    let gateways = signers.into_iter().map(Gateway::with_identity).collect();
    World {
        sender_identity: sc::SignId::generate(),
        sender_device: sc::SignId::generate(),
        bob_device: sc::DeviceKey::generate(),
        gateways,
        gw_ids,
        chain: MockSealChain::new(100),
    }
}

#[test]
fn locked_until_finality_then_opens() {
    let mut w = world();
    let item = w.seal(b"the launch code is 42", 2, 1000);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();

    // Before finality: gateways release NOTHING, so the item stays locked.
    let shares = w.collect(&seal_id);
    assert!(shares.is_empty(), "no shares should be released before finality");
    match w.open(&item, &shares) {
        OpenOutcome::Locked { .. } => {}
        other => panic!("must be locked before finality, got {other:?}"),
    }

    // Finalise → gateways release → item opens with the exact plaintext.
    w.chain.finalize(&seal_id).unwrap();
    let shares = w.collect(&seal_id);
    assert_eq!(shares.len(), 3, "all gateways release after finality");
    match w.open(&item, &shares) {
        OpenOutcome::Opened { plaintext } => assert_eq!(plaintext, b"the launch code is 42"),
        other => panic!("must open after finality, got {other:?}"),
    }
}

#[test]
fn one_gateway_offline_still_opens() {
    // t=2 of n=3: a single gateway can fail and the item still opens (§18).
    let mut w = world();
    let item = w.seal(b"hi", 2, 1000);
    // deposit to only 2 of 3 gateways
    for env in item.share_envelopes.iter().take(2) {
        let gw = w.gateways.iter_mut().find(|g| g.id == env.gateway_id).unwrap();
        gw.deposit(env.clone());
    }
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&seal_id).unwrap();
    let shares = w.collect(&seal_id);
    assert_eq!(shares.len(), 2);
    assert!(matches!(w.open(&item, &shares), OpenOutcome::Opened { .. }));
}

#[test]
fn insufficient_shares_stay_locked_after_finality() {
    // Even AFTER finality, fewer than t shares (e.g. one leaked/early-released share)
    // cannot reconstruct the key — the item stays locked (§18 "one gateway releases early").
    let mut w = world();
    let item = w.seal(b"secret", 2, 1000);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&seal_id).unwrap();
    // hand the recipient exactly one share (t=2 required)
    let one = vec![item.share_envelopes[0].clone()];
    match w.open(&item, &one) {
        OpenOutcome::Locked { state: ItemState::KeySharesReleasing, .. } => {}
        other => panic!("one share must be insufficient, got {other:?}"),
    }
}

#[test]
fn wrong_recipient_cannot_open() {
    let mut w = world();
    let item = w.seal(b"private", 2, 1000);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&seal_id).unwrap();
    let shares = w.collect(&seal_id);

    // Mallory has her own device; the leaf is bound to Bob's device.
    let mallory = sc::DeviceKey::generate();
    let outcome = try_open(
        &item.envelope,
        &item.signed_leaf,
        &w.sender_identity.public(),
        &mallory,
        &shares,
        &w.chain,
    );
    assert!(matches!(outcome, OpenOutcome::Rejected { .. }), "wrong recipient rejected, got {outcome:?}");
}

#[test]
fn altered_ciphertext_rejected() {
    let mut w = world();
    let mut item = w.seal(b"do not change me", 2, 1000);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&seal_id).unwrap();
    let shares = w.collect(&seal_id);
    // flip one ciphertext byte after sealing
    item.envelope.ciphertext[0] ^= 0x01;
    assert!(matches!(w.open(&item, &shares), OpenOutcome::Rejected { .. }));
}

#[test]
fn replay_rejected_by_chain() {
    let mut w = world();
    let item = w.seal(b"once", 2, 1000);
    w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    let second = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public());
    assert!(second.is_err(), "same seal_id must not be accepted twice");
}

#[test]
fn wrong_chain_id_rejected() {
    let mut w = world();
    let mut item = w.seal(b"x", 2, 1000);
    item.signed_leaf.leaf.chain_id = 1; // not 7789
    let r = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public());
    assert!(r.is_err(), "wrong chain id must be rejected at submit");
}

#[test]
fn expired_item_stays_locked() {
    let mut w = world();
    let item = w.seal(b"time-boxed", 2, 5); // ttl = 5 slots
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.advance(10); // past expiry
    w.chain.finalize(&seal_id).unwrap();
    let shares = w.collect(&seal_id);
    match w.open(&item, &shares) {
        OpenOutcome::Locked { state: ItemState::Expired, .. } => {}
        other => panic!("expired item must be locked, got {other:?}"),
    }
}

#[test]
fn revoked_item_stays_locked() {
    let mut w = world();
    let item = w.seal(b"revoke me", 2, 1000);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&seal_id).unwrap();
    w.chain.revoke(&seal_id);
    let shares = w.collect(&seal_id);
    match w.open(&item, &shares) {
        OpenOutcome::Locked { state: ItemState::Revoked, .. } => {}
        other => panic!("revoked item must be locked, got {other:?}"),
    }
}

// ───────────────────────────── DSCP-2 FastSeal ──────────────────────────────

#[test]
fn fastseal_opens_on_preconf_quorum_before_finality() {
    // A FastSeal item opens on a t-of-n quorum of gateway pre-confirmations, WITHOUT
    // waiting for L1 finality (spec DSCP-2 §4 fast path).
    let mut w = fast_world();
    let item = w.seal_mode(b"fast secret", 2, 1000, SealMode::FastSeal);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();

    // Strict release still yields nothing (chain is not finalised).
    assert!(w.collect(&seal_id).is_empty(), "strict gateways release nothing pre-finality");

    // Fast path: gateways release shares and stake pre-confirmations.
    let shares = w.collect_fast(&seal_id);
    let preconfs = w.preconfs(&seal_id, w.chain.slot());
    assert_eq!(shares.len(), 3);
    assert_eq!(preconfs.len(), 3);
    assert!(w.chain.proof(&seal_id).is_none(), "precondition: not finalised");

    match w.open_fast(&item, &shares, &preconfs) {
        OpenOutcome::Opened { plaintext } => assert_eq!(plaintext, b"fast secret"),
        other => panic!("FastSeal must open on pre-conf quorum, got {other:?}"),
    }
}

#[test]
fn fastseal_locked_without_preconf_quorum() {
    // Below-threshold pre-confirmations do NOT open a FastSeal item.
    let mut w = fast_world();
    let item = w.seal_mode(b"need two", 2, 1000, SealMode::FastSeal);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    let shares = w.collect_fast(&seal_id);
    let one_preconf = vec![w.gateways[0].pre_confirm(&seal_id, w.chain.slot()).unwrap()];
    match w.open_fast(&item, &shares, &one_preconf) {
        OpenOutcome::Locked { .. } => {}
        other => panic!("one pre-conf (< t) must stay locked, got {other:?}"),
    }
}

#[test]
fn strictseal_ignores_preconfs_until_finality() {
    // Vault mode: even a full pre-conf quorum + fast-released shares cannot open a
    // StrictSeal item. Only L1 finality does.
    let mut w = fast_world();
    let item = w.seal_mode(b"vault only", 2, 1000, SealMode::StrictSeal);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    let shares = w.collect_fast(&seal_id);
    let preconfs = w.preconfs(&seal_id, w.chain.slot());
    assert_eq!(preconfs.len(), 3);
    match w.open_fast(&item, &shares, &preconfs) {
        OpenOutcome::Locked { .. } => {}
        other => panic!("StrictSeal must ignore pre-confs, got {other:?}"),
    }
    // Finality opens it.
    w.chain.finalize(&seal_id).unwrap();
    let shares = w.collect(&seal_id);
    assert!(matches!(w.open(&item, &shares), OpenOutcome::Opened { .. }));
}

#[test]
fn forged_preconfs_do_not_count() {
    // Tampered pre-confirmation signatures are ignored, dropping the quorum below t.
    let mut w = fast_world();
    let item = w.seal_mode(b"x", 2, 1000, SealMode::FastSeal);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    let shares = w.collect_fast(&seal_id);
    let mut pcs = w.preconfs(&seal_id, w.chain.slot());
    // corrupt two of three signatures → only one valid pre-conf remains (< t=2)
    pcs[0].signature[0] ^= 0x01;
    pcs[1].signature[0] ^= 0x01;
    match w.open_fast(&item, &shares, &pcs) {
        OpenOutcome::Locked { .. } => {}
        other => panic!("forged pre-confs must not count toward quorum, got {other:?}"),
    }
}

#[test]
fn equivocating_gateway_preconf_is_slashable() {
    // A gateway that pre-confirms a leaf L1 later contradicts produces a standalone
    // fraud proof (the "slashing if equivocated" edge of the architecture).
    let mut w = fast_world();
    let item = w.seal_mode(b"equiv", 2, 1000, SealMode::FastSeal);
    w.deposit_all(&item);
    let seal_id = item.signed_leaf.leaf.seal_id;
    let pc = w.gateways[0].pre_confirm(&seal_id, w.chain.slot()).unwrap();

    // L1 finalises a DIFFERENT leaf hash for this seal_id → equivocation.
    let conflicting = sc::random_32();
    let evidence = detect_equivocation(&pc, &conflicting).expect("must detect equivocation");
    assert!(evidence.is_valid(), "fraud proof must verify on its own");
    assert_eq!(evidence.gateway_id, w.gateways[0].id);

    // Consistent finality (same leaf) is NOT fraud.
    assert!(detect_equivocation(&pc, &item.signed_leaf.leaf.leaf_hash()).is_none());
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn delivery_relay_prefetch_moves_only_ciphertext() {
    // The delivery relay lets the recipient prefetch the LOCKED ciphertext ahead of the
    // release gate; it never carries keys or plaintext, and the prefetched envelope
    // still cannot open until the FastSeal pre-conf quorum arrives.
    let mut w = fast_world();
    let secret = b"prefetched, then opened by pre-confs";
    let item = w.seal_mode(secret, 2, 1000, SealMode::FastSeal);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();

    // Sender posts ONLY the ciphertext envelope; recipient prefetches by mailbox tag.
    let relay = MockDeliveryRelay::new();
    relay.post(item.envelope.clone());
    let tag = item.envelope.recipient_mailbox_tag;
    let fetched = relay.prefetch(&tag);
    assert_eq!(fetched.len(), 1, "recipient prefetches the one posted envelope");
    let prefetched = &fetched[0];

    // What the relay held is ciphertext — the plaintext never appears in it.
    assert!(!contains(&prefetched.ciphertext, secret), "relay must not carry plaintext");

    // Prefetched but still LOCKED (no pre-confirmations yet).
    let shares = w.collect_fast(&seal_id);
    let locked = try_open_dscp2(
        prefetched,
        &item.signed_leaf,
        &w.sender_identity.public(),
        &w.bob_device,
        &shares,
        &[],
        &w.chain,
    );
    assert!(matches!(locked, OpenOutcome::Locked { .. }), "prefetch alone must not open it");

    // Pre-conf quorum → opens straight from the PREFETCHED envelope (no re-fetch).
    let preconfs = w.preconfs(&seal_id, w.chain.slot());
    match try_open_dscp2(
        prefetched,
        &item.signed_leaf,
        &w.sender_identity.public(),
        &w.bob_device,
        &shares,
        &preconfs,
        &w.chain,
    ) {
        OpenOutcome::Opened { plaintext } => assert_eq!(plaintext, secret),
        other => panic!("must open from the prefetched ciphertext, got {other:?}"),
    }
}

// ─────────────────────── DSCP-2 gateway staking / slashing ──────────────────

#[test]
fn stake_below_minimum_is_rejected() {
    let mut reg = GatewayRegistry::new(1_000_000);
    assert!(reg.stake([1u8; 32], 999_999).is_err());
    assert!(reg.stake([1u8; 32], 1_000_000).is_ok());
    assert!(reg.is_active(&[1u8; 32]));
    assert_eq!(reg.total_bonded(), 1_000_000);
}

#[test]
fn valid_evidence_slashes_and_forfeits_bond() {
    let mut w = fast_world();
    let item = w.seal_mode(b"equiv", 2, 1000, SealMode::FastSeal);
    w.deposit_all(&item);
    let seal_id = item.signed_leaf.leaf.seal_id;
    let gw = w.gateways[0].id;
    let pc = w.gateways[0].pre_confirm(&seal_id, w.chain.slot()).unwrap();

    let mut reg = GatewayRegistry::new(1_000_000);
    reg.stake(gw, 5_000_000).unwrap();

    // L1 finalises a different leaf → equivocation → slash.
    let evidence = detect_equivocation(&pc, &sc::random_32()).unwrap();
    assert_eq!(reg.slash(&evidence).unwrap(), 5_000_000, "forfeits the whole bond");
    assert!(matches!(reg.standing(&gw), GatewayStanding::Slashed { .. }));
    assert!(!reg.is_active(&gw));
    // can't double-slash or re-bond a slashed gateway
    assert!(reg.slash(&evidence).is_err());
    assert!(reg.stake(gw, 9_000_000).is_err());
}

#[test]
fn unbonded_or_forged_cannot_be_slashed() {
    let mut w = fast_world();
    let item = w.seal_mode(b"x", 2, 1000, SealMode::FastSeal);
    w.deposit_all(&item);
    let seal_id = item.signed_leaf.leaf.seal_id;
    let mut pc = w.gateways[0].pre_confirm(&seal_id, w.chain.slot()).unwrap();

    let mut reg = GatewayRegistry::new(1_000_000);
    // valid evidence but the gateway never bonded → cannot slash
    let evidence = detect_equivocation(&pc, &sc::random_32()).unwrap();
    assert!(reg.slash(&evidence).is_err(), "unbonded gateway cannot be slashed");

    // forged pre-conf produces no evidence at all
    reg.stake(w.gateways[0].id, 2_000_000).unwrap();
    pc.signature[0] ^= 0x01;
    assert!(detect_equivocation(&pc, &sc::random_32()).is_none(), "forged pre-conf is not evidence");
}

#[test]
fn slashed_gateway_cannot_help_open_fastseal() {
    // End-to-end: a t=2 FastSeal item with pre-confs from two gateways. Slash one, and
    // its pre-conf no longer counts → the item drops below quorum and stays locked.
    let mut w = fast_world();
    let item = w.seal_mode(b"needs two honest", 2, 1000, SealMode::FastSeal);
    w.deposit_all(&item);
    let seal_id = w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    let shares = w.collect_fast(&seal_id);

    let mut reg = GatewayRegistry::new(1_000_000);
    for g in &w.gateways {
        reg.stake(g.id, 2_000_000).unwrap();
    }
    let all_preconfs = w.preconfs(&seal_id, w.chain.slot());

    // All bonded → full quorum opens it.
    let trusted = reg.filter_active(&all_preconfs);
    assert_eq!(trusted.len(), 3);
    assert!(matches!(w.open_fast(&item, &shares, &trusted), OpenOutcome::Opened { .. }));

    // Slash two of three gateways → only one trusted pre-conf remains (< t=2) → locked.
    let bad0 = detect_equivocation(&w.gateways[0].pre_confirm(&seal_id, w.chain.slot()).unwrap(), &sc::random_32()).unwrap();
    let bad1 = detect_equivocation(&w.gateways[1].pre_confirm(&seal_id, w.chain.slot()).unwrap(), &sc::random_32()).unwrap();
    reg.slash(&bad0).unwrap();
    reg.slash(&bad1).unwrap();
    let trusted = reg.filter_active(&all_preconfs);
    assert_eq!(trusted.len(), 1);
    assert!(matches!(w.open_fast(&item, &shares, &trusted), OpenOutcome::Locked { .. }));
}

// ───────────────────────── identity / address / contacts ────────────────────

#[test]
fn identity_address_and_card_roundtrip() {
    let me = Identity::generate("Maya");
    // the address is the base58 WCAHT address of the identity pubkey
    assert_eq!(me.address(), wcaht_address(&me.identity_pub()));
    assert!(!me.address().is_empty());

    // identity is deterministic from its seeds (so it can be persisted + restored)
    let restored = Identity::from_seeds("Maya", me.identity_seed, me.device_seed);
    assert_eq!(restored.identity_pub(), me.identity_pub());
    assert_eq!(restored.device_pub(), me.device_pub());
    assert_eq!(restored.address(), me.address());

    // card encodes → decodes, binding the exact keys + chain + name
    let card = me.card();
    let code = card.encode();
    assert!(code.starts_with("denvion:"));
    let scanned = ContactCard::decode(&code).unwrap();
    assert_eq!(scanned, card);
    assert_eq!(scanned.address(), me.address());
    assert_eq!(scanned.device_pub, me.device_pub());
    assert_eq!(scanned.name, "Maya");
}

#[test]
fn contact_card_rejects_wrong_chain_and_garbage() {
    let card = Identity::generate("Bob").card();
    // wrong chain id in the payload is rejected
    let mut wrong = card.clone();
    wrong.chain_id = 1;
    assert!(ContactCard::decode(&wrong.encode()).is_err());
    // unknown version
    let mut vbad = card.clone();
    vbad.version = 9;
    assert!(ContactCard::decode(&vbad.encode()).is_err());
    // non-base58 garbage
    assert!(ContactCard::decode("denvion:0OIl!!!").is_err());
    // too short
    assert!(ContactCard::decode("denvion:2g").is_err());
}

#[test]
fn i_can_seal_to_a_contact_i_added_from_their_card() {
    // Alice adds Bob by scanning his card, then seals a message to his device key.
    let bob = Identity::generate("Bob");
    let bob_card = ContactCard::decode(&bob.card().encode()).unwrap();

    let alice_id = sc::SignId::generate();
    let alice_dev = sc::SignId::generate();
    let gw_ids: Vec<[u8; 32]> = (0..3).map(|_| sc::random_32()).collect();
    let item = seal_text(
        b"only Bob's device can open this",
        &alice_id, &alice_dev,
        &bob_card.device_pub, // sealed TO the contact's real device key
        sc::random_32(), &gw_ids, 2, 100, 1000,
    )
    .unwrap();
    // the sealed leaf commits to Bob's device, addressed via his card
    assert_eq!(item.signed_leaf.leaf.recipient_device_commitment, device_commitment(&bob_card.device_pub));
    assert_eq!(bob_card.address(), bob.address());
}

#[test]
fn phone_directory_resolves_number_to_address_without_storing_the_number() {
    let bob = Identity::generate("Bob");
    let dir = MockDirectory::new();
    dir.register("+1 (555) 123-4567", bob.card());

    // a differently-formatted SAME number resolves to Bob's address + device key
    let found = dir.lookup("1 555 123 4567").expect("resolves");
    assert_eq!(found.address(), bob.address());
    assert_eq!(found.device_pub, bob.device_pub());

    // an unknown number does not resolve
    assert!(dir.lookup("+1 999 000 0000").is_none());

    // the directory key is a hash of the normalized number — same number, same key
    assert_eq!(phone_commitment("+1 555-123-4567"), phone_commitment("15551234567"));
    // different numbers → different keys
    assert_ne!(phone_commitment("+15551234567"), phone_commitment("+15559999999"));
}

// ─────────────────────────────── media (§8.2 / §10.3 / §11) ──────────────────

#[test]
fn media_round_trips_through_chunks_after_finality() {
    let mut w = world();
    // 3.5 chunks, so the last one is a partial — the common off-by-one.
    let picture: Vec<u8> = (0..3_500u32).map(|i| (i % 251) as u8).collect();
    let m = w.seal_media(&picture, 1000, 2, 100);
    assert_eq!(m.chunks.len(), 4);

    w.deposit_all(&m.item);
    w.chain.submit_leaf(&m.item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&m.item.seal_id).unwrap();
    let shares = w.collect(&m.item.seal_id);

    match w.open_media(&m, &shares) {
        MediaOutcome::Opened { manifest, opener } => {
            assert_eq!(manifest.content_kind, ContentKind::Image);
            assert_eq!(manifest.mime_type, "image/jpeg");
            assert_eq!(manifest.plaintext_size, picture.len() as u64);
            assert_eq!(manifest.width, 1920);
            assert_eq!(manifest.preview, b"blurred-preview");
            assert_eq!(w.reassemble(&opener, &m.chunks), picture);
        }
        other => panic!("expected Opened, got {}", media_label(&other)),
    }
}

#[test]
fn media_chunks_are_useless_before_the_gate_opens() {
    let mut w = world();
    let video: Vec<u8> = (0..5_000u32).map(|i| (i % 97) as u8).collect();
    let m = w.seal_media(&video, 1024, 2, 100);
    w.deposit_all(&m.item);
    w.chain.submit_leaf(&m.item.signed_leaf, &w.sender_identity.public()).unwrap();
    // NOT finalised: the recipient may already hold every byte of ciphertext.
    let shares = w.collect(&m.item.seal_id);

    match w.open_media(&m, &shares) {
        MediaOutcome::Locked { .. } => {}
        other => panic!("media opened before finality: {}", media_label(&other)),
    }
    // and the chunks on their own reveal nothing about the plaintext
    for c in &m.chunks {
        assert_ne!(&c.bytes[..], &video[..c.bytes.len().min(video.len())]);
    }
}

#[test]
fn altered_chunk_is_rejected_against_the_manifest() {
    let mut w = world();
    let bytes: Vec<u8> = (0..2_048u32).map(|i| i as u8).collect();
    let m = w.seal_media(&bytes, 512, 2, 100);
    w.deposit_all(&m.item);
    w.chain.submit_leaf(&m.item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&m.item.seal_id).unwrap();
    let shares = w.collect(&m.item.seal_id);

    let MediaOutcome::Opened { opener, .. } = w.open_media(&m, &shares) else {
        panic!("expected Opened");
    };
    // a relay that flips one byte of one chunk is caught by the manifest hash
    let mut tampered = m.chunks[1].bytes.clone();
    tampered[7] ^= 0x01;
    assert!(opener.decrypt_chunk(1, &tampered).is_err());
    // the untouched chunk still opens
    assert!(opener.decrypt_chunk(1, &m.chunks[1].bytes).is_ok());
}

#[test]
fn chunks_cannot_be_reordered() {
    let mut w = world();
    let bytes: Vec<u8> = (0..2_048u32).map(|i| i as u8).collect();
    let m = w.seal_media(&bytes, 512, 2, 100);
    w.deposit_all(&m.item);
    w.chain.submit_leaf(&m.item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&m.item.seal_id).unwrap();
    let shares = w.collect(&m.item.seal_id);

    let MediaOutcome::Opened { opener, .. } = w.open_media(&m, &shares) else {
        panic!("expected Opened");
    };
    // serving chunk 2's bytes as chunk 0 fails: position is bound by both the manifest
    // hash list and the per-chunk key/AAD.
    assert!(opener.decrypt_chunk(0, &m.chunks[2].bytes).is_err());
    // and an out-of-range index is refused rather than panicking
    assert!(opener.decrypt_chunk(99, &m.chunks[0].bytes).is_err());
}

#[test]
fn manifest_root_binds_the_chunk_list_to_the_signed_leaf() {
    let w = world();
    let bytes: Vec<u8> = (0..3_000u32).map(|i| i as u8).collect();
    let m = w.seal_media(&bytes, 1000, 2, 100);

    // the leaf commits to the real chunk list …
    let hashes: Vec<[u8; 32]> = m.chunks.iter().map(|c| c.ciphertext_hash).collect();
    assert_eq!(m.item.signed_leaf.leaf.manifest_root, manifest_root_of(&hashes));
    assert_ne!(m.item.signed_leaf.leaf.manifest_root, [0u8; 32]);
    assert_eq!(m.item.signed_leaf.leaf.content_type, ContentType::Media);

    // … and dropping or swapping any chunk changes the root
    let mut short = hashes.clone();
    short.pop();
    assert_ne!(manifest_root_of(&short), m.item.signed_leaf.leaf.manifest_root);
    let mut swapped = hashes.clone();
    swapped.swap(0, 1);
    assert_ne!(manifest_root_of(&swapped), m.item.signed_leaf.leaf.manifest_root);
}

#[test]
fn media_leaf_leaks_nothing_about_the_file() {
    let w = world();
    let bytes = b"JPEG-ish payload that should never be inferable from the chain".to_vec();
    let m = w.seal_media(&bytes, 16, 2, 100);
    let leaf = &m.item.signed_leaf.leaf;

    // what goes on-chain is the leaf. It must not carry mime, filename, size, caption or preview.
    let public = leaf.canonical_bytes();
    assert!(!contains(&public, b"image/jpeg"), "mime type leaked into the leaf");
    assert!(!contains(&public, b"caption"), "caption leaked into the leaf");
    assert!(!contains(&public, b"blurred-preview"), "preview leaked into the leaf");
    assert!(!contains(&public, &bytes), "plaintext leaked into the leaf");
    // the plaintext length is not a public field either
    assert!(!contains(&public, &(bytes.len() as u64).to_le_bytes()));

    // the envelope the relay stores is ciphertext only
    let relay_sees = &m.item.envelope.ciphertext;
    assert!(!contains(relay_sees, b"image/jpeg"));
    assert!(!contains(relay_sees, b"caption"), "caption leaked to the relay");
    assert!(!contains(relay_sees, &bytes));
}

#[test]
fn text_seals_still_have_a_zero_manifest_root() {
    let w = world();
    let item = w.seal(b"plain text", 2, 100);
    assert_eq!(item.signed_leaf.leaf.manifest_root, [0u8; 32]);
    assert_eq!(item.signed_leaf.leaf.content_type, ContentType::Text);
}

#[test]
fn empty_media_is_refused() {
    let w = world();
    let err = seal_media_with_mode(
        b"",
        ContentKind::Video,
        "video/mp4",
        "",
        b"",
        PreviewPolicy::None,
        (0, 0),
        0,
        1024,
        &w.sender_identity,
        &w.sender_device,
        &w.bob_device.public(),
        sc::random_32(),
        &w.gw_ids,
        2,
        w.chain.slot(),
        100,
        SealMode::StrictSeal,
        0,
        0,
    );
    assert!(err.is_err());
}

fn media_label(o: &MediaOutcome) -> String {
    match o {
        MediaOutcome::Locked { reason, .. } => format!("Locked({reason})"),
        MediaOutcome::Opened { .. } => "Opened".to_string(),
        MediaOutcome::Rejected { reason } => format!("Rejected({reason})"),
    }
}

#[test]
fn the_timelock_lives_in_the_signed_leaf_and_cannot_be_moved() {
    let mut w = world();
    let now = now_unix();
    let item = seal_text_with_mode(
        b"open me later",
        &w.sender_identity,
        &w.sender_device,
        &w.bob_device.public(),
        sc::random_32(),
        &w.gw_ids,
        2,
        w.chain.slot(),
        100_000,
        SealMode::StrictSeal,
        now + 3600, // reveal
        0,
    )
    .expect("seal");

    // the window is IN the leaf, so the sender signature and the anchored leaf hash cover it
    assert_eq!(item.signed_leaf.leaf.reveal_at_unix, now + 3600);

    w.deposit_all(&item);
    w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&item.seal_id).unwrap();
    let shares = w.collect(&item.seal_id);
    assert_eq!(shares.len(), 3, "gateways released — finality is satisfied");

    // FINALISED, every share in hand, and it STILL will not open: the timelock is enforced
    // by the protocol core, not by the app drawing a lock over the top.
    match w.open(&item, &shares) {
        OpenOutcome::Locked { reason, .. } => assert!(reason.contains("timelocked"), "{reason}"),
        other => panic!("timelocked item opened early: {other:?}"),
    }

    // Moving the deadline invalidates the sender signature — a relay cannot shorten it.
    let mut tampered = item.signed_leaf.clone();
    tampered.leaf.reveal_at_unix = now - 1;
    match try_open(&item.envelope, &tampered, &w.sender_identity.public(), &w.bob_device, &shares, &w.chain) {
        OpenOutcome::Rejected { reason } => assert!(reason.contains("signature"), "{reason}"),
        other => panic!("edited deadline was accepted: {other:?}"),
    }
}

#[test]
fn a_destroyed_item_never_opens_even_with_every_share() {
    let mut w = world();
    let now = now_unix();
    let item = seal_text_with_mode(
        b"gone",
        &w.sender_identity,
        &w.sender_device,
        &w.bob_device.public(),
        sc::random_32(),
        &w.gw_ids,
        2,
        w.chain.slot(),
        100_000,
        SealMode::StrictSeal,
        0,
        now - 1, // destroy_at already passed
    )
    .expect("seal");
    w.deposit_all(&item);
    w.chain.submit_leaf(&item.signed_leaf, &w.sender_identity.public()).unwrap();
    w.chain.finalize(&item.seal_id).unwrap();
    let shares = w.collect(&item.seal_id);
    match w.open(&item, &shares) {
        OpenOutcome::Locked { state, reason } => {
            assert_eq!(state, ItemState::Expired);
            assert!(reason.contains("self-destructed"), "{reason}");
        }
        other => panic!("self-destructed item opened: {other:?}"),
    }
}
