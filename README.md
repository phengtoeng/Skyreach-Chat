# Denvion SplitSeal

> 🧭 **Working on this? Read [`SKYREACH_DOC.md`](SKYREACH_DOC.md) first** — the authoritative
> task/status doc: what's built & verified, live-chain facts (nodes, keys, fees), constraints,
> and exactly what to work on next.

WCAHT-native **sealed messenger** for **iOS + Android**. Encrypted content travels
over a fast delivery route; a separate WCAHT seal route proves + timestamps + gates
its opening. **Content stays locked until the seal is finalised.**

This repo is the **Phase 0/1 foundation** implementing **DSCP-2**: a real, tested Rust
protocol core + audited-crypto facade + a mock WCAHT seal chain + real-chain SDK +
**native** apps for both platforms (SwiftUI + Kotlin/Compose). It supports both release
modes — **StrictSeal** (opens only at hard L1 finality) and **FastSeal** (opens on a
quorum of slashable gateway pre-confirmations, sub-250ms, before finality). It is
**not** the full production network yet (see Roadmap).

```
denvion-splitseal/
├─ seal-crypto/   audited-crypto facade — NO homemade primitives
│                 (XChaCha20-Poly1305 · Ed25519 · X25519 HPKE · BLAKE3 · Shamir t/n)
├─ seal-core/     DSCP-1 objects, client state machine, mock WCAHT seal chain,
│                 gateways (strict post-finality release), sender/recipient flows,
│                 sealed MEDIA (chunked + encrypted manifest), + threat-model tests
├─ seal-ffi/      C ABI (iOS/Swift) + JNI (Android/Kotlin) over seal-core + on-device demo
├─ wcaht-seal-sdk/ REAL WCAHT integration — live finality read + anchor-tx signer
│                 (WcahtSealChain implements seal_core::SealChain) + HTTP payload-
│                 prefetch delivery relay (ciphertext-only store-and-forward,
│                 incl. the /blob media chunk store)
├─ ios-app/       NATIVE SwiftUI messenger (calls the C ABI)
└─ android-app/   NATIVE Kotlin + Jetpack Compose messenger (calls JNI)
```

Both apps share the exact same Rust core; only the UI layer differs per platform.

## What works right now (verified)

```bash
cd denvion-splitseal
cargo test          # 63 tests green: locked-until-finality + full threat matrix + media + batching
```

The tests prove the core promise: nothing opens before a **finalised** seal **and**
`t` released key shares, and every misuse is rejected — wrong recipient, altered
ciphertext, replay, wrong chain id, expiry, revocation, and insufficient shares. A
single gateway can be offline and a `t=2` item still opens. For media they also prove
that a recipient holding **every** chunk still cannot open one before the gate, and that
an altered or reordered chunk is refused.

## Every message touches the chain — and the app pays for it

A message is not just encrypted locally; it is **committed on-chain**. Its signed leaf is
folded into a merkle root and that root is anchored in a real WCAHT transaction, so there is
public, timestamped evidence that this exact message existed by that slot.

The user never pays and never signs. Chat identity keys and wallet keys are deliberately
separate, so a user has no chain account that *could* be charged — the fee comes from a
Skyreach treasury account (gas sponsorship, like a Solana fee payer or an ERC-4337 paymaster).

It stays affordable because commitment is **batched**: every message in a window shares one
root and therefore **one transaction**, at a flat 5,001 kak no matter how many messages are in
it (up to 10,000). Each message still gets its own inclusion proof, and the recipient verifies
that proof against its **own** leaf — the batching service is never trusted. A message that was
never batched cannot borrow someone else's root.

This needed no consensus change: the root *is* the transaction's recipient address, so a
confirmed transaction paying it is itself the proof. See `SKYREACH_DOC.md` §6c.

## Two release modes (DSCP-2)

Every seal carries a `SealMode` in its signed leaf, so it can't be silently downgraded:

| Mode | Opens when | Use |
|---|---|---|
| **StrictSeal** (vault) | the WCAHT seal reaches **hard L1 finality** | high-assurance: OTC settlement, legal sign-off, sealed bids |
| **FastSeal** (fast path) | a **quorum of `t` slashable gateway pre-confirmations** — sub-250ms — *or* finality, whichever is first | real-time chat feel |

