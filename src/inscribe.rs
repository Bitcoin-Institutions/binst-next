// reused-from: binst-pilot-webapp@dda2d1a:src/inscribe.rs (2026-11-16) — adapted: wallet address read from the session store instead of hidden inputs; indexer module renamed; 2026-11-17 one-time reveal key signed in the page, wallet signs key-path inputs only (extension or hand-off), chain API from settings
//! Serverless inscription flow — no local `ord` CLI, no bridge server.
//!
//! Full pipeline, entirely in-browser:
//!   1. Fetch UTXOs for the connected address from the chain API (settings)
//!   2. Make a one-time reveal key and save its recovery descriptor locally
//!   3. Build commit + reveal transactions via `txbuilder`
//!   4. The user's wallet signs the commit (key-path inputs only); the page
//!      signs the reveal's script path with the one-time key and, for a child,
//!      the wallet signs the parent input. Both are signed before either is
//!      broadcast; the signed reveal is saved with the recovery key
//!   5. Broadcast the commit, then the reveal
//!   6. Return the reveal txid as the inscription reference
//!
//! "The wallet" is either a browser extension (UniSat, Xverse) or any PSBT
//! signer reached through the hand-off dialog (`handoff.rs`): Sparrow, Bitcoin
//! Core, Electrum, or a hardware signer by file or QR.

use bitcoin::base64::{engine::general_purpose::STANDARD, Engine};
use bitcoin::psbt::Psbt;
use bitcoin::secp256k1::SecretKey;
use bitcoin::{Address, Amount, Network, OutPoint, Txid};
use binst_inscription::BinstEntity;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::fetch::fetch_text;
use crate::reveal::{self, WalletKey};
use crate::txbuilder::{build_commit_reveal, psbt_to_base64, Utxo};
use crate::wallet::{
    detect_all_btc_providers, get_connected_l1_provider, get_connected_pubkey,
    is_sats_connect_provider, resolve_btc_provider,
};

// ── Public entry point ────────────────────────────────────────────

/// Result of a successful serverless inscription.
pub struct InscribeResult {
    pub commit_txid: String,
    pub reveal_txid: String,
    /// Ordinals inscription ID — `<reveal_txid>i0`
    pub inscription_id: String,
    pub total_fees: u64,
    /// The reveal output holding the new inscription's sat (output 0 for a
    /// root, output 1 for a child — the parent is returned in output 0).
    /// Pass this as `parent_utxo` to the next inscription in a parent-child chain.
    pub reveal_utxo: Utxo,
}

