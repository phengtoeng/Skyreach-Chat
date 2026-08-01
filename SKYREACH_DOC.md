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
| Native apps (SwiftUI + Kotlin/Compose) with StrictSeal/FastSeal toggle | ✅ done; iOS builds + runs via `xcodegen` (§6b), two-device send/receive verified live |
| Sealed media (photo/video): chunked + encrypted manifest + `/blob` relay store | ✅ done; verified end-to-end on the live backbone from Android |
| Total Rust tests | **45 green** |
| iOS media UI | ⚠️ written, NOT yet compiled — needs a Mac |
| Production gateway services, device linking, groups/calls, audit | ❌ Phase 3 |

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

# iOS: generate the project (project.yml wires the FFI + builds it as a prebuild step)
brew install xcodegen && cd ios-app && xcodegen generate
```

The iOS `.xcodeproj` is generated, not committed — see §6b for the full build. `README.md`'s
manual Xcode wiring is superseded by `ios-app/project.yml`.

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

## 6b. Backend services on N6 + two-device testing

The apps talk to five stateless HTTP services (ciphertext + hashes only — **never** keys or
plaintext). They are DEPLOYED + LIVE on WCAHT node **N6** (`51.79.176.134`), in a directory
**separate from the validator** (`~/skyreach/`, ports well clear of the node's 8901/8902):

| service   | port | systemd unit         | role                                                  |
|-----------|------|----------------------|-------------------------------------------------------|
| relay     | 9200 | `skyreach-relay`     | ciphertext inbox (per mailbox tag) **+ `/blob` media chunk store** |
| gateway 1 | 9201 | `skyreach-gw1`       | holds share[0]; releases only after finality (425 until) |
| gateway 2 | 9202 | `skyreach-gw2`       | holds share[1]                                        |
| gateway 3 | 9203 | `skyreach-gw3`       | holds share[2]                                        |
| directory | 9988 | `skyreach-directory` | `hash(phone) → contact card` (privacy-preserving)     |

Each unit is `Restart=always`; logs are in `~/skyreach/logs/`. Manage:
```bash
ssh ubuntu@51.79.176.134
sudo systemctl restart skyreach-relay skyreach-gw1 skyreach-gw2 skyreach-gw3 skyreach-directory
tail -f ~/skyreach/logs/relay.log
```
Redeploy after a Rust change (N6 has a minimal toolchain, builds natively):
```bash
# from denvion-splitseal/
tar czf /tmp/sk.tgz Cargo.toml Cargo.lock seal-crypto seal-core seal-ffi wcaht-seal-sdk
scp /tmp/sk.tgz ubuntu@51.79.176.134:~/ && ssh ubuntu@51.79.176.134 '
  rm -rf ~/skyreach-src && mkdir ~/skyreach-src && tar xzf ~/sk.tgz -C ~/skyreach-src &&
  source ~/.cargo/env && cd ~/skyreach-src &&
  cargo build -p wcaht-seal-sdk --bin wcaht-seal-relay --bin wcaht-seal-gateway --bin wcaht-seal-directory &&
  cp target/debug/wcaht-seal-{relay,gateway,directory} ~/skyreach/bin/ &&
  for n in relay gw1 gw2 gw3 directory; do sudo systemctl restart skyreach-$n; done'
```

### Sealed media (photo / video)

An image or video is **never** carried in the envelope and never rests anywhere readable.
The envelope carries an **encrypted `MediaManifest`**; the pixels go out as separately
encrypted chunks the relay stores as opaque blobs. The chain sees only `manifest_root` —
32 bytes, with no filename, mime, size, dimensions or thumbnail in it.

```
sender ──encrypted chunks────────▶ relay    PUT /blob/<ciphertext-hash>   (opaque)
       ──encrypted manifest──────▶ relay    POST /inbox/<mailbox tag>
       ──signed leaf────────────▶ WCAHT    manifest_root, 32 bytes
       ──key shares─────────────▶ gateways encrypted to the recipient device
```

Every chunk key is a KDF subkey of `K_content`, which is Shamir-split across the gateways.
So the recipient can pre-download an entire video that stays cryptographically **unopenable**
until the release gate opens — the same gate that drives time-reveal and time-destroy.

Relay wire contract (raw binary, not JSON; 8 MiB cap; idempotent):
```text
PUT  /blob/<64-char lowercase hex>   body = nonce||ciphertext  → {"status":"stored"|"exists"}
GET  /blob/<64-char lowercase hex>   → the bytes, or 404 {"message":"no such blob"}
HEAD /blob/<64-char lowercase hex>   → 200 / 404
```
The relay **verifies the content address** — bytes must hash to the name they claim — so it
cannot substitute a chunk. It holds no key and can never open one. The name is validated as
strict 64-char lowercase hex because it becomes a filename (path-traversal gate). Blobs are
**not** gossiped between relays: the client uploads to every relay itself.

Receiving is two steps, because the chunk list lives *inside* the encrypted manifest:
`ss_open_media_info` (open manifest → learn which chunks to fetch) → download →
`ss_open_media_file` (verify each chunk against the manifest, decrypt, reassemble).

Deployed to N5 + N6 + N7 on 2026-08-01. Verified live: a 184,808-byte photo round-tripped
byte-identically, with the relay's stored copy at 7.997 bits/byte entropy and no image magic.

### Configurable backend in the app
Both apps default their server host to **N6** (`51.79.176.134`) — nothing to set up to use the
live backend. To point elsewhere (e.g. `10.0.2.2` for services on the emulator's own machine,
`127.0.0.1` on the iOS sim), open **Settings ▸ Server**, type the IP/host, Save. One host drives
all five URLs; ports are fixed.

### iOS build (on the Mac, after `git pull`)
The Xcode project is **generated from `ios-app/project.yml`** by [XcodeGen] — the `.xcodeproj` is
gitignored, so there is no manual Xcode wiring to redo. `project.yml` already carries the linker
flags (`-force_load`), the bridging header, the header search path, the Info.plist keys, and a
preBuildScript that builds the Rust core, so a clean checkout is:

```bash
brew install xcodegen                 # once
cd ios-app && xcodegen generate       # writes SplitSeal.xcodeproj + SplitSeal/Info.plist
xcodebuild -project SplitSeal.xcodeproj -scheme SplitSeal \
  -sdk iphonesimulator -derivedDataPath DerivedData build
