# SKYREACH_DOC — Denvion SplitSeal task & status doc

> **Read this first.** It is the single source of truth for what Denvion SplitSeal is,
> what is already built and verified, how to build/run it, the live-chain facts you need,
> and what to work on next. Keep it updated as work lands.

_Last updated: 2026-07-29._

---

## 0. TL;DR

Denvion SplitSeal is a **WCAHT-native sealed messenger**: encrypted content travels off-chain,
and a WCAHT **seal** gates when it can be opened. Nothing opens until the seal releases —
either at **hard L1 finality** (StrictSeal / vault) or on a **quorum of slashable gateway
pre-confirmations** (FastSeal / sub-250ms fast path). This is protocol **DSCP-2**.

**Status: DSCP-2 is feature-complete.** Protocol core, real on-chain anchoring, payload-prefetch
delivery relay, gateway staking/slashing, and a dual-mode UI in both native apps are all built.
**27 Rust tests green.** Three real transactions landed on the live chain (see §6). Not audited;
Phase 3 (production services + media/groups/calls + audit) is what remains.

- **Location:** `C:\Users\toeng\Desktop\WCAHT\denvion-splitseal\` — sibling of the WCAHT
  blockchain repo `PoASy3\`, **not inside it**.
- **Chain id:** `7789`. **Protocol version:** `2` (DSCP-2).

---

## 1. What it is (and is not)

- **Is:** a high-assurance "proof-and-release" messenger — sealed OTC trades, legal sign-off,
  sealed bids, anything where *when a message can be opened* is enforced by consensus, not by an
  honest app. Content is end-to-end encrypted; the chain only ever sees a commitment (leaf hash),
  never plaintext or keys.
- **Is not (yet):** a consumer WhatsApp/Telegram rival, an audited product, or anything you may
  market as "unbreakable" / legally enforceable. Do **not** make those claims (spec §25).

Two release modes, both bound into the signed seal leaf so they can't be silently downgraded:

| Mode | Opens when | For |
|---|---|---|
| **StrictSeal** (vault) | hard **L1 finality** of the seal | max assurance |
| **FastSeal** (fast path) | **t-of-n slashable gateway pre-confirmations** (or finality, whichever first) | real-time feel |

---

## 2. Status at a glance

| Area | State |
|---|---|
| Audited crypto facade (`seal-crypto`) | ✅ done, 4 tests |
| DSCP-1 protocol + threat matrix (`seal-core`) | ✅ done |
| DSCP-2 StrictSeal + FastSeal + pre-confs + slashing | ✅ done |
| Payload-prefetch delivery relay | ✅ done (HTTP + in-proc), FastSeal e2e ~37ms |
| Gateway staking / slashing registry + on-chain anchors | ✅ done, full loop verified live |
| Real WCAHT finality read + anchor/stake/slash txs (`wcaht-seal-sdk`) | ✅ done, 3 txs landed on-chain |
| FFI: C ABI (iOS) + JNI (Android) | ✅ done, 2 tests |
| Native apps (SwiftUI + Kotlin/Compose) with StrictSeal/FastSeal toggle | ✅ code done; **needs Xcode/Android SDK to compile** |
| Total Rust tests | **27 green** |
| Production gateway/relay services, device linking, media/groups/calls, audit | ❌ Phase 3 |

---

## 3. Repository layout

```
denvion-splitseal/
├─ seal-crypto/     audited-crypto facade — NO homemade primitives
│                   (XChaCha20-Poly1305 · Ed25519 · X25519 HPKE · BLAKE3 · Shamir t/n)
├─ seal-core/       DSCP-2 protocol: SealLeaf/SealMode, EncryptedEnvelope, KeyShareEnvelope,
│                   MockSealChain, Gateway, PreConfirmation, SlashingEvidence,
│                   GatewayRegistry (staking/slashing), DeliveryRelay, seal_text(_with_mode),
│                   try_open / try_open_dscp2 + threat-model & DSCP-2 tests
├─ seal-ffi/        C ABI (ss_version/ss_run_demo/ss_run_fast_demo/ss_free) + JNI bridge;
│                   on-device StrictSeal & FastSeal demo transcripts
├─ wcaht-seal-sdk/  REAL WCAHT integration: WcahtRpc (live finality), AnchorSigner
│                   (byte-exact TX::v2 signer), WcahtSealChain (SealChain over real finality),
│                   stake_anchor_tx + gateway_escrow_address, relay.rs (HTTP delivery relay).
│                   Bins: probe, live-demo, anchor, relay, fast-e2e, stake
├─ ios-app/         NATIVE SwiftUI messenger (SealCore.swift over the C ABI) + mode toggle
└─ android-app/     NATIVE Kotlin/Compose messenger (SealCore.kt over JNI) + mode toggle
```

Both apps drive the **same** Rust core; only the UI layer differs.

---

## 4. Build & test

```bash
cd denvion-splitseal
cargo test          # 27 tests green (crypto, threat matrix, DSCP-2, registry, relay, SDK)
cargo build         # clean, no warnings
```

Mobile (needs the platform toolchains — NOT available on the dev machine used so far):

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk

# Android: build the core into jniLibs, then open android-app/ in Android Studio
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
  -o android-app/app/src/main/jniLibs build --release -p seal-ffi

# iOS: build the static lib, then link libseal_ffi.a in Xcode with the bridging header
cargo build --release -p seal-ffi --target aarch64-apple-ios
```