/// Full serverless inscription pipeline.
///
/// - `entity`: the BINST entity to inscribe.
/// - `fee_rate`: sat/vB (default 10 if 0 is passed).
/// - `parent_inscription_id`: optional Ordinals parent ID (`<txid>i<n>`); tag 3.
/// - `parent_utxo`: the UTXO currently holding the parent inscription's sat.
///   **Required by the Ordinals provenance rule** — the reveal must spend it.
pub async fn inscribe_serverless(
    entity: &BinstEntity,
    fee_rate: u64,
    parent_inscription_id: Option<&str>,
    parent_utxo: Option<Utxo>,
) -> Result<InscribeResult, String> {
    let fee_rate = if fee_rate == 0 { 10 } else { fee_rate };

    require_parent_utxo(parent_inscription_id, parent_utxo.as_ref())?;

    // ── 1. Resolve connected wallet ───────────────────────────────
    let pubkey_hex = get_connected_pubkey();
    if pubkey_hex.starts_with('<') {
        return Err("Connect a BTC wallet first".to_string());
    }
    let wallet_internal = pubkey_hex
        .parse::<bitcoin::key::XOnlyPublicKey>()
        .map_err(|e| format!("Invalid x-only pubkey: {e}"))?;

    let address_str = get_connected_address()?;
    let network = Network::Testnet4;
    let change_address: Address = address_str
        .parse::<Address<bitcoin::address::NetworkUnchecked>>()
        .map_err(|e| format!("Invalid wallet address '{address_str}': {e}"))?
        .require_network(network)
        .map_err(|_| format!("Address '{address_str}' is not testnet4"))?;
    let wallet = WalletKey {
        internal: wallet_internal,
        origin: crate::session::l1_origin(),
        script_pubkey: change_address.script_pubkey(),
    };

    // ── 2. Fetch UTXOs ────────────────────────────────────────────
    let utxos = fetch_utxos(&address_str, &change_address).await?;
    if utxos.is_empty() {
        return Err(format!(
            "No UTXOs found for {address_str} on testnet4. Fund the address first."
        ));
    }

    // Inscription carriers (≤ 546 sats) are never spent as commit inputs; the
    // parent UTXO is passed separately and explicitly.
    let funding_utxos: Vec<_> = utxos
        .into_iter()
        .filter(|u| u.amount.to_sat() > 546)
        .collect();
    if funding_utxos.is_empty() {
        return Err(format!(
            "No spendable (non-inscription) UTXOs found for {address_str}. \
             Send plain tBTC (>546 sats, no inscriptions) to this address first."
        ));
    }

    // ── 3. One-time reveal key, then the plan ─────────────────────
    let reveal_secret = fresh_secret()?;
    let reveal_key = reveal_secret
        .x_only_public_key(&bitcoin::secp256k1::Secp256k1::new())
        .0;
    let plan = build_commit_reveal(
        &reveal_key,
        entity,
        &funding_utxos,
        fee_rate,
        &change_address,
        network,
        parent_inscription_id,
        parent_utxo,
    )?;
    let estimated_fees = plan.estimated_fee;
    let commit_txid_planned = plan.commit_tx.compute_txid().to_string();
    let label = entity_label(entity);
    // Saved before anything is signed: if the reveal never happens, this
    // descriptor spends the commit output back (Bitcoin Core / Sparrow import).
    // No copy, no signing.
    let saved = crate::recovery::remember(crate::recovery::Record {
        commit_txid: commit_txid_planned.clone(),
        reveal_txid: String::new(),
        reveal_hex: String::new(),
        descriptor: reveal::recovery_descriptor(&plan, &reveal_secret),
        label: label.clone(),
        created_ms: js_sys::Date::now() as u64,
    });
    if !saved {
        return Err("This browser would not save the one-time recovery key (private window or storage full), \
                    so nothing was signed. Use a normal window and try again."
            .into());
    }
    // An extension wallet never broadcasts what it signs here, so while
    // nothing is broadcast a failed attempt can drop its record. A signer
    // outside the browser (Sparrow, Core) may have broadcast the commit
    // itself: its record is always kept.
    let handoff = crate::session::l1_is_handoff();
    let give_up = |why: String| {
        if !handoff {
            crate::recovery::forget(&commit_txid_planned);
        }
        why
    };

    // ── 4. Sign both transactions before broadcasting either ──────
    // The commit's txid is fixed before signing (segwit), so the reveal can
    // be signed now: a reload or a failed hand-off after this point leaves a
    // complete reveal to rebroadcast, not a stranded commit.
    let commit_psbt = reveal::commit_psbt(&plan, &wallet).map_err(give_up)?;
    let signed_commit = sign_with_wallet(&commit_psbt, &address_str, "commit", &label).await.map_err(give_up)?;
    let commit_hex = tx_hex(&reveal::extract(&signed_commit).map_err(give_up)?);

    let mut reveal_psbt = reveal::reveal_psbt(&plan, &wallet).map_err(give_up)?;
    reveal::sign_reveal_input(&mut reveal_psbt, &plan, &reveal_secret).map_err(give_up)?;
    if !reveal::unsigned_inputs(&reveal_psbt).is_empty() {
        reveal_psbt = sign_with_wallet(&reveal_psbt, &address_str, "reveal", &label).await.map_err(give_up)?;
    }
    let reveal_hex = tx_hex(&reveal::extract(&reveal_psbt).map_err(give_up)?);
    crate::recovery::signed_reveal(&commit_txid_planned, &reveal_hex);

    // ── 5. Broadcast ──────────────────────────────────────────────
    // A failed broadcast call does not prove the commit was not relayed, so
    // the key is kept from here on.
    let commit_txid = broadcast_tx(&commit_hex).await?;
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] Commit tx broadcast: {commit_txid}"
    )));
    let reveal_txid = broadcast_tx(&reveal_hex).await.map_err(|why| {
        format!("{why}. The commit is out; the signed reveal is saved under Settings to broadcast again.")
    })?;
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] Reveal tx broadcast: {reveal_txid}"
    )));
    crate::recovery::revealed(&commit_txid_planned, &reveal_txid);

    let inscription_id = format!("{reveal_txid}i0");
    let inscription_vout = plan.inscription_vout();
    let inscription_out = &plan.reveal_tx.output[inscription_vout as usize];
    let reveal_utxo = Utxo {
        outpoint: OutPoint::new(
            reveal_txid.parse::<Txid>().unwrap_or_else(|_| plan.reveal_tx.compute_txid()),
            inscription_vout,
        ),
        amount: inscription_out.value,
        script_pubkey: inscription_out.script_pubkey.clone(),
    };

    Ok(InscribeResult {
        commit_txid,
        reveal_txid,
        inscription_id,
        total_fees: estimated_fees,
        reveal_utxo,
    })
}

