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

**Status: DSCP-2 is feature-complete, and sealed MEDIA (photo/video) now works end to end.**
Protocol core, real on-chain anchoring, payload-prefetch delivery relay, gateway
staking/slashing, a dual-mode UI in both native apps, and chunked encrypted media with a
`/blob` store on the live relays are all built. **45 Rust tests green.** Three real
transactions landed on the live chain (see §6). Not audited; Phase 3 (production gateway
services, groups/calls, audit) is what remains.

⚠️ The **iOS media UI is written but has never been compiled** — it was authored on a Windows
machine with no Xcode. Build it on a Mac before trusting it.

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
| **Batched on-chain commitment (SEAL_ROOT) + sponsored fees** | ✅ done + LIVE on N5/N6/N7; every message is committed under a root anchored in a real tx, the app pays |
| **Treasury monitoring / alerting** | ✅ done; `:9300/health` → 503 on low treasury, anchor failure, or backlog overflow |
| Total Rust tests | **63 green** |
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
cargo test          # 63 tests green (crypto, threat matrix, DSCP-2, registry, relay, media, batching, SDK)
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
| `wcaht-seal-batcher [addr]` | the batching service itself (§6c); queue-only without a seed | `WCAHT_RPC` + `WCAHT_BATCHER_SEED` to anchor |
| `wcaht-seal-batch-demo [url]` | 6 messages → 1 root → 1 tx; verifies each proof as a recipient would, and that an unbatched message can't borrow the root | a running batcher |
| `wcaht-seal-stats [addr]` | live status dashboard (defaults to :9301, not :9300) | — |

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
- **Fee = `max(5_000, canonical_cu)` where `canonical_cu = declared_cu.max(type_floor)`**, and a
  Transfer's floor is 5_000 (`PoASy3/src/transaction/validation.rs`,
  `block_compute_budget.rs`). **A transfer costs 5_000 kak, not 200_000.**
  ⚠️ This doc previously said the minimum was 200_000. That was wrong, and it was wrong in the
  SDK too: `transfer_tx` *declared* 200_000 CU on every transfer, and since the declared value
  is what gets charged, it self-inflicted a **40× overcharge**. Fixed via `transfer_tx_with_cu`;
  anchors declare `ANCHOR_COMPUTE_UNITS = 5_000`. Don't "fix" a fee rejection by raising the
  declared CU — you are setting the price, not discovering it.
- **A transfer's amount has no dust floor** — consensus asks only for `amount > 0`. An anchor
  therefore sends **1 kak** (`ANCHOR_AMOUNT_KAK`), because the destination is hash-derived and
  nobody can ever spend from it. Total cost of an anchor: **5_001 kak**.
- **`recent_blockhash` is a 64-char HEX string** in the tx JSON (`deserialize_recent_blockhash`
  wants exactly 64 hex chars). Fetch it from `GET /blockchain/recent_blockhash`.
- **Confirming a tx: prefer the balance delta.** `GET /transaction/<base58-signature>` is
  documented to return `{"status":"confirmed","slot":N,…}`, but it has been observed returning
  `Transaction not found` on **every** node for txs that demonstrably landed (2026-08-01). Do
  not conclude a tx failed from that endpoint alone — check whether the balances actually moved.
- **`GET /balance/<addr>` can be stale for ~a minute** after a tx lands, and nodes disagree
  during that window (one read 0 while the other two read the correct value, then self-healed).
  Re-read before concluding divergence.
- **Canonical signing:** `AnchorSigner::canonical_bytes_v2` is a byte-exact replica of the runtime
  `Transaction::canonical_bytes()` (`TX::v2`) — see `PoASy3/src/transaction/transaction.rs:456`.
  If you change the tx shape, re-check it against that function.

**On-chain proof points (all from `CBXRVVK…` via validator N5, 2026-07-29):**
- seal **anchor** tx confirmed @ slot **1045452** (sig `41GN8qGv…`)
- gateway **stake** tx (bond 5,000,000) confirmed @ slot **1051758**
- **slash-claim** tx confirmed @ slot **1051777**

---

## 6b. Backend services on N5/N6/N7 + two-device testing

