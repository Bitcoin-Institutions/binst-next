// reused-from: binst-pilot-webapp@dda2d1a:src/inscribe.rs (2026-11-16) — adapted: wallet address read from the session store instead of hidden inputs; indexer module renamed
//! Serverless inscription flow — no local `ord` CLI, no bridge server.
//!
//! Full pipeline, entirely in-browser:
//!   1. Fetch UTXOs for the connected address from Mempool.space
//!   2. Build commit + reveal transactions via `txbuilder`
//!   3. Sign commit PSBT via the connected BTC wallet (`signPsbt`)
//!   4. Broadcast the signed commit tx via Mempool.space
//!   5. Sign the reveal PSBT via the wallet
//!   6. Broadcast the signed reveal tx
//!   7. Return the reveal txid as the inscription reference
//!
//! Confirmation polling lives in `bridge::start_confirmation_poll`, which
//! already queries Mempool.space directly (no local bridge needed).

use bitcoin::base64::{engine::general_purpose::STANDARD, Engine};
use bitcoin::psbt::Psbt;
use bitcoin::{Address, Amount, Network, OutPoint, Txid};
use binst_inscription::BinstEntity;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::fetch::fetch_text;
use crate::txbuilder::{build_commit_reveal, commit_to_psbt, psbt_to_base64, Utxo};
use crate::wallet::{
    detect_all_btc_providers, get_connected_l1_provider, get_connected_pubkey,
    is_sats_connect_provider, resolve_btc_provider,
};

// ── Network constants ─────────────────────────────────────────────