fn entity_label(entity: &BinstEntity) -> String {
    match entity {
        BinstEntity::Institution(b) => format!("Institution \"{}\"", b.name),
        BinstEntity::ProcessTemplate(b) => format!("Process \"{}\"", b.name),
        _ => "Inscription".to_string(),
    }
}

fn tx_hex(tx: &bitcoin::Transaction) -> String {
    hex_encode(&bitcoin::consensus::encode::serialize(tx))
}

/// 32 random bytes from Web Crypto as a secret key.
fn fresh_secret() -> Result<SecretKey, String> {
    let crypto = web_sys::window()
        .and_then(|w| w.crypto().ok())
        .ok_or("This browser has no Web Crypto")?;
    for _ in 0..4 {
        let mut bytes = [0u8; 32];
        crypto
            .get_random_values_with_u8_array(&mut bytes)
            .map_err(|e| format!("crypto.getRandomValues: {e:?}"))?;
        if let Ok(secret) = SecretKey::from_slice(&bytes) {
            return Ok(secret);
        }
    }
    Err("Could not make a reveal key".into())
}

/// Have the user's wallet sign its key-path inputs, and return the merged,
/// checked PSBT. Extension wallets are called directly; any other signer goes
/// through the hand-off dialog (file, QR, copy/paste, Bitcoin Core command).
async fn sign_with_wallet(psbt: &Psbt, address: &str, step: &str, label: &str) -> Result<Psbt, String> {
    let returned: Vec<u8> = if crate::session::l1_is_handoff() {
        crate::handoff::request(step, label, psbt).await?
    } else {
        wallet_sign_psbt(&psbt_to_base64(psbt), address).await?.into_bytes()
    };
    let mut merged = reveal::merge_signed(psbt, &returned)?;
    reveal::finalize_key_path_inputs(&mut merged);
    let missing = reveal::unsigned_inputs(&merged);
    if !missing.is_empty() {
        return Err(format!(
            "The {step} came back without a signature on input(s) {missing:?}. \
             Check that the signer holds the key for {address}."
        ));
    }
    reveal::verify_key_path_signatures(&merged)?;
    Ok(merged)
}

// ── UTXO fetching ─────────────────────────────────────────────────

