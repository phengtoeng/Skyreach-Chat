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