const MEMPOOL_BASE: &str = "https://mempool.space/testnet4/api";

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
/// - `parent_inscription_id`: optional Ordinals parent ID (`<txid>i0`). When
///   set, tag 3 is embedded in the envelope script, creating an on-chain
///   cryptographic parent-child link (e.g. process template → institution).
/// - `parent_utxo`: the UTXO currently holding the parent inscription's sat.
///   **Required by the Ordinals provenance rule** — without spending the parent
///   UTXO as an input of the reveal tx, the indexer ignores tag 3 entirely.
///   Use `fetch_tx_output` to retrieve this from an already-broadcast (possibly
///   unconfirmed) transaction.
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
    let admin_pubkey = pubkey_hex
        .parse::<bitcoin::key::XOnlyPublicKey>()
        .map_err(|e| format!("Invalid x-only pubkey: {e}"))?;

    let address_str = get_connected_address()?;
    let network = Network::Testnet4;
    let change_address: Address = address_str
        .parse::<Address<bitcoin::address::NetworkUnchecked>>()
        .map_err(|e| format!("Invalid wallet address '{address_str}': {e}"))?
        .require_network(network)
        .map_err(|_| format!("Address '{address_str}' is not testnet4"))?;

    // ── 2. Fetch UTXOs from Mempool.space ─────────────────────────
    let utxos = fetch_utxos(&address_str, &change_address).await?;
    if utxos.is_empty() {
        return Err(format!(
            "No UTXOs found for {address_str} on testnet4. Fund the address first."
        ));
    }

    // Filter out inscription-carrier UTXOs (≤ 546 sats dust) from the funding
    // pool used for coin selection.  Inscription sats must never be spent as
    // commit inputs — doing so causes the wallet to warn "inscriptions will be
    // spent" and risks destroying those inscriptions if change routing is wrong.
    // The parent UTXO is passed separately and explicitly; it is never in this list.
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

    // ── 3. Build commit + reveal transactions ─────────────────────
    let plan = build_commit_reveal(
        &admin_pubkey,
        entity,
        &funding_utxos,
        fee_rate,
        &change_address,
        network,
        parent_inscription_id,
        parent_utxo,
    )?;

    let estimated_fees = plan.estimated_fee;

    // ── 4. Sign commit tx via wallet signPsbt ─────────────────────
    let commit_psbt = commit_to_psbt(&plan)?;
    let commit_psbt_b64 = psbt_to_base64(&commit_psbt);

    let signed_commit_b64 = wallet_sign_psbt(&commit_psbt_b64, &address_str).await?;
    let commit_raw_hex = finalize_psbt_to_hex(&signed_commit_b64)?;

    // ── 5. Broadcast commit tx ────────────────────────────────────
    let commit_txid = broadcast_tx(&commit_raw_hex).await?;
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] Commit tx broadcast: {commit_txid}"
    )));

    // ── 6. Build reveal PSBT (now we know the real commit txid) ───
    // txbuilder already embedded the correct commit txid (computed from the
    // unsigned tx). After signing, the txid doesn't change (witness data is
    // segwit so it goes into wtxid, not txid). So the reveal's input is correct.
    let reveal_psbt = build_reveal_psbt(&plan)?;
    let reveal_psbt_b64 = psbt_to_base64(&reveal_psbt);

    let signed_reveal_b64 = wallet_sign_psbt(&reveal_psbt_b64, &address_str).await?;
    let reveal_raw_hex = finalize_psbt_to_hex(&signed_reveal_b64)?;

    // ── 7. Broadcast reveal tx ────────────────────────────────────
    let reveal_txid = broadcast_tx(&reveal_raw_hex).await?;
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] Reveal tx broadcast: {reveal_txid}"
    )));

    let inscription_id = format!("{reveal_txid}i0");

    // The inscription sat lives at `plan.inscription_vout()` of the reveal tx
    // (output 1 when a parent was spent, since the parent is returned in output 0).
    // Build the UTXO descriptor so callers can pass it as `parent_utxo`
    // to the next child inscription without waiting for confirmation.
    //
    // reveal_txid comes from mempool.space (already trimmed) and should
    // always be a valid 64-hex txid.  If somehow it's not, we still
    // return Ok — the reveal already broadcast successfully.  The caller
    // (stack_ui) derives reveal_utxo from `fetch_tx_output` as a fallback.
    let inscription_vout = plan.inscription_vout();
    let inscription_out = &plan.reveal_tx.output[inscription_vout as usize];
    let reveal_utxo = if let Ok(txid_parsed) = reveal_txid.parse::<Txid>() {
        Utxo {
            outpoint: OutPoint::new(txid_parsed, inscription_vout),
            amount: inscription_out.value,
            script_pubkey: inscription_out.script_pubkey.clone(),
        }
    } else {
        web_sys::console::warn_1(&JsValue::from_str(&format!(
            "[BINST] Could not parse reveal txid '{}' as Txid — reveal_utxo will be a placeholder",
            reveal_txid
        )));
        // Placeholder — callers that need the real UTXO should use fetch_tx_output.
        Utxo {
            outpoint: OutPoint::null(),
            amount: inscription_out.value,
            script_pubkey: inscription_out.script_pubkey.clone(),
        }
    };

    Ok(InscribeResult {
        commit_txid,
        reveal_txid,
        inscription_id,
        total_fees: estimated_fees,
        reveal_utxo,
    })
}

// ── UTXO fetching ─────────────────────────────────────────────────