async fn fetch_utxos(address: &str, script_address: &Address) -> Result<Vec<Utxo>, String> {
    let url = format!("{}/address/{address}/utxo", crate::settings::esplora_base());
    let text = fetch_text(&url).await.map_err(|e| {
        format!("Failed to fetch UTXOs from the chain API: {e}")
    })?;

    let items: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Mempool UTXO JSON parse error: {e}"))?;

    let arr = items
        .as_array()
        .ok_or_else(|| "Mempool UTXO response is not an array".to_string())?;

    let script_pubkey = script_address.script_pubkey();

    let mut utxos = Vec::new();
    for item in arr {
        // Accept both confirmed and unconfirmed UTXOs so we can spend
        // change outputs without waiting for a block (common on testnet4
        // where block intervals can be long).
        let txid_str = item["txid"].as_str().ok_or("UTXO missing txid")?;
        let vout = item["vout"].as_u64().ok_or("UTXO missing vout")? as u32;
        let value = item["value"].as_u64().ok_or("UTXO missing value")?;

        let txid: Txid = txid_str
            .parse()
            .map_err(|e| format!("Bad txid '{txid_str}': {e}"))?;

        utxos.push(Utxo {
            outpoint: OutPoint::new(txid, vout),
            amount: Amount::from_sat(value),
            script_pubkey: script_pubkey.clone(),
        });
    }

    Ok(utxos)
}

// ── Extension wallet PSBT signing ─────────────────────────────────

/// Sign a PSBT via the connected browser-extension wallet.
///
/// Two paths:
///  1. **Sats-connect** (Xverse): `provider.request("signPsbt", { psbt, signInputs, broadcast: false })`
///  2. **Direct** (UniSat, OKX): `provider.signPsbt(psbtHex, options)` → hex string.
async fn wallet_sign_psbt(psbt_b64: &str, signing_address: &str) -> Result<String, String> {
    let stored_path = get_connected_l1_provider();
    let (provider, window_path) = if !stored_path.is_empty() {
        let p = resolve_btc_provider(&stored_path)
            .ok_or_else(|| format!("BTC provider '{}' not found", stored_path))?;
        (p, stored_path.as_str().to_string())
    } else {
        let handle = detect_all_btc_providers()
            .into_iter()
            .next()
            .ok_or_else(|| "No Bitcoin wallet detected — connect a BTC wallet first".to_string())?;
        let p = resolve_btc_provider(handle.window_path)
            .ok_or_else(|| format!("BTC provider '{}' not found", handle.window_path))?;
        (p, handle.window_path.to_string())
    };

    if is_sats_connect_provider(&provider) {
        return wallet_sign_psbt_sats_connect(&provider, psbt_b64, signing_address, &window_path).await;
    }
    wallet_sign_psbt_direct(&provider, psbt_b64, signing_address, &window_path).await
}