See `README.md` for the full Xcode wiring (`-force_load`, bridging header, header search path).

---

## 5. Runnable demos & tools (all in `wcaht-seal-sdk`)

| Bin | What it proves | Needs |
|---|---|---|
| `wcaht-seal-probe [url]` | reads REAL `finalized_slot` + `recent_blockhash` from a live node | any node |
| `wcaht-seal-live-demo [url]` | seal → LOCKED → (real finality advances) → OPENED (baseline anchor, no submit) | any node |
| `wcaht-seal-anchor <url> [kp]` | submits a REAL anchor tx, confirms it, opens after finality | validator + API key + keypair |
| `wcaht-seal-relay [addr]` | runs the HTTP payload-prefetch delivery relay | — |
| `wcaht-seal-fast-e2e` | FastSeal end-to-end via prefetch relay, timed (~37ms, no finality) | — (spins its own relay) |
| `wcaht-seal-stake <url> [kp]` | on-chain gateway bond → equivocate → slash → slash-claim published → excluded | validator + API key + keypair |

FFI demos (on-device, no chain): `ss_run_demo` (StrictSeal), `ss_run_fast_demo` (FastSeal).

---

## 6. LIVE WCAHT integration — operational facts you WILL need

The SplitSeal SDK talks to the real WCAHT chain (`PoASy3`). Facts learned the hard way:

- **Submit to a VOTING VALIDATOR, not the observer.** The local node N1 (`http://127.0.0.1:8901`)
  reads finality fine but **rejects tx submit** with `NON_VALIDATOR_TX_SUBMIT_DISABLED` (it's a
  non-voting follower). Submit to a validator:
  - N5 `http://139.99.150.23:8901`, N6 `http://51.79.176.134:8901`, N7 `http://51.79.162.80:8901`
    (all `:8901`, `is_voting_validator: true`).
- **API key:** the `SubmitTransaction`/Admin key lives in `PoASy3/security_config.json`
  (`api_auth.keys[0].key`). Header is `x-api-key` (case-insensitive). Same key works on N1 + all
  validators; `PoASy3/tools/heartbeat.py` uses it too (`HEARTBEAT_API_KEY`). Pass it via the
  `WCAHT_API_KEY` env var to the bins — do **not** hardcode it in source.
