# Provenance

Files copied from `binst-pilot-webapp` at commit `dda2d1a` on 2026-11-16. The source repo was not modified.

| Source path | Changes |
|---|---|
| `src/txbuilder.rs` | none (verbatim); 2026-11-17: one-time reveal key as leaf and internal key instead of the admin key over NUMS; the parent tag encodes the index |
| `src/stack.rs` | none (verbatim) |
| `src/stack_plan.rs` | none (verbatim) |
| `src/auth.rs` | none (verbatim) |
| `src/fetch.rs` | none (verbatim) |
| `src/storage.rs` | none (verbatim) |
| `src/inscribe.rs` | wallet address read from the session store instead of hidden inputs; indexer module renamed; 2026-11-17: one-time reveal key signed in the page, wallet signs key-path inputs only (extension or PSBT hand-off), chain API from settings |
| `src/citrea.rs` | L2 address read from the session store instead of a hidden input; 2026-11-17: reads use the Citrea RPC from settings |
| `src/wallet.rs` | wallet-bar UI and DOM hidden-input state removed; session state lives in session.rs |
| `src/search.rs` | indexer half only (configuration, UniSat parsing, fetches) plus its tests; the search view is not carried over; 2026-11-17: the user's own ord server as a backend |
| `api/indexer.js` | none (verbatim) |
| `vercel.json` | none (verbatim; JSON cannot carry a header) |
| `.cargo/config.toml` | none (verbatim) |
| `scripts/inject-env.sh` | none (verbatim) |
| `LICENSE` | none (verbatim) |
| `src/wallet_picker.rs` | icon helpers only → `src/render/icons.rs`; `evm_connect_and_switch` rewritten in `src/connect.rs` to return the session |

## From other repos

| Source | Destination | Changes |
|---|---|---|
| `diegobianqui-site@28cf1ae:src/lab/constellation/viewer.js` | `assets/js/hero.js` | Kept the renderer guard, pixel-ratio cap, texture and sprite helpers and the comet idea; dropped the agent-session model, panels, controls and post-processing; nodes laid out as a three-level tree |
| `orchos-site@6d2f0a2:src/motion/motion.js` | `assets/js/motion.js` | The `fx` / `jsfx` gate only; the binding engine replaced by an IntersectionObserver for `.reveal` blocks |
| `three@0.186.1` (MIT), copied from `diegobianqui-site@28cf1ae:node_modules/three/build/` | `assets/vendor/three/` | None (see `assets/vendor/three/PROVENANCE`) |

## Approaches followed (no code copied)

| Source | Where | What |
|---|---|---|
| `ord` 0.27 `src/wallet/batch/plan.rs` (CC0) | `src/txbuilder.rs`, `src/reveal.rs` | The reveal key is a fresh key used as both the leaf key and the internal key, signed by the inscriber, with a `rawtr(<WIF>)` recovery descriptor |