/// Sats-connect `request("signPsbt", ...)` — used by Xverse.
///
/// For the 2-input child reveal PSBT (parent UTXO + commit), each input may
/// belong to a different P2TR address:
///   - input 0 (parent): the institution's current sat address (key-path spend)
///   - input 1 (commit): tweaked address (script-path spend)
///
/// Xverse's signInputs validates that each mapped address matches the
/// witness_utxo script for the given input indices.  Mapping all inputs
/// under one address causes "PSBT params are invalid" when the two inputs
/// live at different addresses.
///
/// Fix: build signInputs by grouping input indices by their witness_utxo
/// address.  Inputs without a witness_utxo fall back to `signing_address`.
async fn wallet_sign_psbt_sats_connect(
    provider: &JsValue,
    psbt_b64: &str,
    signing_address: &str,
    window_path: &str,
) -> Result<String, String> {
    let request_fn = js_sys::Reflect::get(provider, &JsValue::from_str("request"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
        .ok_or_else(|| format!("{window_path}: request() not found"))?;

    let psbt_bytes = STANDARD.decode(psbt_b64)
        .map_err(|e| format!("PSBT base64 decode: {e}"))?;
    let psbt = Psbt::deserialize(&psbt_bytes)
        .map_err(|e| format!("PSBT deserialize: {e}"))?;

    // Build signInputs: { "<address>": [input_indices...] }
    // Group each input index under the address derived from its witness_utxo script.
    // Inputs without a resolvable address fall back to `signing_address`.
    let network = Network::Testnet4;
    let mut addr_to_indices: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();

    for (i, inp) in psbt.inputs.iter().enumerate() {
        // The reveal's script-path input arrives already signed by the page.
        if inp.final_script_witness.is_some() {
            continue;
        }
        let addr = inp.witness_utxo.as_ref().and_then(|txout| {
            Address::from_script(&txout.script_pubkey, network)
                .ok()
                .map(|a| a.to_string())
        });
        let key = addr.unwrap_or_else(|| signing_address.to_string());
        addr_to_indices.entry(key).or_default().push(i);
    }

    // If every input resolved to the same address, simplify to a single entry.
    // Otherwise emit one entry per address.
    let sign_inputs = js_sys::Object::new();
    for (addr, indices) in &addr_to_indices {
        let arr = js_sys::Array::new();
        for &idx in indices {
            arr.push(&JsValue::from_f64(idx as f64));
        }
        js_sys::Reflect::set(&sign_inputs, &JsValue::from_str(addr), &arr).ok();
    }

    // Build params: { psbt, signInputs, broadcast: false }
    let params = js_sys::Object::new();
    js_sys::Reflect::set(&params, &JsValue::from_str("psbt"), &JsValue::from_str(psbt_b64)).ok();
    js_sys::Reflect::set(&params, &JsValue::from_str("signInputs"), &sign_inputs).ok();
    js_sys::Reflect::set(&params, &JsValue::from_str("broadcast"), &JsValue::from_bool(false)).ok();

    let sign_inputs_desc: Vec<String> = addr_to_indices.iter()
        .map(|(a, idxs)| format!("{}: {:?}", &a[..a.len().min(20)], idxs))
        .collect();
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] signPsbt via sats-connect (signInputs: {{{}}}) on {window_path}",
        sign_inputs_desc.join(", ")
    )));

    let promise: js_sys::Promise = request_fn
        .call2(provider, &JsValue::from_str("signPsbt"), &params)
        .map_err(|e| format!("request('signPsbt') call failed: {e:?}"))?
        .dyn_into()
        .map_err(|_| "request('signPsbt') did not return a Promise".to_string())?;

    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| {
            let msg = format!("{e:?}");
            if is_rejection(&msg) {
                "User rejected the PSBT signing request".to_string()
            } else {
                format!("signPsbt rejected: {msg}")
            }
        })?;

    // Response shapes:
    //   { status: "success", result: { psbt: "<base64>" } }
    //   { result: { psbt: "<base64>" } }
    //   { psbt: "<base64>" }
    //   or plain string
    if let Some(s) = result.as_string() {
        return Ok(s);
    }

    // Try result.result.psbt
    if let Ok(inner) = js_sys::Reflect::get(&result, &JsValue::from_str("result")) {
        if let Ok(p) = js_sys::Reflect::get(&inner, &JsValue::from_str("psbt")) {
            if let Some(s) = p.as_string() {
                return Ok(s);
            }
        }
        // result.psbt (if result is flat)
        if let Some(s) = inner.as_string() {
            return Ok(s);
        }
    }
    // Try result.psbt directly
    if let Ok(p) = js_sys::Reflect::get(&result, &JsValue::from_str("psbt")) {
        if let Some(s) = p.as_string() {
            return Ok(s);
        }
    }

    // Debug: log what we got
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] signPsbt sats-connect unexpected result: {:?}", result
    )));

    Err("signPsbt (sats-connect) returned an unexpected value".to_string())
}