- **Funded signer:** `PoASy3/heartbeat_keypair.json` — account `CBXRVVKSALoR5sPjACETuhQZd3xj3M1AWvq5nGH8PwiD`
  (funded ~3yr). The ed25519 **seed is the first 32 bytes** of its 64-byte `keypair` array.
  (In production each gateway funds its own; for tests this one account bonds/anchors.)
- **Minimum fee = `compute_units × price_per_cu` = 200_000** (NOT the base 5000). Below that you
  get `fee X below deterministic consensus minimum 200000`. The bins use cu=200_000, fee=200_000.
- **`recent_blockhash` is a 64-char HEX string** in the tx JSON (`deserialize_recent_blockhash`
  wants exactly 64 hex chars). Fetch it from `GET /blockchain/recent_blockhash`.
- **Confirm a tx** via `GET /transaction/<base58-signature>` → `{"status":"confirmed","slot":N,…}`
  (404 until it's included). Poll it.
- **Canonical signing:** `AnchorSigner::canonical_bytes_v2` is a byte-exact replica of the runtime
  `Transaction::canonical_bytes()` (`TX::v2`) — see `PoASy3/src/transaction/transaction.rs:456`.
  If you change the tx shape, re-check it against that function.

**On-chain proof points (all from `CBXRVVK…` via validator N5, 2026-07-29):**
- seal **anchor** tx confirmed @ slot **1045452** (sig `41GN8qGv…`)
- gateway **stake** tx (bond 5,000,000) confirmed @ slot **1051758**
- **slash-claim** tx confirmed @ slot **1051777**

---

## 7. Non-negotiable constraints (from the spec — DO NOT violate)

1. **No homemade cryptography.** All primitives go through `seal-crypto` (audited crates only).
2. **Never put plaintext, media, raw private keys, phone numbers, or readable metadata on WCAHT.**
   The chain sees only the seal leaf/root commitment.
3. **Keep WCAHT wallet keys separate from chat identity/device keys**, and separate again from a
   gateway's pre-confirmation signing key.
4. **FastSeal** must use off-chain payload prefetch + slashable gateway pre-confirmations;
   **StrictSeal** must wait for full L1 finality.
5. Bind every seal to `chain_id=7789`, ciphertext hash, recipient device commitment, sender
   signature, expiry, protocol version, and `SealMode`.
6. **No EVM contracts for v1** — native WCAHT RPC adapters with local mocks (that's `wcaht-seal-sdk`).
7. **Not audited.** No "unbreakable" / DRM / legal-enforceability claims.
8. Blockchain hygiene (from the WCAHT project): the live 3-validator cluster has zero fault
   tolerance — never rapid-restart nodes; don't run heavy/concurrent `cargo build`s on the N1 host;
   don't spam the chain (a couple of test txs is fine, floods are not).

---

## 8. What's DONE in detail (DSCP-2 inventory)

- **`SealMode { StrictSeal, FastSeal }`** on `SealLeaf` (covered by leaf hash + sender signature).
- **`PreConfirmation`** — a staked gateway's ed25519-signed promise over
  `(chain_id, seal_id, leaf_hash, gateway_id, sequenced_slot, expiry)` (domain `DSCP-2/PRECONF`).
  `Gateway::with_identity` adds `pre_confirm()` + `request_share_fast()`.
- **`try_open` / `try_open_dscp2`** — mode-aware release gate: L1 finality opens either mode; else
  StrictSeal stays locked, FastSeal opens on ≥`t` valid distinct-gateway pre-confs (share-holders,
  committing to THIS leaf, unexpired, signature-verified).
- **`detect_equivocation` → `SlashingEvidence`** — self-verifying fraud proof.
- **`GatewayRegistry`** — `stake` / `slash` / `is_active` / `filter_active` / `total_bonded`;
  `GatewayStanding { Active{bond} | Slashed{forfeited} | Unbonded }`. Slashing forfeits the bond and
  drops the gateway from every FastSeal quorum.
- **`DeliveryRelay` + `MockDeliveryRelay`** (seal-core) and an HTTP relay (`wcaht-seal-sdk/relay.rs`,
  `serve_relay` + `DeliveryRelayClient`) — ciphertext-only store-and-forward + prefetch.
- **`AnchorSigner`** — `transfer_tx` primitive → `anchor_tx` + `stake_anchor_tx`;
  `gateway_escrow_address(gateway_id)` = deterministic unspendable bond escrow.
- **FFI** — `ss_run_fast_demo` + `nativeRunFastDemo`; `ss_version` reports DSCP-2.
- **Apps** — StrictSeal/FastSeal toggle (iOS segmented `Picker`, Android `FilterChip` row);
  `SealMsg.mode` drives mode-aware status chips; `send()` parses both transcript shapes.

---

## 9. What's NEXT (Phase 3) — pick up here

Prioritized, with entry points. None of this is started.

1. **Production gateway services (multi-gateway).** Turn `Gateway` + `GatewayRegistry` into 3+
   independent long-running services that: hold shares, verify finality independently, issue
   pre-confs, and expose `request_share` / `request_share_fast` / `pre_confirm` over HTTP. Entry:
   mirror the pattern in `wcaht-seal-sdk/relay.rs` (tiny_http server) for a gateway server bin.
2. **Production delivery-relay service** — deploy `serve_relay` as a real hosted service (auth on
   fetch by mailbox tag, retention/expiry, size limits). Entry: `wcaht-seal-sdk/relay.rs`.
3. **On-chain stake custody that actually moves the bond.** Today `stake_anchor_tx` locks funds in
   a derived unspendable escrow and slashing is recorded/published on-chain, but forfeiture isn't
   programmatically enforced (no native gateway-registry tx type; EVM is disallowed for v1). Decide:
   (a) add a native SEAL/STAKE tx type to WCAHT runtime (big, touches consensus — coordinate with
   the `PoASy3` team), or (b) a trusted registry service that custodies bonds and forfeits on a
   finalized slash-claim. Until then, treat slashing as "provable + published", not "auto-forfeited".
4. **Device linking + proof screen** in the apps (show the seal proof / finality / pre-conf quorum).
5. **Media / groups / calls** — encrypted chunked images/voice/docs (`ContentType::Media/Document`
   already exists), MLS groups, `CALL_SESSION` (`ContentType::CallSession` exists as a stub).
6. **Real gateway staking economics** — min-bond calibration, partial slashing, unbonding periods.
7. **24h soak + external crypto/protocol audit** before any real use.

Design intent to preserve: FastSeal was slotted in behind the same `SealChain`/`try_open` seam
without a client rewrite. Keep new release gates composable the same way.

---

## 10. Gotchas / lessons

- **`ss_version` / `SealMsg.mode`:** the two FFI demos have DIFFERENT transcript shapes —
  StrictSeal = `before_finality`/`after_finality` + `shares_released`; FastSeal =
  `before_preconf`/`after_preconf_quorum` + `preconfs`. The app parsers handle both; keep that in
  sync if you change the demos.
- **N1 finality lag:** the local observer finalizes ~24–40 slots (~10–16s) behind wall-clock and its
  `recent_blockhash` can transiently 503 when it lags. Read finality from it, but submit elsewhere.
- **Don't trust a single `/health` read** on a WCAHT node right after a restart (stale-read fallback).
- **The mobile UI can't be compiled on the current dev machine** (no Xcode/Android SDK). The Swift/
  Kotlin is written against the tested FFI output but has not been compiled — do that in the IDEs.

---

## 11. Where else state lives

- **`README.md`** — user-facing overview, build, and the verified demo transcripts.
- **Claude auto-memory:** `PoASy3/.claude/…/memory/denvion-splitseal-app.md` (+ the `MEMORY.md`
  index line) — the running project record across sessions. Update it when big things land.
- **This file (`SKYREACH_DOC.md`)** — the authoritative task/status doc. Keep it current.
