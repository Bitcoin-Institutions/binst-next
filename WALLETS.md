# Sovereign wallets for BINST Next: options and a phased plan (2026-11-16)

**Status (2026-11-17):** Diego picked W0 + W1 + W2. They are built on branch `wip/sovereign-wallets` (see "Built" at the end); W3–W6 are not.

**Goal:** rely less on commercial browser-extension wallets (UniSat, Xverse, Leather, MetaMask). Let people use their own desktop wallet apps, hardware or air-gapped signers, and their own Bitcoin Core and ord nodes.

**Short answer:** the way in is not a new extension. Two changes do it:
1. Hand the PSBT to the user's own software (file or animated QR) and take the signed PSBT back.
2. Change the reveal so it no longer needs the wallet to sign a non-standard tapscript leaf. Today no sovereign signer can sign that leaf (see A0).

---

## 1. What the flows needed from a wallet (before W0)

This table describes the code as it was on 2026-11-16. Section "Built" at the end says what changed.

Code: `src/inscribe.rs` (sign and broadcast), `src/txbuilder.rs` (commit/reveal), `src/wallet.rs` (detection), `src/connect.rs` (L1/L2 connect), `src/members.rs` (BIP-322), `src/citrea.rs` (L2 calls). It was copied from `binst-pilot-webapp@dda2d1a`.

| Flow | L1 needs | L2 needs |
|---|---|---|
| Connect | Address (P2TR) plus the **untweaked x-only key** (`getPublicKey`). That key becomes the institution's `admin`, and `load.rs` derives the admin address from it. | — |
| Create institution (`actions.rs:51`) | UTXOs of the address (mempool.space). **PSBT 1, commit:** key-path P2TR funding inputs. **PSBT 2, reveal:** one **script-path** input. The leaf is `<admin_pubkey> OP_CHECKSIG OP_FALSE OP_IF "ord" … OP_ENDIF` and the internal key is NUMS. UniSat signs it with `disableTweakSigner: true`. Broadcast: `POST mempool.space/testnet4/api/tx`. | — |
| Define process (`actions.rs:89`) | As above, plus the **parent sat**. Reveal input 0 is a key-path spend of the institution's UTXO at the holder address; input 1 is the script-path commit output. | — |
| Nostr binding (`members.rs:142`) | `signMessage(msg, "bip322-simple")` from the holder address. | — |
| Start instance / execute step | — | EIP-1193 `eth_requestAccounts`, `wallet_switchEthereumChain` / `wallet_addEthereumChain` (Citrea testnet, 5115), `eth_sendTransaction`. Reads go over HTTP to `rpc.testnet.citrea.xyz`. |

PSBT gaps that matter for any non-extension signer (`inscribe.rs:672-722`, `txbuilder.rs:417`):
- The reveal's `tap_key_origins` is keyed on the **output** key with a zero fingerprint and an empty path. A hardware or descriptor signer finds no key of its own in it and signs nothing.
- No `PSBT_IN_TAP_INTERNAL_KEY`, no `PSBT_IN_TAP_MERKLE_ROOT`, and no BIP-32 derivations on the commit's funding inputs or on the parent input. Signers that match on derivation (Coldcard, Jade, Ledger, Trezor, BitBox) need these. Sparrow and Core can fill them in only for scripts in their own wallet.

## 2. The blocker: nobody but extension wallets can sign the reveal leaf (A0)