A `PreConfirmation` is a gateway's signed, staked promise that it sequenced the seal
committing to *this exact leaf hash*. If L1 later finalises a **different** leaf for the
same `seal_id`, `detect_equivocation` produces standalone `SlashingEvidence` — the
gateway is provably lying and its bond is slashable. FastSeal only counts pre-confs from
gateways that actually hold a share, that commit to the real leaf, and whose signatures
verify; forged or sub-quorum pre-confs keep the item locked (all covered by tests).

Both modes run on-device through the FFI: `ss_run_demo` (StrictSeal) and
`ss_run_fast_demo` (FastSeal opening before finality).

### FastSeal end-to-end (with the prefetch relay)

The delivery relay is an untrusted, ciphertext-only store-and-forward. The recipient
**prefetches the locked ciphertext** ahead of the release gate, so unlocking needs no
network round-trip:

```bash
cargo run -p wcaht-seal-sdk --bin wcaht-seal-relay        # start the relay (or use the in-process one)
cargo run -p wcaht-seal-sdk --bin wcaht-seal-fast-e2e     # spins a relay + runs the full fast path
```

Measured run (no L1 finality involved):

```
sender posted ciphertext to relay (67 bytes)
recipient prefetched locked ciphertext in 0.52 ms
  before pre-confs: LOCKED (fast-path: 0/2 gateway pre-confirmations …)
  3 pre-confs → unlock in 36 ms (local; ciphertext already in hand)
  OPENED: "FastSeal: prefetched locked, unlocked by pre-confs."
finalised? false   total prefetch+unlock ≈ 37 ms
```

Well under the 250ms target — because the ciphertext was prefetched, the unlock at
quorum time is a purely local crypto operation.

## Sealed media — photos and video

A photo or video is **never** carried in the envelope, and never rests anywhere readable.
The envelope carries an **encrypted `MediaManifest`**; the pixels go out as separately
encrypted chunks that the relay stores as opaque blobs. The chain sees only
`manifest_root` — 32 bytes, with no filename, mime type, size, dimensions or thumbnail.

```
sender ──encrypted chunks────────▶ relay    PUT /blob/<ciphertext-hash>   (opaque)
       ──encrypted manifest──────▶ relay    POST /inbox/<mailbox tag>
       ──signed leaf────────────▶ WCAHT    manifest_root, 32 bytes
       ──key shares─────────────▶ gateways encrypted to the recipient device
```

Every chunk key is a KDF subkey of `K_content`, which is Shamir-split across the gateways.
So a recipient can pre-download an **entire video that stays cryptographically unopenable**
until the release gate opens — the same gate that drives time-reveal and time-destroy. It
isn't the app politely hiding a file it could show.

The relay verifies the content address (bytes must hash to the name they claim), so it
cannot substitute a chunk — and it holds no key, so it can never open one. Chunks default
to 1 MiB, with per-chunk keys from a reviewed KDF so no (key, nonce) pair can repeat.

```bash
cargo run -p wcaht-seal-sdk --bin wcaht-seal-media-e2e   # relay + 3 gateways, over real HTTP
```

Measured run — the recipient downloads every byte *before* finality and still gets nothing:

```
1. Alice picks a 2.50 MiB image
   sealed into 3 encrypted chunks; leaf carries manifest_root 153bd9be… and NOTHING else
2. uploaded 3 chunks to the relay (it stores ciphertext it cannot open)
   relay rejects a blob that doesn't match its hash → HTTP 400
4. Bob downloads all 2621560 bytes of ciphertext BEFORE finality
   → LOCKED: waiting for WCAHT finality (StrictSeal vault mode)
5. seal FINALISED on WCAHT; 3 gateways released their shares
6. decrypted + reassembled 2621440 bytes — byte-identical to Alice's file ✓
7. a single flipped byte in chunk 1 → REFUSED: chunk 1 does not match its manifest hash
```

Verified on the live backbone (2026-08-01): a photo sent from the Android app round-tripped
byte-identically through N5/N6/N7, with the relay's stored copy at **7.997 bits/byte entropy**
and no image magic — indistinguishable from random.

### Gateway staking & slashing (on-chain)