/// A parent ID without the parent's UTXO would write tag 3 without spending
/// the parent, and ord never links such a child. Refuse instead of inscribing
/// an orphan.
fn require_parent_utxo(parent_inscription_id: Option<&str>, parent_utxo: Option<&Utxo>) -> Result<(), String> {
    match (parent_inscription_id, parent_utxo) {
        (Some(pid), None) => Err(format!(
            "Could not locate the parent inscription {pid}'s current UTXO, so the child \
             cannot be linked to it. Check that the parent is confirmed and owned by the \
             connected wallet, then retry."
        )),
        _ => Ok(()),
    }
}

/// Which PSBT inputs to ask a direct-API wallet to sign: inputs not signed yet
/// whose UTXO belongs to `signing_address` (commit funding UTXOs, the parent
/// sat). The reveal's script-path input is already signed by the page.
fn should_sign_input(already_signed: bool, utxo_addr: Option<&str>, signing_address: &str) -> bool {
    !already_signed && utxo_addr == Some(signing_address)
}

/// Direct `signPsbt(psbtHex, options)` call — used by UniSat, OKX, etc.
///
/// Key differences from sats-connect (Xverse):
/// - UniSat takes **hex**, not base64.
/// - UniSat returns **hex** (not base64 or `{ psbt }`).
/// - For multi-input PSBTs, `toSignInputs` must be provided explicitly so
///   UniSat knows which key to use for each input.
/// - The page signs the reveal's script-path input itself (one-time reveal
///   key), so the wallet is asked only for key-path inputs at its own address:
///   commit funding inputs and the parent sat. `disableTweakSigner` is never
///   set; key-path inputs need the tweaked key.
async fn wallet_sign_psbt_direct(
    provider: &JsValue,
    psbt_b64: &str,
    signing_address: &str,
    window_path: &str,
) -> Result<String, String> {
    let sign_fn_val = js_sys::Reflect::get(provider, &JsValue::from_str("signPsbt"))
        .ok()
        .filter(|v| v.is_function())
        .or_else(|| {
            js_sys::Reflect::get(provider, &JsValue::from_str("signPSBT"))
                .ok()
                .filter(|v| v.is_function())
        })
        .ok_or_else(|| format!("{window_path}: no signPsbt/signPSBT function found"))?;

    let sign_fn: js_sys::Function = sign_fn_val
        .dyn_into()
        .map_err(|_| "signPsbt is not a function on this provider".to_string())?;

    // UniSat requires hex, not base64.
    let psbt_bytes = STANDARD.decode(psbt_b64)
        .map_err(|e| format!("PSBT base64 decode: {e}"))?;
    let psbt_hex = hex_encode(&psbt_bytes);

    // Parse the PSBT to build toSignInputs.
    let psbt = Psbt::deserialize(&psbt_bytes)
        .map_err(|e| format!("PSBT deserialize: {e}"))?;

    // toSignInputs: the unsigned inputs whose UTXO is at the wallet's address.
    let network = Network::Testnet4;
    let to_sign_inputs = js_sys::Array::new();
    for (i, inp) in psbt.inputs.iter().enumerate() {
        let utxo_addr = inp.witness_utxo.as_ref().and_then(|txout| {
            Address::from_script(&txout.script_pubkey, network).ok().map(|a| a.to_string())
        });
        if !should_sign_input(inp.final_script_witness.is_some(), utxo_addr.as_deref(), signing_address) {
            continue;
        }
        let entry = js_sys::Object::new();
        js_sys::Reflect::set(&entry, &JsValue::from_str("index"), &JsValue::from_f64(i as f64)).ok();
        js_sys::Reflect::set(&entry, &JsValue::from_str("address"), &JsValue::from_str(signing_address)).ok();
        to_sign_inputs.push(&entry);
    }

    // Build options: { autoFinalized: false, toSignInputs: [...] }
    let options = js_sys::Object::new();
    js_sys::Reflect::set(&options, &JsValue::from_str("autoFinalized"), &JsValue::from_bool(false)).ok();
    js_sys::Reflect::set(&options, &JsValue::from_str("toSignInputs"), &to_sign_inputs).ok();

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] signPsbt via direct call ({} inputs, {} to sign) on {window_path}",
        psbt.inputs.len(),
        to_sign_inputs.length()
    )));

    let promise: js_sys::Promise = sign_fn
        .call2(provider, &JsValue::from_str(&psbt_hex), &options)
        .map_err(|e| format!("signPsbt call failed: {e:?}"))?
        .dyn_into()
        .map_err(|_| "signPsbt did not return a Promise".to_string())?;

    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| {
            let msg = format!("{e:?}");
            if is_rejection(&msg) {
                "User rejected the PSBT signing request".to_string()
            } else {
                format!("signPsbt rejected: {msg}")
            }
        })?;

    // UniSat returns a hex string directly.
    // Some wallets return base64 or { psbt: "..." } — handle all cases.
    if let Some(s) = result.as_string() {
        return Ok(s);
    }
    if let Ok(psbt_field) = js_sys::Reflect::get(&result, &JsValue::from_str("psbt")) {
        if let Some(s) = psbt_field.as_string() {
            return Ok(s);
        }
    }

    Err("signPsbt returned an unexpected value (expected hex string or {psbt:...})".to_string())
}