```

`scripts/build_rust_ios.sh` runs as a prebuild step and lipos the `x86_64-apple-ios` +
`aarch64-apple-ios-sim` static libs into `ios-app/build/libseal_ffi_sim.a`. For a device build
swap in `aarch64-apple-ios`. **Edit `project.yml`, never the generated project** — `xcodegen
generate` overwrites both the `.xcodeproj` and `SplitSeal/Info.plist`.

Android side: on a FRESH (non-emulator) checkout, rebuild the `.so` once —
`cargo ndk -t x86_64 -o android-app/app/src/main/jniLibs build --release -p seal-ffi`.

[XcodeGen]: https://github.com/yonaskolb/XcodeGen

> **Do not drop `NSAppTransportSecurity ▸ NSAllowsArbitraryLoads` from `project.yml`.** The five
> services are plain http on a raw IP, so without it iOS silently refuses every `URLSession`
> call and the app looks broken in a very confusing way: sends hang on "Sealing…" forever and
> the inbox poll returns nothing, while adding a contact still works (add-by-code is a purely
> local `ss_parse_card` call, no network). `httpPost`/`httpGet` swallow errors with `try?`, so
> nothing is logged. A domain exception can't replace it — the host is user-configurable in
> Settings. Revisit once the backend has TLS.

### How the two-device test works (Android ↔ iOS)
1. Both apps default to the **N6** backend — no setup (Settings ▸ Server to change; must match).
2. A and B **add each other**. Easiest path (works with no camera): on each phone open
   **My Denvion ID**, copy the `denvion:…` code, send it across; on the other phone tap
   **New Contact ▸ Paste ▸ Add by code ▸ ✓ Save**. (QR scan / phone-number directory also work.)
3. A opens the B conversation and sends → the app seals to B's device key, ships the ciphertext
   `{seal_id, bundle}` to the relay under **B's mailbox tag** + the 3 shares to the gateways.
4. B's app polls **its own** mailbox tag (`ss_mailbox_tag(my device_pub)`) every ~3 s, collects the
   released shares, opens locally, shows it. On A's side you only see the outgoing "🔒 Sealed for B".

### Gotchas for the 2-device test
- **"Me" is a self-loopback, not another device.** The auto-created "Me" contact is linked to your
  OWN device key, so sealing to it ships to your own mailbox and your own poll echoes it right back —
  an on-device proof of the full pipeline, NOT a send to iOS. Use a REAL contact (the other phone).
- **Messages aren't tagged by sender yet** (TOP FOLLOW-UP). Received messages are keyed by the
  recipient's mailbox tag (the per-seal sender id is ephemeral), so ALL inbound — including "Me"
  self-test messages — shows in EVERY real conversation. Before a clean iOS test either **Clear
  storage** on the app (fresh identity + empty inbox; re-share codes), OR do the **sender-tagging
  fix**: have the sender sign with their stable identity so the recipient routes each message to the
  right chat (small change: FFI `ss_seal_shippable` takes the sender's identity seed + the recipient
  filters by `sender_id_pub` == a contact's `identity_pub`; mirror on iOS + Android).

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
- **iOS ATS silently kills the whole app.** Plain-http backend + no `NSAllowsArbitraryLoads` =
  every `URLSession` call fails before it leaves the device, with no log line, because
  `httpPost`/`httpGet` use `try?`. It presents as "sending is stuck / I never receive anything"
  while contact-add still works. See the warning in §6b. First thing to check on any iOS
  networking bug: `plutil -extract NSAppTransportSecurity xml1 -o - <App>.app/Info.plist`.
- **Failed sends still look sent.** `ConversationView.send()` writes the outgoing message to the
  transcript via `Store.addThreadMsg` BEFORE it knows whether `shipSeal` succeeded, then on
  failure leaves the bubble on "Sealing…" forever with no retry and no error. A message in your
  own transcript is NOT evidence it reached the relay — check the peer's mailbox on the relay.
- **The relay + gateways are in-memory only** (`relay.rs`, `gateway_service.rs` both use a plain
  `HashMap`). Restarting a `skyreach-*` unit on N6 drops every queued ciphertext and every
  finalized share, so undelivered messages are gone for good. Fix before any real use.

---

## 11. Where else state lives

- **`README.md`** — user-facing overview, build, and the verified demo transcripts.
- **Claude auto-memory:** `PoASy3/.claude/…/memory/denvion-splitseal-app.md` (+ the `MEMORY.md`
  index line) — the running project record across sessions. Update it when big things land.
- **This file (`SKYREACH_DOC.md`)** — the authoritative task/status doc. Keep it current.
