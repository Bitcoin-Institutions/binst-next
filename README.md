# BINST Next

A prototype of a redesigned workflow for [BINST](https://github.com/Bitcoin-Institutions/BINST) (Bitcoin Institutions), written in Rust and compiled to WebAssembly.

An institution is an Ordinals inscription on Bitcoin. Its process definitions are child inscriptions. Instances of a process run on an L2 (Citrea). This app shows each institution as a tree of those three levels and lets you act on the tree.

**Pilot on Bitcoin testnet4 and Citrea testnet. Unaudited. Use test coins only.**

## What works

| Area | State |
|---|---|
| Institution trees from live data (UniSat indexer + Citrea RPC), with a featured institution that needs no wallet | Working |
| Shareable URLs for every screen, including one instance | Working |
| Create an institution, define a process, start an instance, execute a step | Built; not yet exercised with a wallet |
| Sign elsewhere: a watch-only Taproot account whose PSBTs are signed by Sparrow, Bitcoin Core, Electrum or a hardware signer (file, copy/paste, animated BBQr/UR QR, camera scan back) | Institution and definition exercised on regtest with Bitcoin Core as the signer; not yet on testnet4 |
| Your own servers: ord (`--index-addresses`), esplora/mempool and Citrea RPC, set under Settings | ord and esplora exercised on regtest |
| Home page with a three.js view of the featured tree (static tree under reduced motion) | Working |
| Members and roles on Nostr (NIP-58 badges issued by a key the holder vouches for with a BIP-322 signature) | Reading and verification working; binding and issuing not yet exercised with a wallet |
| Bitcoin key authorising L2 steps, Ark treasury, covenant vault | Not built (see the Lab page) |

## What enforces what

| Claim | Enforced by |
|---|---|
| The institution exists and who controls it | Bitcoin |
| A process definition belongs to an institution | The indexer's rule that the child's reveal spent the parent's sat. Not a consensus rule. |
| Finding institutions and definitions | The UniSat indexer API, a trusted third party, or your own ord server (Settings) |
| Instance state and step order | The Citrea contract |
| Who may execute a step | Nobody yet: the pilot factory is open to anyone |
| Members and roles | Signatures only: shown in the app, not enforced on L2 |

## Run it

Requirements: Rust with the `wasm32-unknown-unknown` target, [Trunk](https://trunkrs.dev), Node 22. On macOS the C parts of `secp256k1` need Homebrew LLVM (`.cargo/config.toml` points to it).

The protocol crates are taken from a local checkout: `Cargo.toml` patches the `BINST-protocol` git dependency to `../../binst-protocol`. Clone [BINST-protocol](https://github.com/Bitcoin-Institutions/BINST-protocol) there, or remove the `[patch]` block to build against the published repository.

```bash
cargo test --lib                 # unit tests, run natively
node scripts/dev-proxy.mjs       # indexer proxy on :8787
trunk serve --port 8090          # the app on http://localhost:8090
```

`scripts/dev-proxy.mjs` runs `api/indexer.js` (the same function Vercel serves) when `UNISAT_API_KEY` is set. Without a key, set `INDEXER_UPSTREAM` to the origin of a deployed copy of this app and it forwards there.

## Layout

- `src/state.rs`, `src/store.rs`, `src/router.rs`: typed state with a pure update function, the one store, hash routes
- `src/load.rs`, `src/actions.rs`, `src/connect.rs`: network reads, signing and broadcasting, wallet sessions
- `src/render/`: HTML-string views and one delegated event listener
- `src/nostr.rs`, `src/bip322.rs`, `src/relay.rs`, `src/members.rs`: members and roles on Nostr
- `src/txbuilder.rs`, `src/inscribe.rs`, `src/citrea.rs`, `src/wallet.rs` and others: logic carried over from the earlier pilot web app
- `assets/js/hero.js`: the home page scene; `assets/vendor/three/`: three.js 0.186.1, unmodified

`PROVENANCE.md` lists every file that came from another repository and what changed.

## Licence

MIT. See `LICENSE`. three.js is MIT and keeps its own licence file.