fn is_rejection(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("reject") || lower.contains("cancel") || lower.contains("denied")
}

// ── Parent UTXO fetching ──────────────────────────────────────────

/// Fetch a specific output from a (possibly unconfirmed) transaction via
/// Mempool.space and return it as a `Utxo`.
///
/// Used by callers that need to pass the parent inscription's current UTXO
/// to `inscribe_serverless` so that the reveal tx can spend it (Ordinals
/// provenance rule).
///
/// `vout` is `0` for a root inscription and `1` for a child (the parent is
/// returned in output 0 of a child reveal).
pub async fn fetch_tx_output(txid: &str, vout: u32) -> Result<Utxo, String> {
    let url = format!("{}/tx/{txid}", crate::settings::esplora_base());
    let text = fetch_text(&url)
        .await
        .map_err(|e| format!("Failed to fetch tx {txid}: {e}"))?;

    let tx_json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Tx JSON parse error: {e}"))?;

    let vout_arr = tx_json["vout"]
        .as_array()
        .ok_or_else(|| format!("Tx {txid} has no vout array"))?;

    let output = vout_arr
        .get(vout as usize)
        .ok_or_else(|| format!("Tx {txid} has no vout {vout}"))?;

    let value = output["value"]
        .as_u64()
        .ok_or_else(|| format!("Tx {txid} vout {vout} missing value"))?;

    let scriptpubkey_hex = output["scriptpubkey"]
        .as_str()
        .ok_or_else(|| format!("Tx {txid} vout {vout} missing scriptpubkey"))?;

    let script_bytes = hex_decode(scriptpubkey_hex)
        .map_err(|e| format!("scriptpubkey hex decode: {e}"))?;
    let script_pubkey = bitcoin::ScriptBuf::from_bytes(script_bytes);

    let txid_parsed: Txid = txid
        .parse()
        .map_err(|e| format!("Invalid txid '{txid}': {e}"))?;

    Ok(Utxo {
        outpoint: OutPoint::new(txid_parsed, vout),
        amount: Amount::from_sat(value),
        script_pubkey,
    })
}

// ── Transaction broadcast ─────────────────────────────────────────