A FastSeal pre-confirmation is only as trustworthy as the gateway's stake behind it.
`GatewayRegistry` is the economic layer: gateways bond on WCAHT, and a valid
`SlashingEvidence` (a pre-conf that L1 later contradicts) forfeits the bond and drops
the gateway from every FastSeal quorum (`filter_active`). `AnchorSigner::stake_anchor_tx`
locks the bond into a deterministic, unspendable escrow derived from the gateway id.

```bash
WCAHT_API_KEY=<submit-key> \
  cargo run -p wcaht-seal-sdk --bin wcaht-seal-stake -- http://<validator>:8901
```

Verified on the live chain (2026-07-29): gateway bonded 5,000,000 → stake tx confirmed
in slot 1051758 → the gateway equivocated → fraud proof verified → **slashed (bond
forfeited)** → slash claim published on-chain in slot 1051777 → gateway excluded from
FastSeal (`is_active = false`, 0 pre-confs kept). The whole bonded → equivocated →
slashed → excluded loop runs against real transactions.

### Against the REAL chain (not the mock)

`wcaht-seal-sdk` reads live finality from a running WCAHT node and gates opening on
it. Point it at a node (default N1 `http://127.0.0.1:8901`):

```bash
cargo run -p wcaht-seal-sdk --bin wcaht-seal-probe        # prints REAL finalized_slot / recent_blockhash
cargo run -p wcaht-seal-sdk --bin wcaht-seal-live-demo    # seal → LOCKED → (real finality) → OPENED
```

Verified live output — the message stays locked until the chain finalises past the
anchor slot, then opens:

```
chain finalized_slot at seal time = 1022990
[t0] status=Finalising  shares_released=0  ->  LOCKED (waiting for WCAHT finality)
waiting for real finality to advance..........  finalized_slot 1022990 -> 1023008
[t1] status=Finalised   shares_released=3  ->  OPENED: "Opened only after the real WCAHT chain finalised."
```

`WcahtSealChain` implements the same `seal_core::SealChain` trait the mock does, so
`try_open` is unchanged — only the finality source is swapped.

**The anchor transaction now lands on-chain for real.** `AnchorSigner` builds + signs
a byte-exact WCAHT `TX::v2` transfer that commits the leaf hash; `wcaht-seal-anchor`
submits it to a **voting validator** and waits for confirmation + finality:

```bash
WCAHT_API_KEY=<submit-key> \
  cargo run -p wcaht-seal-sdk --bin wcaht-seal-anchor -- http://<validator>:8901 <funded-keypair.json>
```

Verified on the live chain (2026-07-29): anchor tx accepted → confirmed in slot
1045452 → LOCKED until finality reached it → OPENED. Notes: submit to a validator,
not a non-voting follower (followers return `NON_VALIDATOR_TX_SUBMIT_DISABLED`); the
consensus minimum fee is `compute_units × price_per_cu`.

## Build for iOS + Android (native)

The Rust core (`seal-ffi`) compiles into each native app: a static lib linked into
the SwiftUI app on iOS, and a `.so` loaded by the Kotlin app on Android.

### One-time toolchain
```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk        # Android .so packaging
```

### Android (Kotlin + Jetpack Compose) — `android-app/`
```bash
# from repo root: build the core into the app's jniLibs
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
  -o android-app/app/src/main/jniLibs build --release -p seal-ffi
# then open android-app/ in Android Studio and Run  (or: cd android-app && ./gradlew installDebug)
```
`SealCore.kt` does `System.loadLibrary("seal_ffi")` and calls the JNI exports
(`Java_com_denvion_splitseal_SealCore_native*`).

### iOS (SwiftUI) — `ios-app/`
The Xcode project is generated from `ios-app/project.yml` by [XcodeGen]; the `.xcodeproj` is
gitignored, so there is nothing to wire up by hand:

```bash
brew install xcodegen                 # once
cd ios-app && xcodegen generate       # writes SplitSeal.xcodeproj + SplitSeal/Info.plist
open SplitSeal.xcodeproj
```