- **Bitcoin Core** signs tapscript leaves only if the leaf parses as miniscript (`SignTaprootScript` → `miniscript::FromScript`; [sign.cpp](https://github.com/bitcoin/bitcoin/blob/master/src/script/sign.cpp)). The ord envelope (`OP_FALSE OP_IF …`) is not miniscript, so `walletprocesspsbt` cannot sign it.
- **Ledger** signs registered BIP-388 wallet policies only: `tr(KP,TREE)` with leaves made of miniscript or `multi_a` ([app-bitcoin-new wallet.md](https://github.com/LedgerHQ/app-bitcoin-new/blob/master/doc/wallet.md), [BIP-388](https://github.com/bitcoin/bips/blob/master/bip-0388.mediawiki)). Arbitrary script is not listed.
- **Trezor**: Taproot support covers spending from Taproot UTXOs, plus pre-signed external Taproot inputs ([firmware CHANGELOG](https://github.com/trezor/trezor-firmware/blob/main/core/CHANGELOG.md)). I found no tapscript-leaf signing.
- **Jade**: BIP-86 single-key P2TR ([CHANGELOG](https://github.com/Blockstream/Jade/blob/master/CHANGELOG.md)). Its miniscript support (v1.0.30, [Blockstream](https://blog.blockstream.com/blockstream-jade-adds-support-for-miniscript/)) is not tapminiscript ([Delving, 2026-04](https://mirror.b10c.me/delvingbitcoin-org/2026-04-28-qr-based-signing-flow-payloads-in-miniscript-context-id2464.html)).
- **Coldcard** Edge 6.6.0X supports MiniTapscript, up to 8 leaves ([coldcard.com/firmware](https://coldcard.com/firmware)). That is still miniscript, not an envelope.
- **SeedSigner**: its README still says "Taproot not quite yet supported" ([README](https://github.com/SeedSigner/seedsigner)). It may lag the code; not verified.
- **Passport**: full Taproot since v2.3.0, key-path ([NoBS](https://www.nobsbitcoin.com/foundation-passport-v2-3-0)).

**How `ord` solves this:** `ord wallet inscribe` puts a **fresh random key** in the leaf, signs the reveal itself, and backs the key up into Bitcoin Core as `rawtr(<wif>)` so a failed reveal can be recovered ([ord src/wallet/batch/plan.rs](https://github.com/ordinals/ord/blob/master/src/wallet/batch/plan.rs), around lines 368, 595 and 676). Bitcoin Core only signs ordinary key-path inputs.

**Proposal A0, the enabler for everything below:** do the same in BINST Next.
- The browser generates an ephemeral key for the leaf and signs the script-path input in WASM. It keeps the key, encrypted in IndexedDB and offered as a download, until the reveal confirms.
- The user's wallet signs only key-path inputs: the commit's funding inputs and the reveal's parent-sat input. Every Taproot-capable signer can do that.
- `admin` stays the user's own key in the JSON body. Readers use the body field (`load.rs:53,67`), not the leaf key.
- Provenance stays sound: it comes from spending the parent sat, which the holder signs.
- **Needs a protocol review before it lands** (`bitcoin-kit:bitcoin-protocol-reviewer`). Check: does anything in `binst-protocol` (envelope parser, scanner) or the book treat the leaf key as the admin's signature? The leaf key currently shows the admin signed the reveal; afterwards only the parent spend does, and a root institution has no parent. If a root inscription must prove its admin key, add a key-path or BIP-322 proof as a separate step, or keep the old leaf as an "extension wallet" mode.
- A0 alone also gives UniSat users a reveal with no `disableTweakSigner` special case.

## 3. Options, with the browser → wallet channel and what works

Signing works the same on testnet3 and testnet4: the address prefix (`tb1p`), the xpub versions (`tpub`) and coin type 1' are shared. A signer that does "testnet" signs testnet4 PSBTs. Only coordinators that sync the chain need testnet4 support (my reasoning, not a vendor claim).

### 3.1 PSBT hand-off: file and animated QR (no extension)
- **Channel:** the page offers `commit.psbt` and `reveal.psbt` (BIP-174 binary, plus base64 to copy). It shows them as an animated QR and accepts the signed PSBT back by file upload, paste, or camera scan.
- **QR formats:**
  - **BBQr** (Coinkite; header `B$` + encoding + type `P`/`T` + total + index; H/2/Z encodings) for Coldcard Q. Sparrow picks BBQr or UR per device ([BBQr spec](https://github.com/coinkite/BBQr/blob/master/BBQr.md), [Coldcard air-gap](https://coldcard.com/learn/advanced-concepts/air-gap-signing-methods)).
  - **UR `crypto-psbt`** (Blockchain Commons, fountain codes) for Keystone, Passport, SeedSigner, Jade and Sparrow ([BCR-2020-006](https://github.com/BlockchainCommons/Research/blob/master/papers/bcr-2020-006-urtypes.md), [animated QRs](https://developer.blockchaincommons.com/animated-qrs/)).
- **Rust crates (keeps the WASM-first rule):** `bbqr` 0.6.0 ([SatoshiPortal/bbqr-rust](https://github.com/SatoshiPortal/bbqr-rust)), `ur` 0.5.2 ([ur-rs](https://github.com/dspicher/ur-rs/)), and `rqrr` 0.11.0 for decoding camera frames in WASM ([rqrr](https://github.com/WanzenBug/rqrr)). `getUserMedia` plus a canvas gives the frames. This avoids `BarcodeDetector`, which is not in every browser ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/Barcode_Detection_API)).
- **Who signs:**
  - **Sparrow:** testnet4 since 1.9.1 ([NoBS](https://nobsbitcoin.com/sparrow-wallet-v1-9-1)). It is also the coordinator for USB hardware wallets, so it covers Ledger, Trezor, BitBox and Jade without any WebHID code.
  - **Bitcoin Core GUI:** testnet4 since 28.0 ([release notes](https://github.com/bitcoin/bitcoin/blob/master/doc/release-notes/release-notes-28.0.md)); loads and signs PSBTs from file.
  - **Electrum:** testnet4 since 4.5.6 ([NoBS](https://www.nobsbitcoin.com/electrum-v4-5-6/)). Whether it creates P2TR wallets is reported by third parties ([Spark tracker](https://www.spark.money/tools/bitcoin-taproot-wallet-support)); not verified.
  - **Coldcard** (BBQr or microSD), **Keystone, Passport, Jade** (UR).
- **Works for:** commit and reveal (key-path inputs) once A0 lands. Without A0, none of these can sign the reveal.
- **Also needed:** the user registers a watch-only account once: an xpub or a `tr([fp/86'/1'/0']tpub…/<0;1>/*)` descriptor, pasted or scanned. That lets the page find UTXOs, write correct BIP-371 derivations, and pick the `admin` key without an extension.
- **Message signing:** BIP-322 is the weak spot.
  - Bitcoin Core has no BIP-322 RPC (legacy `signmessage` only).
  - Spike: hand the BIP-322 `to_sign` virtual transaction over as a PSBT, then rebuild the simple signature from its witness. Some signers may refuse a transaction that spends a non-existent `to_spend`. Test per device.
- **Sovereignty gained:** high. **Effort:** M (A0, PSBT fields, a hand-off dialog, QR in and out).

### 3.2 The user's own Bitcoin Core, through a tiny local bridge
- A browser page can't call bitcoind: Core sends no CORS headers (both proposals were closed unmerged: [#1487](https://github.com/bitcoin/bitcoin/pull/1487), [#12040](https://github.com/bitcoin/bitcoin/pull/12040)), and the page must never hold RPC credentials.
- **Design, `binst-bridge`** (Rust binary, run by the user):
  - Listens on `127.0.0.1` only. CORS is allowed for exactly the BINST origins (`binst-next.vercel.app`, `binst.orchos.xyz`, `localhost:8090`).
  - Needs a random per-launch token that the user pastes into the page once.
  - Reads the cookie file itself, so the RPC password never leaves the machine.
  - Allowlist: `getblockchaininfo`, `listunspent`, `getaddressinfo`, `walletprocesspsbt` (sign=true, finalize=false), `testmempoolaccept`, `sendrawtransaction`. Refuses everything else, and refuses unless `chain == testnet4|signet|regtest`.
  - Each `walletprocesspsbt` and `sendrawtransaction` asks for a y/N in its own terminal. Optional for testnet, documented.
  - Optionally passes `GET` through to the user's ord server (3.3) and esplora (3.4), which fixes their CORS too.
- **Browser caveats:**
  - Chrome 142+ asks the user's permission for public-site → loopback requests (Local Network Access); `targetAddressSpace` is the documented hint ([Chrome blog](https://developer.chrome.com/blog/local-network-access)).
  - Loopback is a potentially trustworthy origin, so `http://127.0.0.1` from an https page passes mixed-content checks in Chromium and Firefox. Safari not verified.
  - Running the app from `localhost:8090` sidesteps both.
- **Works for:**
  - Commit and the reveal's parent input (key-path) with a descriptor wallet holding the keys. The leaf needs A0 (Core can't sign it).
  - Chain reads and broadcast from the user's own node.
  - Not BIP-322.
- **Zero-install fallback:** the page shows the exact `bitcoin-cli -testnet4 walletprocesspsbt <base64>` and `sendrawtransaction <hex>` lines to copy, plus a paste box for the result.
- **Sovereignty:** highest (own node, own keys). **Effort:** M (the bridge is about 300 lines of Rust; the page side reuses 3.1's "signed PSBT in" path).

### 3.3 The user's own ord server: indexer, and `ord wallet` hand-off
- **Indexer:** `IndexerBackend::OrdServer` already exists in `src/indexer.rs` but is not wired.
  - ord returns JSON with `Accept: application/json`.
  - `/address/<addr>` needs `--index-addresses` and returns `outputs`, `inscriptions`, `sat_balance`, `runes_balances`. `/inscription/<id>` carries `parents` and `children`. `/output/<outpoint>` gives the inscription location ([ord API guide](https://github.com/ordinals/ord/blob/master/docs/src/guides/api.md)).
  - The guide says nothing about CORS, so reach it same-origin (dev proxy) or through the bridge.
  - Gains: no UniSat key, no 2,000-a-day quota, and ord's own `children` replace the "children at the admin address" workaround.
  - Diego's home testnet4 ord sits on :8080 through the tunnel. Whether it runs with `--index-addresses` is not checked.
- **CLI signing:** the page writes the BINST JSON and shows the command: `ord --testnet4 wallet inscribe --fee-rate N --file binst-<slug>.json --metaprotocol binst [--parent <institution-id>]`. `--metaprotocol` and `--parent` exist ([inscribe.rs](https://github.com/ordinals/ord/blob/master/src/subcommand/wallet/inscribe.rs)), and `--dry-run` previews. ord signs the reveal with its own key and bitcoind signs the rest. The page then watches for the inscription.
  - Caveat 1: the envelope ord builds must match what BINST readers expect (tag 7 `binst`, content type, `admin` = a key of this ord wallet). Check with a regtest inscription before offering it.
- **Works for:** create institution and define process, if the ord wallet holds the parent. Not L2, not BIP-322.
- **Sovereignty:** high. **Effort:** S (indexer switch and settings) plus S (command generator).

### 3.4 Self-hosted mempool or esplora for reads and broadcast
- `MEMPOOL_BASE` is a constant in `inscribe.rs:32`. Make it a setting: mempool.space (default), a user URL (their own mempool/esplora: `/address/:a/utxo`, `/tx/:txid`, `POST /tx`), or the bridge.
- mempool.space sends `access-control-allow-origin: *` (checked today); a self-hosted instance must be configured to do the same, or be reached through the bridge.
- Browsers cannot speak Electrum's TCP protocol, so an Electrum server (electrs, Fulcrum) is usable only through esplora's HTTP API or the bridge.
- **Sovereignty:** medium (privacy, no third-party broadcast). **Effort:** S.

### 3.5 Hardware wallets directly from the page (WebHID / WebUSB / Web Serial)
- Chromium only. Firefox and Safari don't ship these APIs ([WebHID](https://developer.mozilla.org/en-US/docs/Web/API/WebHID_API), [WebUSB](https://developer.mozilla.org/en-US/docs/Web/API/WebUSB_API), [Web Serial](https://developer.mozilla.org/en-US/docs/Web/API/Web_Serial_API)).
- **Ledger:** `ledger-bitcoin` client over WebHID. Policy `tr(@0/**)` for key-path ([npm](https://www.npmjs.com/package/ledger-bitcoin)). Miniscript since app 2.1.0; the `a:` fix is in 2.1.2 ([LSB-019](https://donjon.ledger.com/lsb/019/)). Reveal leaf: no.
- **Trezor:** `@trezor/connect-web` opens Trezor's hosted popup (connect.trezor.io), so a third-party page sits in the path. Key-path only.
- **BitBox02:** `bitbox-api` 0.13.0 is a Rust crate with a WASM/WebHID npm build, a natural fit for this stack ([bitbox-api-rs](https://github.com/BitBoxSwiss/bitbox-api-rs/)). Key-path assumed; policy support not verified.
- **Jade:** USB serial with a CBOR RPC; the official client is Python (`jadepy`). No maintained browser library found. Jade does QR well, so 3.1 covers it.
- **Works for:** key-path inputs only, so it needs A0. Sparrow (3.1) already reaches all of these devices.
- **Sovereignty:** high. **Effort:** M–L per vendor. **Worth it only after 3.1**, for people who don't want a desktop coordinator.

### 3.6 L2 (Citrea testnet, EIP-1193): brief
- **Frame** (desktop EVM wallet): archived on GitHub; last release v0.6.11 on 2025-02-03 ([floating/frame](https://github.com/floating/frame)). Drop it.
- **Hardware-backed keys** through an extension: MetaMask or Rabby paired with Ledger, Trezor or Keystone (QR). The key is on the user's device, but an extension is still in the path. Effort: none (works today).
- **Local key through the bridge:** the bridge exposes `eth_accounts` and `eth_sendTransaction` for chain 5115 only, signing with a Foundry keystore (`cast wallet`) after a terminal y/N. The page gets an EIP-1193 shim (`window.__binstBridge`) that is listed in the picker. Effort: S on top of 3.2.
- **Own RPC:** let the user set the Citrea RPC URL (their own Citrea full node, which syncs from their own bitcoind). Effort: S.
- **The sovereign end state is R7:** steps are authorized by a BIP-322 or Schnorr signature from the Bitcoin key (contracts exist, not deployed). Then any relayer can submit, and the L2 wallet stops mattering. Signing that message from a hardware wallet runs into the same BIP-322 limit as in 3.1.

### 3.7 The wallets in Diego's list (2026-11-16)
| Wallet | Kind | Fits "sovereign"? | Note |
|---|---|---|---|
| Xverse, UniSat | Extension | No (already covered) | Xverse dropped testnet4; UniSat is today's working L1 wallet. |
| Leather | Extension (the old Hiro desktop app is gone) | No | Detected already. Testnet4 not verified. |
| Phantom, OKX | Multi-chain extensions | No | Commercial; testnet4 and script-path signing not verified; low priority. |
| Ordinals Wallet | Web wallet | No | Mainnet-oriented web app; no PSBT hand-off from other sites found. |
| Best Wallet | MPC mobile app | No | MPC with a vendor; no PSBT hand-off. |
| **Sparrow** | Desktop app | **Yes** | Through 3.1; testnet4 since 1.9.1; coordinator for hardware wallets. |
| **Bitcoin Core + ord** | Own node | **Yes** | Through 3.2 / 3.3. |
| **Electrum** | Desktop app | Yes, if P2TR | Through 3.1; testnet4 since 4.5.6; P2TR not verified. |

## 4. Ranking: sovereignty gained vs. effort

| Rank | Option | Sovereignty | Effort | Flows covered | Depends on |
|---|---|---|---|---|---|
| 1 | **A0 ephemeral reveal key** + correct BIP-371 PSBT fields | enabler | S–M | create, define | protocol review |
| 2 | **3.1 PSBT file + BBQr/UR QR** + watch-only descriptor | high | M | create, define (not BIP-322) | A0 |
| 3 | **3.3a own ord as indexer** + **3.4 own esplora** (settings) | medium | S | all reads, broadcast | CORS or bridge |
| 4 | **3.2 `binst-bridge`** to Core (+ ord/esplora pass-through, + L2 local key) | highest | M | create, define, reads, broadcast, L2 steps | A0 |
| 5 | **3.3b `ord wallet inscribe` command generator** | high | S | create, define | regtest check that ord's envelope reads as BINST |
| 6 | BIP-322 via a `to_sign` PSBT | medium | S spike | Nostr binding, later R7 | per-device testing |
| 7 | 3.5 direct WebHID (BitBox02 first, then Ledger) | high | M–L each | key-path inputs | A0; Chromium only |
| 8 | L2: own RPC setting | low–medium | S | L2 reads | — |

Only some flows:
- BIP-322 (Nostr binding) stays extension-only until spike 6 works.
- L2 writes stay on an EVM wallet until R7 is deployed, or until the bridge's local key (3.6).

## 5. Phased plan (each phase in a worktree; tests and a regtest run before any testnet4 broadcast, which needs Diego's yes)

| Phase | Content | Files | Checks |
|---|---|---|---|
| W0 | Protocol review of A0 (reviewer agent), then A0. Ephemeral leaf key signed in WASM, kept encrypted until confirmation, downloadable recovery. Proper `tap_internal_key`, `tap_merkle_root`, `tap_bip32_derivation` on every input. Keep the extension-mode leaf behind a flag if the review says so. | `txbuilder.rs`, `inscribe.rs`, new `signer.rs` | Unit tests: the leaf signature verifies, the PSBT round-trips. A regtest commit + reveal (+ child with parent sat) signed by `walletprocesspsbt` for key-path inputs. ord on regtest shows the parent/child link. |
| W1 | "Sign elsewhere": signer kinds in the picker (Extension, PSBT file, QR, Bitcoin Core). Watch-only descriptor/xpub import. Hand-off dialog with download, copy, animated QR (BBQr for Coldcard, UR otherwise), and camera/file/paste return. | `connect.rs`, `session.rs`, new `handoff.rs`, `qr.rs`, `render/handoff.rs` | Round-trip with Sparrow on testnet4 (Diego signs). Camera decode at phone width. QR frames decode in tests with `bbqr`/`ur`. |
| W2 | Settings: indexer (UniSat proxy or own ord URL), esplora URL, Citrea RPC URL. Wire `IndexerBackend::OrdServer`. Dev proxy routes to the home ord through the tunnel. | `indexer.rs`, `inscribe.rs`, `citrea.rs`, `scripts/dev-proxy.mjs` | Featured tree loads from the home ord (`--index-addresses` confirmed first). |
| W3 | `binst-bridge` (new small crate, own repo or `tools/`): loopback, token, origin allowlist, method allowlist, chain guard, cookie auth, terminal confirm. Optional ord/esplora pass-through and Citrea local-key signer. | new crate; `handoff.rs` gets a "Bitcoin Core (bridge)" target | Security audit agent. Regtest end-to-end. Chrome LNA prompt seen. Refuses mainnet. |
| W4 | `ord wallet inscribe` command generator. | `render/pages.rs` | Regtest: an ord-made institution and child read correctly in BINST Next. |
| W5 (spike) | BIP-322 through a `to_sign` PSBT with Sparrow, Coldcard and Jade. | `bip322.rs`, `members.rs` | `bip322.rs` verifies what each device returns. |
| W6 (optional) | Direct WebHID: BitBox02 via `bitbox-api`, then Ledger. | `hw/*.rs` | Diego's devices, testnet4. |

Suggested first pick: **W0 + W1 + W2**. Together they let Sparrow (and through it every hardware wallet) or Bitcoin Core GUI create an institution and define a process against Diego's own ord and node, with no extension.

## Not verified
Electrum P2TR wallets on testnet4; SeedSigner's current Taproot state; BitBox02 policy support; Phantom, OKX and Leather on testnet4; Safari loopback fetch; whether the home ord runs `--index-addresses`; whether signers accept a BIP-322 `to_sign` PSBT; whether ord's envelope is byte-compatible with BINST readers.

## Built (2026-11-17, branch `wip/sovereign-wallets`)

**W0, one-time reveal key** (`src/reveal.rs`, `src/txbuilder.rs`, `src/inscribe.rs`, `src/recovery.rs`)
- The leaf key and the internal key are a fresh key made with Web Crypto (ord's layout). The page signs the reveal's script path and the wallet signs only key-path inputs.
- A new institution needs **one** wallet signature (the commit). A definition needs two: the commit, then the parent input in the reveal.
- The PSBTs carry BIP-371 fields: `tap_internal_key` and `tap_merkle_root` on the commit input, and `tap_bip32_derivation` on the wallet's inputs and change when a descriptor was imported. The bad zero-fingerprint entry is gone, as is UniSat's `disableTweakSigner`.
- The parent tag now encodes the index (`<txid>i<n>`).
- A `rawtr(<WIF>)#checksum` recovery descriptor is saved in localStorage before the commit is signed. It is listed under Settings with Copy and Forget, and dropped only if the commit signature is cancelled or refused.
- Two protocol reviews were run (bitcoin-protocol-reviewer). The first one's advice is adopted, except the root-admin proof input, which is not built because nothing verifies it today (listed below). After the second, one fix pass:
  - only SIGHASH_DEFAULT or ALL signatures are accepted from a signer, because an ANYONECANPAY signature on the parent input could be reused in another transaction;
  - every key-path signature is checked against its coin before broadcast;
  - commit and reveal are both signed before either is broadcast, and the signed reveal is saved for rebroadcast;
  - the recovery key is read back before signing, and never dropped on the hand-off path, since Sparrow or Core may have broadcast the commit themselves;
  - binary or wrapped PSBTs and transactions are accepted, and the descriptor checksum is verified.
- With A0 the leaf no longer shows the body's `admin` key signing. The `admin` field is a claim: control comes from holding the inscription and, for definitions, from spending the parent. The tree's "Admin key" fact is shown, not presented as signed.

**W1, sign elsewhere** (`src/account.rs`, `src/handoff.rs`, `src/qr.rs`, `src/render/handoff.rs`, picker in `src/render/shell.rs`)
- The picker has a "Sign elsewhere" section. Paste a Taproot account descriptor from Sparrow, Core (`listdescriptors`), Coldcard and others, and the page derives the address and key origin. No private key reaches the page.
- The hand-off dialog offers:
  - a `.psbt` download and copy base64;
  - an animated QR in UR `crypto-psbt` or BBQr;
  - the Bitcoin Core `walletprocesspsbt` command.
- The signed result comes back by paste (base64 or hex PSBT, or a signed raw tx in hex), by file, or by camera. The camera decodes in WASM with `rqrr`.
- What comes back must be the identical unsigned transaction, otherwise it is refused. Signatures are merged into the page's PSBT, so the page's own reveal signature survives a signer that drops it.

**W2, own servers** (`src/settings.rs`, `src/indexer.rs`, `src/actions.rs`, `src/citrea.rs`, `#/settings`)
- The ord URL replaces UniSat for lookups, address listings and content.
- The esplora URL serves UTXOs, transactions, fee estimates (mempool's `/v1/fees/recommended`, falling back to esplora's `/fee-estimates`), confirmation status and broadcast.
- The Citrea RPC URL is used for reads.

**Checks run**
- `cargo test --lib`: 196 pass.
- New unit tests cover:
  - the script-path signature verifying against the BIP-341 sighash;
  - BIP-86 derivation vectors and BIP-380 checksum vectors;
  - merge refusing a changed transaction;
  - BBQr and UR round trips, including a skipped UR part;
  - the camera decoder reading our own frames rendered to pixels.
- Regtest (`cargo test --lib reveal_regtest -- --ignored`, Bitcoin Core v30.2 + ord 0.27.1 `--index-addresses`):
  - Core's `walletprocesspsbt` signed the commit and the parent input;
  - ord shows the child with its validated parent;
  - the recovery descriptor, imported into a blank Core wallet, spent a stranded commit;
  - Core's `getdescriptorinfo` agrees with the checksum.
- In the browser, on regtest through a local esplora stand-in, with ord as the indexer and a Core wallet's descriptor imported:
  - created an institution and a definition with Core signing through the dialog;
  - returned the commit both as a PSBT and as a raw transaction;
  - an unsigned PSBT was refused;
  - Cancel works;
  - at 375 px wide nothing overflows.

**Not verified**
- Nothing on testnet4. Every broadcast here went to a local regtest node.
- No real hardware signer, Sparrow or Electrum; no camera (the agent browser has none).
- UniSat with the new PSBTs. The extension path changed: the reveal no longer asks the wallet for the script-path input.

**Follow-ups**
- Root admin proof: the reveal of a root institution also spends a key-path UTXO from `admin_address(admin)`, if the protocol should prove the admin key.
- With ord as the indexer, use its `children` field instead of looking only at the admin address.
- Book: document the new inscription flow and the sign-elsewhere path (workspace coordination rule).
- W3 bridge, W4 `ord wallet` command, W5 BIP-322 spike, W6 WebHID.