/// Resolve the **current** UTXO holding a given Ordinals inscription's sat.
///
/// Uses the inscription indexer (`crate::indexer::indexer_inscription`) to read
/// the sat's current output, which tracks its live location regardless of how
/// many times it has moved (e.g. after multiple children have been inscribed
/// and the parent sat returned each time via output 0 of each child's reveal tx).
///
/// Falls back to parsing the inscription ID itself (`{txid}i0` → vout 0) if
/// the indexer call fails — this is correct only for a fresh root inscription
/// (an institution with no children yet), not for a child, whose sat starts at vout 1.
///
/// Use this instead of `fetch_tx_output(txid, 0)` whenever you need the
/// parent UTXO for an inscription that may already have children.
pub async fn fetch_inscription_sat_utxo(inscription_id: &str) -> Result<Utxo, String> {
    let short = &inscription_id[..8.min(inscription_id.len())];
    let located = crate::indexer::indexer_inscription(inscription_id)
        .await
        .and_then(|ins| ins.output.ok_or_else(|| "indexer returned no output".to_string()));

    let (txid, vout) = match located {
        Ok(output) => output,
        Err(e) => {
            // Indexer unavailable or inscription not indexed yet — fall back.
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "[BINST] indexer sat lookup failed ({e}), falling back to vout 0 for {short}"
            )));
            let txid = inscription_id.split('i').next().unwrap_or(inscription_id);
            return fetch_tx_output(txid, 0).await;
        }
    };

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] Sat location for {short}…: {}:{vout}",
        &txid[..8.min(txid.len())]
    )));

    fetch_tx_output(&txid, vout).await
}

/// Broadcast a raw transaction hex through the chain API (mempool.space by default).
/// Returns the txid on success.
async fn broadcast_tx(raw_hex: &str) -> Result<String, String> {
    let url = format!("{}/tx", crate::settings::esplora_base());

    let opts = web_sys::RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(web_sys::RequestMode::Cors);
    opts.set_body(&JsValue::from_str(raw_hex));

    let req = web_sys::Request::new_with_str_and_init(&url, &opts)
        .map_err(|e| format!("Failed to build broadcast request: {e:?}"))?;
    req.headers()
        .set("Content-Type", "text/plain")
        .map_err(|e| format!("Failed to set content-type: {e:?}"))?;

    let window = web_sys::window().ok_or("No window")?;
    let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&req))
        .await
        .map_err(|e| format!("Broadcast request failed: {e:?}"))?;

    let resp: web_sys::Response = resp_val
        .dyn_into()
        .map_err(|_| "Broadcast response is not a Response object".to_string())?;

    let status = resp.status();
    let body = wasm_bindgen_futures::JsFuture::from(
        resp.text().map_err(|e| format!("resp.text() failed: {e:?}"))?,
    )
    .await
    .map_err(|e| format!("Failed to read broadcast response body: {e:?}"))?
    .as_string()
    .unwrap_or_default();

    if status == 200 {
        Ok(body.trim().to_string())
    } else {
        Err(format!(
            "Broadcast rejected (HTTP {status}): {body}"
        ))
    }
}

// ── Helpers ───────────────────────────────────────────────────────

/// Get the connected L1 wallet address from the session store.
fn get_connected_address() -> Result<String, String> {
    let addr = crate::session::l1_address();
    if addr.is_empty() {
        Err("No BTC wallet address found — connect a wallet first".to_string())
    } else {
        Ok(addr)
    }
}

/// Encode bytes as lowercase hex.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode a lowercase/uppercase hex string to bytes.
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("bad hex byte: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_id_without_parent_utxo_is_refused() {
        let pid = "2222222222222222222222222222222222222222222222222222222222222222i0";
        assert!(require_parent_utxo(Some(pid), None).is_err());
        assert!(require_parent_utxo(None, None).is_ok());
        let parent = Utxo {
            outpoint: OutPoint::null(),
            amount: Amount::from_sat(546),
            script_pubkey: bitcoin::ScriptBuf::new(),
        };
        assert!(require_parent_utxo(Some(pid), Some(&parent)).is_ok());
    }

    #[test]
    fn direct_signer_signs_only_own_unsigned_inputs() {
        let me = "tb1pme";
        assert!(should_sign_input(false, Some(me), me));
        // The reveal's script-path input, already signed by the page: skipped.
        assert!(!should_sign_input(true, Some("tb1pcommitaddress"), me));
        assert!(!should_sign_input(true, Some(me), me));
        // Foreign input: skipped.
        assert!(!should_sign_input(false, Some("tb1psomeoneelse"), me));
        assert!(!should_sign_input(false, None, me));
    }
}