`project.yml` already sets the bridging header, the `seal-ffi/include` header search path, and
`-force_load` (which stops the linker dead-stripping the `ss_*` symbols), and runs
`scripts/build_rust_ios.sh` as a prebuild step. That script builds **two** static libs — a
simulator lib (`libseal_ffi_sim.a`, x86_64 + arm64-sim) and a device one
(`libseal_ffi_ios.a`, arm64) — because a simulator slice cannot run on a phone;
`OTHER_LDFLAGS[sdk=…]` links whichever matches the active SDK.
It also carries the Info.plist keys the app needs at runtime — `NSAppTransportSecurity ▸
NSAllowsArbitraryLoads` (the backend is plain http), plus the camera and contacts usage
descriptions. **Edit `project.yml`, not the generated project.**

[XcodeGen]: https://github.com/yonaskolb/XcodeGen

### Run
Launch on a simulator/device. The chat list has a floating bottom bar —
**Contacts · Calls · Chats · Settings**, plus a search button. Inside a conversation, the
lock/bolt button in the header switches the release mode for the next message:

- **StrictSeal · vault** (lock) — the bubble renders **locked** and shows
  **Waiting for finality**, then opens once the WCAHT seal finalises and the gateways
  release their shares.
- **FastSeal · instant** (bolt) — the bubble opens on a **gateway pre-confirmation quorum**,
  before finality; the chip reads *Waiting for pre-confirms*.

The clock button sets a one-shot **time-reveal / time-destroy** window on the next message,
and the camera button seals a **photo or video** (§ Sealed media).

Both flows execute in the shared Rust core, on-device, identically on iOS and Android.

> Build `seal-ffi` for the target platform first; each app calls into it at launch.

## Security posture (Phase 0/1)

- **No homemade crypto** — everything is in `seal-crypto` over audited crates (spec §7.1).
- **Wallet keys ≠ chat keys** — chat identity/device keys are independent of any WCAHT wallet.
- **Off-chain content, on-chain commitment** — the chain only ever sees the seal leaf/root, never plaintext.
- **Media is opaque to every server** — the relay stores ciphertext chunks it cannot open, and
  the chain never learns the mime type, size, filename or thumbnail.
- ⚠️ **Not production-ready:** needs an independent crypto/protocol audit, real (not mock)
  WCAHT finality, and hardened key storage before any real use. Do not make “unbreakable”
  or legal-enforceability claims (spec §25).
- ⚠️ **iOS media UI is written but has never been compiled** — there is no Mac in the loop
  that produced it. Build it before trusting it.

## Roadmap (what's next, on top of this core)

| Phase | Work |
|---|---|
| 2 ▸ *started* | ✅ `wcaht-seal-sdk`: live finality read + `WcahtSealChain` (real `SealChain`, verified end-to-end); ✅ byte-exact `TX::v2` anchor signer. ▸ remaining: submit the anchor tx from a funded gateway account (API key), 3 independent seal-gateway services, delivery-relay service, device linking + proof screen |
| 2+ ▸ **done** | ✅ **DSCP-2** protocol: `SealMode` on the leaf, slashable `PreConfirmation`s, mode-aware `try_open_dscp2`, equivocation → `SlashingEvidence`. ✅ **payload-prefetch delivery relay** (ciphertext-only HTTP; FastSeal e2e ~37ms, no finality). ✅ **gateway staking/slashing** (`GatewayRegistry` + on-chain stake/slash anchors — full loop verified live). ✅ **StrictSeal/FastSeal UI toggle** in both native apps |
| 3 ▸ *started* | ✅ **sealed media** — chunked + encrypted manifest, `manifest_root` on-chain, `/blob` store on the live relays (N5/N6/N7), photo+video send/receive in the Android app. ▸ remaining: production multi-gateway services, device linking + proof screen, voice notes/documents, blob GC after `destroy_at`, MLS groups, `CALL_SESSION` live calls, 24h soak + external audit |
| 3 ▸ **done** | ✅ **batched on-chain commitment + sponsored fees** — `SEAL_ROOT` merkle batching, one transaction per batch, treasury-paid so users never sign or pay; per-message inclusion proofs verified against the recipient's own leaf. Live on N5/N6/N7 with `/health` alerting. ▸ remaining: a treasury top-up runbook, and per-node treasury accounts to shrink the blast radius of one shared key |

The core here is written so FastSeal slots in as an alternative release gate without
rewriting the client protocol.