async fn fetch_utxos(address: &str, script_address: &Address) -> Result<Vec<Utxo>, String> {
    let url = format!("{MEMPOOL_BASE}/address/{address}/utxo");
    let text = fetch_text(&url).await.map_err(|e| {
        format!("Failed to fetch UTXOs from Mempool.space: {e}")
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

// ── Wallet PSBT signing ───────────────────────────────────────────

/// Sign a PSBT via the connected BTC wallet.
///
/// Two paths:
///  1. **Sats-connect** (Xverse): `provider.request("signPsbt", { psbt, signInputs, broadcast: false })`
///     → response `{ result: { psbt: "<base64>" } }` or `{ status: "success", result: { psbt } }`
///  2. **Direct** (UniSat, OKX): `provider.signPsbt(psbtHex, options)` → hex string.
///
/// `signing_address` is used in both paths so the wallet knows which key to
/// use for each input.
async fn wallet_sign_psbt(psbt_b64: &str, signing_address: &str) -> Result<String, String> {
    // Resolve the connected provider
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

    // Detect sats-connect (Xverse): has request + connect, no requestAccounts
    if is_sats_connect_provider(&provider) {
        return wallet_sign_psbt_sats_connect(&provider, psbt_b64, signing_address, &window_path).await;
    }

    // Direct path: signPsbt() / signPSBT()
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

/// Which PSBT inputs to ask a direct-API wallet to sign: the Tapscript input
/// (the inscription commit — input 1 in a child reveal, input 0 otherwise) and
/// any input whose UTXO belongs to `signing_address` (commit funding UTXOs, the
/// parent sat). Anything else is skipped.
fn should_sign_input(is_tapscript: bool, utxo_addr: Option<&str>, signing_address: &str) -> bool {
    is_tapscript || utxo_addr == Some(signing_address)
}

/// Direct `signPsbt(psbtHex, options)` call — used by UniSat, OKX, etc.
///
/// Key differences from sats-connect (Xverse):
/// - UniSat takes **hex**, not base64.
/// - UniSat returns **hex** (not base64 or `{ psbt }`).
/// - For multi-input PSBTs, `toSignInputs` must be provided explicitly so
///   UniSat knows which key to use for each input.
/// - The commit input of the reveal PSBT (input 0 for a root, input 1 for a
///   child) is a Tapscript (script-path) spend.
///   `disableTweakSigner: true` is required — it tells UniSat to sign with
///   the raw (untweaked) private key, which is what Tapscript CHECKSIG needs.
/// - The parent UTXO (input 0 of a child reveal) is a P2TR key-path spend.
///   We include it only when it is owned by `signing_address` (i.e. the sat
///   has been transferred to the wallet's ordinals address).  If it lives at
///   a NUMS-tweaked inscription address, UniSat cannot sign it key-path and
///   we skip it (the input remains unsigned; the broadcast will fail — but
///   that is a separate problem from the PSBT params rejection we are fixing
///   here).
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

    // Build toSignInputs: one entry per input that we want UniSat to sign.
    //
    // disableTweakSigner controls which signing key UniSat uses:
    //   false (default): tweaked key  — correct for plain P2TR key-path spends
    //                                   (commit UTXO at wallet address, parent sat at wallet address)
    //   true:            raw key      — correct for Tapscript CHECKSIG leaf spends
    //                                   (reveal input 0, which has tap_scripts in the PSBT)
    //
    // Detecting Tapscript inputs: the PSBT input has a non-empty tap_scripts map.
    // Plain key-path P2TR inputs have no tap_scripts.
    let network = Network::Testnet4;
    let to_sign_inputs = js_sys::Array::new();

    for (i, inp) in psbt.inputs.iter().enumerate() {
        let utxo_addr = inp.witness_utxo.as_ref().and_then(|txout| {
            Address::from_script(&txout.script_pubkey, network)
                .ok()
                .map(|a| a.to_string())
        });

        let should_sign = should_sign_input(!inp.tap_scripts.is_empty(), utxo_addr.as_deref(), signing_address);

        if !should_sign {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "[BINST] signPsbt: skipping input {i} (UTXO addr {:?} ≠ wallet addr {})",
                utxo_addr, &signing_address[..signing_address.len().min(20)]
            )));
            continue;
        }

        // Use disableTweakSigner only for Tapscript-path inputs (those that have
        // tap_scripts populated).  Plain key-path P2TR inputs (commit UTXO, parent sat)
        // must be signed with the tweaked key — disableTweakSigner would break them.
        let is_tapscript = !inp.tap_scripts.is_empty();

        let entry = js_sys::Object::new();
        js_sys::Reflect::set(&entry, &JsValue::from_str("index"), &JsValue::from_f64(i as f64)).ok();
        js_sys::Reflect::set(&entry, &JsValue::from_str("address"), &JsValue::from_str(signing_address)).ok();
        js_sys::Reflect::set(&entry, &JsValue::from_str("disableTweakSigner"), &JsValue::from_bool(is_tapscript)).ok();
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

// ── PSBT finalization ─────────────────────────────────────────────

/// Finalize a signed PSBT and extract the raw hex transaction.
///
/// Wallets return the signed PSBT in different encodings:
/// - UniSat returns hex (with autoFinalized: false → sigs in tap_key_sig /
///   tap_script_sigs, NOT in final_script_witness)
/// - Xverse / OKX return base64 (already finalized — final_script_witness set)
///
/// We try hex first (pure [0-9a-fA-F] chars), then fall back to base64.
/// After decoding we call `finalize_psbt_inputs` to move tap signatures into
/// `final_script_witness` before calling `extract_tx`.
fn finalize_psbt_to_hex(signed_psbt: &str) -> Result<String, String> {
    let s = signed_psbt.trim();

    // Detect hex: all chars are hex digits and length is even
    let psbt_bytes = if s.len() % 2 == 0 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        hex_decode(s).map_err(|e| format!("PSBT hex decode failed: {e}"))?
    } else {
        STANDARD
            .decode(s)
            .map_err(|e| format!("PSBT base64 decode failed: {e}"))?
    };

    let mut psbt = Psbt::deserialize(&psbt_bytes)
        .map_err(|e| format!("Failed to deserialize signed PSBT: {e}"))?;

    // Move tap signatures into final_script_witness for any inputs that are
    // not yet finalized (UniSat with autoFinalized:false leaves sigs in
    // tap_key_sig / tap_script_sigs rather than final_script_witness).
    finalize_psbt_inputs(&mut psbt)?;

    let tx = psbt
        .extract_tx()
        .map_err(|e| format!("PSBT not fully signed — cannot extract tx: {e}"))?;

    let raw = bitcoin::consensus::encode::serialize(&tx);
    Ok(hex_encode(&raw))
}

/// Finalize PSBT inputs that carry Taproot signatures but have not yet had
/// their `final_script_witness` set.
///
/// Two cases handled:
///
/// 1. **Key-path P2TR** (`tap_scripts` empty, `tap_key_sig` set):
///    `witness = [<64-byte schnorr sig>]`
///
/// 2. **Script-path / Tapscript** (`tap_scripts` non-empty, `tap_script_sigs` set):
///    `witness = [<sig>, <script>, <control_block>]`
///    (BIP-341 script-path witness stack, bottom-to-top order)
///
/// Inputs that already have `final_script_witness` set are left untouched
/// (Xverse sets it; we don't want to overwrite a valid finalization).
fn finalize_psbt_inputs(psbt: &mut Psbt) -> Result<(), String> {
    use bitcoin::Witness;

    for (i, inp) in psbt.inputs.iter_mut().enumerate() {
        // Already finalized — nothing to do.
        if inp.final_script_witness.is_some() {
            continue;
        }

        if inp.tap_scripts.is_empty() {
            // ── Key-path spend ────────────────────────────────────
            // tap_key_sig should hold a 64-byte Schnorr signature.
            if let Some(sig) = inp.tap_key_sig {
                let mut w = Witness::new();
                w.push(sig.to_vec());
                inp.final_script_witness = Some(w);
                web_sys::console::log_1(&JsValue::from_str(&format!(
                    "[BINST] finalize input {i}: key-path sig → final_script_witness"
                )));
            } else {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "[BINST] finalize input {i}: no tap_key_sig and no final_script_witness — witness will be empty"
                )));
            }
        } else {
            // ── Script-path / Tapscript spend ─────────────────────
            // Find the first tap_script_sigs entry that matches a known leaf.
            // For our inscriptions there is always exactly one leaf.
            let maybe_sig = inp
                .tap_script_sigs
                .values()
                .next()
                .copied();

            let maybe_leaf = inp.tap_scripts.iter().next();

            match (maybe_sig, maybe_leaf) {
                (Some(sig), Some((cb, (script, _)))) => {
                    // BIP-341 script-path witness: <sig> <script> <control_block>
                    let mut w = Witness::new();
                    w.push(sig.to_vec());
                    w.push(script.as_bytes());
                    w.push(cb.serialize());
                    inp.final_script_witness = Some(w);
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "[BINST] finalize input {i}: tapscript sig → final_script_witness ({} script bytes)",
                        script.len()
                    )));
                }
                _ => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[BINST] finalize input {i}: tap_scripts present but no tap_script_sigs — witness will be empty"
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Build a PSBT for the reveal transaction.
fn build_reveal_psbt(plan: &crate::txbuilder::InscriptionPlan) -> Result<Psbt, String> {
    // The reveal tx has a script-path Taproot input. We embed the PSBT
    // input metadata (witness UTXO, tap leaf script, control block) so
    // the wallet knows exactly what to sign.
    use bitcoin::psbt::{Input as PsbtInput};
    use bitcoin::taproot::LeafVersion;

    let mut psbt = Psbt::from_unsigned_tx(plan.reveal_tx.clone())
        .map_err(|e| format!("Reveal PSBT creation failed: {e}"))?;

    // Commit input (Tapscript path — carries the inscription). It is input 0
    // for a root and input 1 when a parent is spent (see txbuilder step 7).
    let commit_idx = plan.commit_input_index();
    let commit_out = plan.commit_tx.output[0].clone();

    let control_block = plan
        .taproot_spend_info
        .control_block(&(plan.inscription_script.clone(), LeafVersion::TapScript))
        .ok_or("Cannot compute control block for reveal")?;

    let mut commit_input = PsbtInput::default();
    commit_input.witness_utxo = Some(commit_out);
    commit_input.tap_scripts.insert(
        control_block.clone(),
        (plan.inscription_script.clone(), LeafVersion::TapScript),
    );
    // tap_leaf_hash for the script-path — wallets use this to know which leaf to sign
    let leaf_hash = bitcoin::taproot::TapLeafHash::from_script(
        &plan.inscription_script,
        LeafVersion::TapScript,
    );
    commit_input.tap_key_origins.insert(
        // Use an all-zero key as placeholder; wallet reads the script from tap_scripts
        plan.taproot_spend_info.output_key().to_x_only_public_key(),
        (vec![leaf_hash], (bitcoin::bip32::Fingerprint::default(), bitcoin::bip32::DerivationPath::default())),
    );
    psbt.inputs[commit_idx] = commit_input;

    // Parent UTXO (optional, input 0 when present): key-path P2TR spend.
    // We provide `witness_utxo` so the wallet knows the value and script.
    // The wallet signs this with the key-path (no tap_scripts needed).
    if let Some(ref p_utxo) = plan.parent_utxo {
        let mut parent_input = PsbtInput::default();
        parent_input.witness_utxo = Some(bitcoin::TxOut {
            value: p_utxo.amount,
            script_pubkey: p_utxo.script_pubkey.clone(),
        });
        let parent_idx = plan
            .reveal_tx
            .input
            .iter()
            .position(|i| i.previous_output == p_utxo.outpoint)
            .ok_or("Reveal tx does not spend the parent UTXO")?;
        psbt.inputs[parent_idx] = parent_input;
    }

    Ok(psbt)
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
    let url = format!("{MEMPOOL_BASE}/tx/{txid}");
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

/// Broadcast a raw transaction hex to Mempool.space testnet4.
/// Returns the txid on success.
async fn broadcast_tx(raw_hex: &str) -> Result<String, String> {
    let url = format!("{MEMPOOL_BASE}/tx");

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
    use bitcoin::key::{TweakedPublicKey, XOnlyPublicKey};
    use binst_inscription::InstitutionBody;
    use std::str::FromStr;

    const ADMIN: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    fn wallet_address() -> Address {
        let pk = XOnlyPublicKey::from_str(ADMIN).unwrap();
        Address::p2tr_tweaked(TweakedPublicKey::dangerous_assume_tweaked(pk), Network::Testnet4)
    }

    fn plan(with_parent: bool) -> crate::txbuilder::InscriptionPlan {
        let pk = XOnlyPublicKey::from_str(ADMIN).unwrap();
        let addr = wallet_address();
        let entity = BinstEntity::Institution(InstitutionBody {
            v: 0,
            name: "TestInst".into(),
            admin: ADMIN.into(),
            citrea_contract: None,
            membership_rune: None,
            description: None,
            website: None,
            vault: None,
        });
        let funding = Utxo {
            outpoint: OutPoint::new(
                Txid::from_str("1111111111111111111111111111111111111111111111111111111111111111").unwrap(),
                0,
            ),
            amount: Amount::from_sat(200_000),
            script_pubkey: addr.script_pubkey(),
        };
        let parent = Utxo {
            outpoint: OutPoint::new(
                Txid::from_str("2222222222222222222222222222222222222222222222222222222222222222").unwrap(),
                0,
            ),
            amount: Amount::from_sat(546),
            script_pubkey: addr.script_pubkey(),
        };
        let parent_id = "2222222222222222222222222222222222222222222222222222222222222222i0";
        build_commit_reveal(
            &pk,
            &entity,
            &[funding],
            2,
            &addr,
            Network::Testnet4,
            with_parent.then_some(parent_id),
            with_parent.then_some(parent),
        )
        .unwrap()
    }

    #[test]
    fn child_reveal_psbt_puts_tapscript_on_input_1_and_parent_on_input_0() {
        let p = plan(true);
        let psbt = build_reveal_psbt(&p).unwrap();
        assert_eq!(psbt.inputs.len(), 2);
        assert!(psbt.inputs[0].tap_scripts.is_empty(), "parent input is a key-path spend");
        assert_eq!(
            psbt.inputs[0].witness_utxo.as_ref().map(|o| o.value),
            Some(p.parent_utxo.as_ref().unwrap().amount),
        );
        assert!(!psbt.inputs[1].tap_scripts.is_empty(), "commit input carries the inscription leaf");
        assert_eq!(psbt.inputs[1].witness_utxo.as_ref(), Some(&p.commit_tx.output[0]));
    }

    #[test]
    fn root_reveal_psbt_keeps_tapscript_on_input_0() {
        let p = plan(false);
        let psbt = build_reveal_psbt(&p).unwrap();
        assert_eq!(psbt.inputs.len(), 1);
        assert!(!psbt.inputs[0].tap_scripts.is_empty());
    }

    #[test]
    fn parent_id_without_parent_utxo_is_refused() {
        let pid = "2222222222222222222222222222222222222222222222222222222222222222i0";
        assert!(require_parent_utxo(Some(pid), None).is_err());
        assert!(require_parent_utxo(None, None).is_ok());
        let p = plan(true);
        assert!(require_parent_utxo(Some(pid), p.parent_utxo.as_ref()).is_ok());
    }

    #[test]
    fn direct_signer_signs_commit_at_any_index_and_only_own_key_path_inputs() {
        let me = wallet_address().to_string();
        // Commit (Tapscript) input lives at the NUMS commit address, not ours: still signed.
        assert!(should_sign_input(true, Some("tb1pcommitaddress"), &me));
        // Parent / funding input at our address: signed.
        assert!(should_sign_input(false, Some(me.as_str()), &me));
        // Foreign key-path input: skipped.
        assert!(!should_sign_input(false, Some("tb1psomeoneelse"), &me));
        assert!(!should_sign_input(false, None, &me));
    }
}