The apps talk to five stateless HTTP services (ciphertext + hashes only — **never** keys or
plaintext). They are DEPLOYED + LIVE on **all three** WCAHT nodes — N5 `139.99.150.23`,
N6 `51.79.176.134`, N7 `51.79.162.80` — each in a directory **separate from the validator**
(`~/skyreach/`, ports well clear of the node's 8901/8902). The apps ship ciphertext to every
relay and read from all of them, so any one node can be down and delivery still works.
**N1 runs none of these**, and is not in the apps' node list.

| service   | port | systemd unit         | role                                                  |
|-----------|------|----------------------|-------------------------------------------------------|
| relay     | 9200 | `skyreach-relay`     | ciphertext inbox (per mailbox tag) **+ `/blob` media chunk store** |
| gateway 1 | 9201 | `skyreach-gw1`       | holds share[0]; releases only after finality (425 until) |
| gateway 2 | 9202 | `skyreach-gw2`       | holds share[1]                                        |
| gateway 3 | 9203 | `skyreach-gw3`       | holds share[2]                                        |
| **batcher** | **9300** | **`skyreach-batcher`** | **commits leaves under a SEAL_ROOT and anchors it on-chain (§6c)** |
| status    | 9301 | `skyreach-stats`     | live status dashboard (**N5 only**)                   |
| directory | 9988 | `skyreach-directory` | `hash(phone) → contact card` (privacy-preserving)     |

> **N5/N7 name their single gateway `skyreach-gw` (not `gw1`); N6 runs all three** as
> `skyreach-gw1/2/3`. A rollout script that only touches `skyreach-gw` silently misses two
> gateways on N6 — restart all three.
>
> **The status dashboard is on 9301, not 9300.** It used to own 9300 and blocked the batcher
> there; both apps derive their batcher URL as `http://<node>:9300` with no way to configure
> it, so the dashboard is the service that moved. `ufw` denies new ports by default — a service
> can look perfectly healthy on `127.0.0.1` while being unreachable from outside
> (`sudo ufw allow 9300/tcp`).

Each unit is `Restart=always`; logs are in `~/skyreach/logs/`. Manage:
```bash
ssh ubuntu@51.79.176.134
sudo systemctl restart skyreach-relay skyreach-gw1 skyreach-gw2 skyreach-gw3 skyreach-directory
tail -f ~/skyreach/logs/relay.log
```
Redeploy after a Rust change. **Only N5 and N6 have cargo — N7 does not**, so build once and
copy the binary; all three are x86_64 / glibc 2.39 so one build serves them all. Build
`--release`: the deployed binaries are release (~3.3 MB), a debug build is ~72 MB.

```bash
# 1. build on N6 (stable toolchain)
# from denvion-splitseal/
tar czf /tmp/sk.tgz Cargo.toml Cargo.lock seal-crypto seal-core seal-ffi wcaht-seal-sdk
scp /tmp/sk.tgz ubuntu@51.79.176.134:sk.tgz
ssh ubuntu@51.79.176.134 'source ~/.cargo/env &&
  rm -rf ~/skyreach-src && mkdir ~/skyreach-src && tar xzf ~/sk.tgz -C ~/skyreach-src &&
  cd ~/skyreach-src && cargo build --release -p wcaht-seal-sdk --bin wcaht-seal-relay'
scp ubuntu@51.79.176.134:skyreach-src/target/release/wcaht-seal-relay /tmp/relay.new

# 2. roll it out — CANARY FIRST (N7, not the app default), verify, then N6 and N5
for h in 51.79.162.80 51.79.176.134 139.99.150.23; do
  scp /tmp/relay.new ubuntu@$h:relay.new
  ssh ubuntu@$h 'cd ~/skyreach/bin &&
    cp -a wcaht-seal-relay wcaht-seal-relay.bak-$(date +%Y%m%d-%H%M%S) &&   # rollback copy
    install -m 755 ~/relay.new wcaht-seal-relay &&
    sudo systemctl restart skyreach-relay'
  # verify before moving to the next node:
  curl -s http://$h:9200/blob/$(printf 'a%.0s' {1..64})   # → {"message":"no such blob"}
done
```

Restarting the relay is safe: the inbox is an append log reloaded on start, so queued
messages survive (verified — 25 items intact across all three restarts). Only restart the
units whose binary actually changed. **Rollback** = `install -m 755` the `.bak-*` copy and
restart.

---

## 6c. Batched on-chain commitment (SEAL_ROOT) + sponsored fees — LIVE

**Every message is committed to the chain, and the app pays for it, not the user.**

Users have no chain account, no balance and no key that could sign a transaction — chat
identity keys and wallet keys are deliberately separate (spec §5.2), so a user *cannot* be
charged even in principle. The fee comes from a Skyreach treasury account. This is gas
sponsorship, the same shape as a Solana fee payer or an ERC-4337 paymaster.

**One transaction per batch, not per message.** Leaves arriving in a window are committed under
a single merkle root; that root is anchored in one transaction. Each message still gets its own
signed leaf and its own inclusion proof.

```
many messages ──▶ batcher :9300 ──▶ seal_batch_root() ──▶ ONE anchor tx (root = destination)
                                                              │
recipient ◀── GET /proof/<seal_id> ── {leaf_hash, merkle_path, leaf_index, leaf_count, root}
                                       verified against the recipient's OWN leaf
```

- **No consensus change was needed.** The root becomes the transaction's *recipient address*,
  so a confirmed tx paying that address IS the chain attesting the root existed by that slot.
  A native `SEAL_ROOT` tx type was considered and **deferred**: it is a consensus change on a
  live 3-validator chain that has halted on a migration before, and its only real benefit is
  cost, not security.
- **The batcher is never trusted.** `verify_seal_inclusion` recomputes the root from the
  recipient's own leaf plus the path. A message that was never batched cannot borrow a root
  (there is a test for exactly this).
- **Odd nodes are promoted, not duplicated** — duplicating would let a forged leaf verify.
  `leaf_count` is part of the proof so level widths replay identically.
- **Cost: 5_001 kak per anchor**, flat, regardless of how many messages are in the batch (up to
  `WCAHT_BATCH_MAX`, default 10_000). Runway at the current treasury is ~200 million anchors.
- **A failed submit never loses messages** — the whole batch goes back on the queue and retries.

### Operating it

```bash
# env (in /home/ubuntu/skyreach/batcher.env, chmod 600 — NEVER in the unit file,
# systemd units are world-readable and this file holds the funded key)
WCAHT_RPC=http://127.0.0.1:8901
WCAHT_BATCHER_SEED=<32-byte hex seed of the treasury account>
WCAHT_BATCH_INTERVAL_MS=1000
WCAHT_BATCH_MAX=10000
WCAHT_TREASURY_WARN_KAK=1000000000   # optional; warn below this

curl -s http://<node>:9300/stats    # pending, batches, anchored, last_root, treasury_*
curl -s http://<node>:9300/health   # 200 healthy / 503 degraded  ← PAGE ON THIS
```

**Point monitoring at `/health`.** A batcher that cannot anchor fails *silently*: it keeps
accepting leaves and keeps answering `425 not anchored yet`, so nothing looks broken from the
outside. `/health` returns **503** with a readable problem list when the treasury is low, when
anchors are failing consecutively, or when the backlog overflowed.

Without a seed the batcher runs **queue-only** — it accepts leaves and submits nothing, and
says so on startup. That is the deliberate safe default: it will never pretend a message was
anchored.

`pending` is capped at 200k leaves. Before that cap existed, a dry treasury grew the queue
without bound until the OOM killer took the process — and every anchored proof held in memory
with it. On overflow the batcher returns 503 and the message is still **delivered**, just not
batched; both apps treat batching as best-effort (`submitLeafForBatching` is fire-and-forget,
and `fetchVerifiedProof` skips any batcher that doesn't answer).

**Verified on the live chain** from all three nodes: 6 sealed messages → one root → one
transaction → every message verified against that same root; the root address holds exactly
1 kak on all three validators and the treasury moved by exactly 5,001.

---

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
5. **Photo/video:** tap the camera in the composer. A seals + uploads encrypted chunks to every
   relay, then ships the manifest bundle. B opens the manifest, pulls the chunks it names,
   verifies each against the manifest, decrypts, and renders. Tap a video to play it fullscreen.

### Gotchas for the 2-device test
- **"Me" is a self-loopback, not another device.** The auto-created "Me" contact is linked to your
  OWN device key, so sealing to it ships to your own mailbox and your own poll echoes it right back —
  an on-device proof of the full pipeline, NOT a send to iOS. Use a REAL contact (the other phone).
- **Sender tagging is DONE** (this used to be the top follow-up). The sender signs with their
  stable chat identity and embeds their card, so the bundle carries `sender_id_pub`; the recipient
  routes each message to the chat whose `identity_pub` matches, and auto-creates the sender as a
  replyable contact. Inbound no longer leaks into every conversation.
- **Media needs the relay to have `/blob`.** If a photo fails to send, check the relay build:
  `curl http://<host>:9200/blob/$(printf 'a%.0s' {1..64})` must answer `no such blob`, not
  `not found` (that means an old binary). All three live nodes were updated 2026-08-01.
- **Both devices must point at the same backend.** Settings ▸ Server. Leave it at the default
  (`51.79.176.134`) to use the replicated live backbone; set any other host and the app pins to
  that single machine (relay 9200, gateways 9201-9203 on consecutive ports — dev only).

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

Prioritized, with entry points. Media (item 5) is **done**; the rest is not started.

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
5. **Media** — ✅ **DONE for photo + video** (see §6b "Sealed media"): `seal_media_with_mode` /
   `try_open_media` in `seal-core`, three `ss_*_media_*` FFI entry points, `/blob` on the relay,
   pickers + rendering in the Android app. Deployed to N5/N6/N7 and verified live.
   ▸ Remaining here: **voice notes and documents** (`ContentKind::Audio` / `File` already exist —
   only the app-side capture/render is missing), **blob GC after `destroy_at`** (chunks currently
   linger on the relay once the key is withheld), true **streaming** for very large files (the
   FFI reads the whole file into memory, capped at 64 MiB), and an **inbox cursor**
   (`GET /inbox/<tag>` still returns the whole mailbox on every 3s poll).
   ▸ Then: **MLS groups**, `CALL_SESSION` live calls (`ContentType::CallSession` exists as a stub).
6. **Real gateway staking economics** — min-bond calibration, partial slashing, unbonding periods.
7. **Batched on-chain commitment + sponsored fees** — ✅ **DONE and LIVE** (§6c). Every message is
   committed under a `SEAL_ROOT` anchored in one real transaction, paid by the treasury.
   ▸ Remaining here: a **treasury top-up runbook** (what to do when `/health` goes 503 — which
   account funds it, how much, who approves); **per-node treasury accounts** so one leaked key
   does not expose the whole budget (safe to split: the tx preimage has no nonce, so separate
   accounts cannot race); and deciding whether the ~200M-anchor runway needs an automatic
   refill at all.
8. **24h soak + external crypto/protocol audit** before any real use.

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
  ▸ The **batcher is in-memory too**: its `proofs` map does not survive a restart, so a proof
  fetched before the restart is unavailable after it. The anchor itself is on-chain and
  permanent — only the convenience lookup is lost.
- **`/transaction/<sig>` cannot be used to confirm a tx** — it returned "Transaction not found"
  on every node for anchors that had demonstrably landed (2026-08-01). **Confirm by balance
  delta.** Related: `/balance` can be stale for ~a minute after a tx and nodes will disagree
  during that window; re-read before calling it divergence.
- **Don't raise declared compute units to clear a fee rejection.** The declared CU *is* the
  price (`fee = max(5_000, declared.max(floor))`). Declaring 200_000 on a transfer is a 40×
  self-inflicted overcharge, and this doc taught that mistake for months.
- **A service bound on `0.0.0.0` can still be unreachable** — `ufw` denies new ports by default,
  so it looks perfectly healthy over `127.0.0.1` and times out from anywhere else.
- **Check what already owns a port before deploying to it.** The batcher's first N5 rollout
  crash-looped on `Address already in use` because the status dashboard had held :9300 for two
  days. `sudo ss -lntp | grep <port>`.
- **A sponsored service fails silently when its funding runs out.** The batcher kept accepting
  messages and answering `425 not anchored yet` — nothing looked broken. Any component that
  spends from a balance needs an endpoint that goes *unhealthy*, not just a log line.

---

## 11. Where else state lives

- **`README.md`** — user-facing overview, build, and the verified demo transcripts.
- **Claude auto-memory:** `PoASy3/.claude/…/memory/denvion-splitseal-app.md` (+ the `MEMORY.md`
  index line) — the running project record across sessions. Update it when big things land.
- **This file (`SKYREACH_DOC.md`)** — the authoritative task/status doc. Keep it current.
