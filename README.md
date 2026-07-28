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
│                 + threat-model tests
├─ seal-ffi/      C ABI (iOS/Swift) + JNI (Android/Kotlin) over seal-core + on-device demo
├─ wcaht-seal-sdk/ REAL WCAHT integration — live finality read + anchor-tx signer
│                 (WcahtSealChain implements seal_core::SealChain) + HTTP payload-
│                 prefetch delivery relay (ciphertext-only store-and-forward)
├─ ios-app/       NATIVE SwiftUI messenger (calls the C ABI)
└─ android-app/   NATIVE Kotlin + Jetpack Compose messenger (calls JNI)
```

Both apps share the exact same Rust core; only the UI layer differs per platform.

## What works right now (verified)

```bash
cd denvion-splitseal
cargo test          # 14 tests green: locked-until-finality + full threat matrix
```

The tests prove the core promise: nothing opens before a **finalised** seal **and**
`t` released key shares, and every misuse is rejected — wrong recipient, altered
ciphertext, replay, wrong chain id, expiry, revocation, and insufficient shares. A
single gateway can be offline and a `t=2` item still opens.

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
```bash
cargo build --release -p seal-ffi --target aarch64-apple-ios          # device
cargo build --release -p seal-ffi --target aarch64-apple-ios-sim      # simulator (Apple Silicon)
```
In Xcode, create an iOS **App** target and add the three `ios-app/SplitSeal/*.swift`
files, then:
- **Link Binary With Libraries** ▸ add `libseal_ffi.a`,
- Build Settings ▸ **Objective-C Bridging Header** = `SplitSeal-Bridging-Header.h`,
- copy `seal-ffi/include/seal_ffi.h` into the project (or add its dir to Header Search Paths),
- Build Settings ▸ **Other Linker Flags**: `-force_load $(PROJECT_DIR)/libseal_ffi.a`
  (stops the linker dead-stripping the `ss_*` symbols).

### Run
Launch on a simulator/device. A segmented control at the top switches the release mode:

- **StrictSeal · vault** — the bubble renders **locked**, shows **Securing seal…**, then
  **opens** once the (mock) WCAHT seal finalises and the gateways release shares
  (`ss_run_demo`).
- **FastSeal · instant** — the bubble opens on a **gateway pre-confirmation quorum**,
  before finality (`ss_run_fast_demo`); the status chip reads *Awaiting gateway pre-confs*
  → *Opened · pre-confirmed*.

Both flows execute in the shared Rust core, on-device, identically on iOS and Android.

> Build `seal-ffi` for the target platform first; each app calls into it at launch.

## Security posture (Phase 0/1)

- **No homemade crypto** — everything is in `seal-crypto` over audited crates (spec §7.1).
- **Wallet keys ≠ chat keys** — chat identity/device keys are independent of any WCAHT wallet.
- **Off-chain content, on-chain commitment** — the chain only ever sees the seal leaf/root, never plaintext.
- ⚠️ **Not production-ready:** needs an independent crypto/protocol audit, real (not mock)
  WCAHT finality, and hardened key storage before any real use. Do not make “unbreakable”
  or legal-enforceability claims (spec §25).

## Roadmap (what's next, on top of this core)

| Phase | Work |
|---|---|
| 2 ▸ *started* | ✅ `wcaht-seal-sdk`: live finality read + `WcahtSealChain` (real `SealChain`, verified end-to-end); ✅ byte-exact `TX::v2` anchor signer. ▸ remaining: submit the anchor tx from a funded gateway account (API key), 3 independent seal-gateway services, delivery-relay service, device linking + proof screen |
| 2+ ▸ **done** | ✅ **DSCP-2** protocol: `SealMode` on the leaf, slashable `PreConfirmation`s, mode-aware `try_open_dscp2`, equivocation → `SlashingEvidence`. ✅ **payload-prefetch delivery relay** (ciphertext-only HTTP; FastSeal e2e ~37ms, no finality). ✅ **gateway staking/slashing** (`GatewayRegistry` + on-chain stake/slash anchors — full loop verified live). ✅ **StrictSeal/FastSeal UI toggle** in both native apps |
| 3 | Real multi-gateway + delivery-relay services in production, device linking + proof screen, media (encrypted chunked images/voice/docs), MLS groups, `CALL_SESSION` live calls, 24h soak + external audit |

The core here is written so FastSeal slots in as an alternative release gate without
rewriting the client protocol.
