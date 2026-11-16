// reused-from: binst-pilot-webapp@dda2d1a:src/wallet.rs (2026-11-16) — adapted: wallet-bar UI and DOM hidden-input state removed; session state lives in session.rs
//! Multi-wallet abstraction for BINST.
//!
//! **Two independent wallet slots:**
//!
//! | Slot | Layer | Wallets | Purpose |
//! |------|-------|---------|---------|
//! | L1 (BTC) | Bitcoin inscriptions | UniSat, Xverse, Leather, OKX | Sign PSBTs for commit+reveal |
//! | L2 (EVM) | Citrea smart contracts | MetaMask, SafePal, Brave, any EIP-1193 | Sign EVM transactions |
//!
//! Each slot is independent.  The two slots are stored in separate hidden
//! inputs (`wallet-l1-*` and `wallet-l2-*`) and can be connected / disconnected
//! independently.
//!
//! ## Wallet detection
//! - **L1**: native Bitcoin wallets only (`window.bitcoin`, `window.unisat`,
//!   `window.okxwallet`, etc.).  SafePal is EVM-only from our perspective.
//! - **L2**: any EIP-1193 provider (`window.ethereum`, `window.safepalProvider`,
//!   etc.).  WalletConnect (eip155) is also offered.

use wasm_bindgen::prelude::*;


// ── Wallet kind ───────────────────────────────────────────────────

/// Which concrete wallet implementation is backing a connected slot.
///
/// `BtcProvider` and `EvmProvider` are the capability-based generic variants —
/// used when a wallet is detected by its API shape rather than its brand name.
/// The named variants are retained for wallets with proprietary injection points
/// (`window.unisat`, `window.safepalProvider`) that are not yet covered by a
/// cross-wallet standard.
#[derive(Debug, Clone, PartialEq)]
pub enum WalletKind {
    /// Any BTC wallet that exposes `window.bitcoin` or `window.unisat` with
    /// `requestAccounts()` + `signMessage()`.  The `name` field is read from
    /// the provider object's `.name` property if present.
    BtcProvider { window_key: &'static str, name: String },
    /// Any EVM wallet that exposes `window.ethereum` implementing EIP-1193
    /// (`provider.request({ method })`).  The `name` field is read from
    /// `provider.networkVersion` / `isMetaMask` / `isCoinbaseWallet` etc.
    EvmProvider { name: String },
}

impl WalletKind {
    pub fn name(&self) -> &str {
        match self {
            WalletKind::BtcProvider { name, .. } => name,
            WalletKind::EvmProvider { name } => name,
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            WalletKind::BtcProvider { .. } => "₿",
            WalletKind::EvmProvider { .. } => "⬡",
        }
    }
}

// ── WalletInfo ───────────────────────────────────────────────────

/// Info returned from a successful wallet connection.
pub struct WalletInfo {
    pub address: String,
    pub public_key: String,
    pub network: String,
    pub kind: WalletKind,
}

// ── Detection helpers ─────────────────────────────────────────────

/// Candidate BTC provider injection points, in preference order.
///
/// Only **native Bitcoin** wallets are listed here.  SafePal is intentionally
/// omitted — its BTC provider (`safepalBRC20Provider`) is unreliable across
/// browser/extension versions, and SafePal users can connect via the L2 EVM
/// slot (`window.safepalProvider`) instead.
///
/// Probed in order; first capable one wins for the auto-select path;
/// all capable ones are shown in the L1 picker.
pub const BTC_WINDOW_KEYS: &[&str] = &[
    "unisat",           // UniSat — best inscription API
    "bitcoin",          // generic cross-wallet standard (Leather, Brave, etc.)
    "BitcoinProvider",  // Xverse (also exposed as XverseProviders.BitcoinProvider)
    "okxwallet",        // OKX Wallet (bitcoin sub-key handled in detect_all_btc_providers)
    "btc",              // generic fallback
    "magicEden",        // Magic Eden wallet
    "leather",          // Leather (Hiro) wallet
];

/// Display-name overrides for keys whose `provider.name` property is absent
/// or not human-readable.
fn btc_key_display_name(key: &str, provider_name: &str, _obj: &JsValue) -> String {
    if !provider_name.is_empty()
        && provider_name != key
        && provider_name.len() < 40
    {
        return provider_name.to_string();
    }
    match key {
        "unisat"          => "UniSat".to_string(),
        "okxwallet"       => "OKX Wallet".to_string(),
        "bitcoin"         => "Bitcoin Wallet".to_string(),
        "BitcoinProvider" => "Xverse".to_string(),
        "magicEden"       => "Magic Eden".to_string(),
        "leather"         => "Leather".to_string(),
        other             => other.to_string(),
    }
}

/// A discovered BTC provider: the JS object and a human-readable name.
pub struct BtcProviderHandle {
    /// How to re-obtain the object from `window` (used when connecting).
    /// `"safepalProvider.bitcoin"` uses dot-path lookup.
    pub window_path: &'static str,
    pub display_name: String,
    /// Data-URI or HTTPS URL of the wallet's own icon, if the provider
    /// exposes one (EIP-6963 `provider.icon`).  `None` → use emoji fallback.
    pub icon_url: Option<String>,
}

/// Probe all known BTC injection points and return every one that can sign.
///
/// "Can sign" means the object has **any** of:
///   - `requestAccounts()` — standard account-request  
///   - `getAccounts()`     — alternative used by some wallets  
///   - `getPublicKey()`    — enough to identify the wallet; accounts follow
///   - `signMessage()`     — minimum needed for BINST auth
///   - `signPSBT()`        — PSBT signing (not always present but proves BTC capability)
///
/// We intentionally do **not** require PSBT support — the BINST bridge signs
/// the actual inscription PSBTs server-side.  The browser wallet only needs to
/// provide an address/pubkey and optionally sign a challenge message.
pub fn detect_all_btc_providers() -> Vec<BtcProviderHandle> {
    let Some(win) = web_sys::window() else { return vec![] };
    let mut found: Vec<BtcProviderHandle> = Vec::new();

    // ── 1. Top-level window keys ─────────────────────────────────
    // Covers: UniSat, SafePal (safepalProvider), Leather, Xverse, OKX, etc.
    for &key in BTC_WINDOW_KEYS {
        let Ok(obj) = js_sys::Reflect::get(&win, &JsValue::from_str(key)) else { continue };
        if obj.is_undefined() || obj.is_null() { continue; }
        // `safepalBRC20Provider` is the dedicated SafePal BTC key — no guard needed.
        // (The old `safepalProvider` key is EVM-only and is no longer in BTC_WINDOW_KEYS.)
        if !is_btc_capable(&obj) { continue; }
        let raw_name = read_provider_name(&obj, "");
        let name = btc_key_display_name(key, &raw_name, &obj);
        let icon_url = read_provider_icon(&obj);
        found.push(BtcProviderHandle { window_path: key, display_name: name, icon_url });
    }

    // ── 2. Nested `.bitcoin` objects on multi-chain providers ────
    // OKX, Phantom, Bitget, TrustWallet, and Brave expose BTC under <root>.bitcoin.
    // We deliberately exclude `ethereum` here — window.ethereum is always
    // the EVM provider (MetaMask etc.) and must not be listed as a BTC wallet.
    let nested_roots: &[(&str, &str)] = &[
        ("okxwallet",    "OKX Wallet"),
        ("bitkeep",      "Bitget"),
        ("phantom",      "Phantom"),
        ("trustwallet",  "Trust Wallet"),
    ];
    for &(root, label) in nested_roots {
        // Skip if the root itself was already picked up in step 1
        if found.iter().any(|h| h.window_path == root) { continue; }
        let Ok(root_obj) = js_sys::Reflect::get(&win, &JsValue::from_str(root)) else { continue };
        if root_obj.is_undefined() || root_obj.is_null() { continue; }
        let Ok(btc) = js_sys::Reflect::get(&root_obj, &JsValue::from_str("bitcoin")) else { continue };
        if btc.is_undefined() || btc.is_null() { continue; }
        if !is_btc_capable(&btc) { continue; }
        let path: &'static str = Box::leak(format!("{root}.bitcoin").into_boxed_str());
        let raw_name = read_provider_name(&btc, "");
        let name = if raw_name.is_empty() { label.to_string() } else { raw_name };
        let icon_url = read_provider_icon(&btc).or_else(|| read_provider_icon(&root_obj));
        found.push(BtcProviderHandle { window_path: path, display_name: name, icon_url });
    }

    // ── 2b. Xverse — window.XverseProviders.BitcoinProvider ──────
    // Xverse injects XverseProviders with a nested BitcoinProvider that has
    // a sats-connect `request()` method.  The top-level `window.BitcoinProvider`
    // is a legacy shim that doesn't support sats-connect, so when the nested
    // provider exists we **replace** the step-1 entry with this better path.
    if let Ok(xp) = js_sys::Reflect::get(&win, &JsValue::from_str("XverseProviders")) {
        if !xp.is_undefined() && !xp.is_null() {
            if let Ok(bp) = js_sys::Reflect::get(&xp, &JsValue::from_str("BitcoinProvider")) {
                if !bp.is_undefined() && !bp.is_null() && is_btc_capable(&bp) {
                    let path: &'static str = "XverseProviders.BitcoinProvider";
                    let raw_name = read_provider_name(&bp, "");
                    let name = if raw_name.is_empty() { "Xverse".to_string() } else { raw_name };
                    let icon_url = read_provider_icon(&bp);
                    // Replace existing BitcoinProvider / Xverse entry if any
                    if let Some(pos) = found.iter().position(|h| {
                        h.window_path == "BitcoinProvider"
                            || h.display_name.to_lowercase().contains("xverse")
                    }) {
                        found[pos] = BtcProviderHandle { window_path: path, display_name: name, icon_url };
                    } else {
                        found.push(BtcProviderHandle { window_path: path, display_name: name, icon_url });
                    }
                }
            }
        }
    }

    // ── 3. EIP-5749 window.providers[] array ─────────────────────
    if let Ok(arr) = js_sys::Reflect::get(&win, &JsValue::from_str("providers")) {
        if let Some(arr) = arr.dyn_ref::<js_sys::Array>() {
            for i in 0..arr.length() {
                let item = arr.get(i);
                if item.is_undefined() || item.is_null() { continue; }
                let (btc_obj, suffix) = if let Ok(b) = js_sys::Reflect::get(&item, &JsValue::from_str("bitcoin")) {
                    if !b.is_undefined() && !b.is_null() && is_btc_capable(&b) {
                        (b, ".bitcoin")
                    } else if is_btc_capable(&item) {
                        (item.clone(), "")
                    } else { continue; }
                } else if is_btc_capable(&item) {
                    (item.clone(), "")
                } else { continue; };
                let path: &'static str = Box::leak(format!("providers[{i}]{suffix}").into_boxed_str());
                if found.iter().any(|h| h.window_path == path) { continue; }
                let name = read_provider_name(&btc_obj, &format!("Wallet {i}"));
                let icon_url = read_provider_icon(&btc_obj);
                found.push(BtcProviderHandle { window_path: path, display_name: name, icon_url });
            }
        }
    }

    // ── 4. WBIP004 — window.wbip_providers[] ─────────────────────
    // The emerging Bitcoin standard for multi-wallet discovery (like EIP-6963
    // for EVM).  Each wallet pushes { id, name, icon, methods?, ... } into
    // the array. The `id` field is the window path to the provider object.
    for wbip_key in &["wbip_providers", "btc_providers"] {
        let Ok(arr_val) = js_sys::Reflect::get(&win, &JsValue::from_str(wbip_key)) else { continue };
        let Some(arr) = arr_val.dyn_ref::<js_sys::Array>() else { continue };
        for i in 0..arr.length() {
            let entry = arr.get(i);
            if entry.is_undefined() || entry.is_null() { continue; }
            // Read provider metadata
            let id = js_sys::Reflect::get(&entry, &JsValue::from_str("id"))
                .ok().and_then(|v| v.as_string()).unwrap_or_default();
            if id.is_empty() { continue; }
            // Skip if we already found a provider at this path
            if found.iter().any(|h| h.window_path == id.as_str()) { continue; }
            // Resolve the provider object from window[id]
            let provider = resolve_btc_provider(&id);
            let is_capable = provider.as_ref().map(|p| is_btc_capable(p)).unwrap_or(false);
            if !is_capable { continue; }
            let wbip_name = js_sys::Reflect::get(&entry, &JsValue::from_str("name"))
                .ok().and_then(|v| v.as_string()).unwrap_or_else(|| id.clone());
            let wbip_icon = js_sys::Reflect::get(&entry, &JsValue::from_str("icon"))
                .ok().and_then(|v| v.as_string())
                .filter(|s| s.starts_with("data:image") || s.starts_with("https://"));
            let path: &'static str = Box::leak(id.into_boxed_str());
            web_sys::console::log_1(&JsValue::from_str(
                &format!("[WBIP004] discovered: {wbip_name} (id={path})")
            ));
            found.push(BtcProviderHandle { window_path: path, display_name: wbip_name, icon_url: wbip_icon });
        }
    }

    // Dedup: if the same underlying JS object appeared via multiple paths,
    // keep only the first entry (which has the more specific path).
    // We can't use Object.is across wasm_bindgen easily, so dedup by
    // normalized name (strip trailing " wallet", case-insensitive).
    let mut seen_names: Vec<String> = Vec::new();
    found.retain(|h| {
        let lc = h.display_name.to_lowercase();
        let normalized = lc.trim_end_matches(" wallet").trim().to_string();
        if seen_names.contains(&normalized) { false } else { seen_names.push(normalized); true }
    });

    found
}

/// Emit a diagnostic `console.log` listing every window key that is not
/// undefined/null, plus which ones look BTC-capable.  Called automatically
/// when the wallet picker opens so developers can see what's injected.
pub fn debug_scan_window_btc_keys() {
    let Some(win) = web_sys::window() else { return };

    // All method names we probe across all wallet APIs
    let all_methods = [
        "requestAccounts", "getAccounts", "getPublicKey", "signMessage", "signPSBT",
        "getSelectedAccount", "sendBitcoin", "connect", "request",
        "getAccount", "signTransaction", "on", "disconnect",
    ];

    let probe_keys = [
        "unisat", "bitcoin", "BitcoinProvider", "XverseProviders",
        "okxwallet", "btc", "bitkeep", "phantom", "magicEden", "leather",
        "trustwallet", "safepalProvider", "ethereum", "braveEthereum", "brave",
        "providers", "coinbaseWalletExtension", "rabby", "tally", "frame",
        "wbip_providers", "btc_providers",
    ];

    let all_flags = [
        "isBraveWallet", "isMetaMask", "isCoinbaseWallet", "isOKExWallet",
        "isSafePal", "isBitget", "isPhantom", "isTrust",
    ];

    let mut btc_capable: Vec<String> = Vec::new();

    for key in probe_keys {
        let Ok(obj) = js_sys::Reflect::get(&win, &JsValue::from_str(key)) else { continue };
        if obj.is_undefined() || obj.is_null() { continue; }

        // Log methods present on the top-level key
        let top_methods: Vec<&str> = all_methods.iter().copied()
            .filter(|m| has_fn(&obj, m))
            .collect();
        let top_flags: Vec<&str> = all_flags.iter().copied()
            .filter(|f| has_bool_flag(&obj, f))
            .collect();
        if !top_methods.is_empty() || !top_flags.is_empty() {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[BINST]   window.{key}  methods:[{}]  flags:[{}]",
                top_methods.join(", "), top_flags.join(", ")
            )));
        }
        if is_btc_capable(&obj) {
            btc_capable.push(format!("window.{key}"));
        }

        // Probe .bitcoin sub-key and log its methods + flags
        if let Ok(btc) = js_sys::Reflect::get(&obj, &JsValue::from_str("bitcoin")) {
            if !btc.is_undefined() && !btc.is_null() {
                let btc_methods: Vec<&str> = all_methods.iter().copied()
                    .filter(|m| has_fn(&btc, m))
                    .collect();
                let btc_flags: Vec<&str> = all_flags.iter().copied()
                    .filter(|f| has_bool_flag(&btc, f))
                    .collect();
                if !btc_methods.is_empty() || !btc_flags.is_empty() {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "[BINST]   window.{key}.bitcoin  methods:[{}]  flags:[{}]",
                        btc_methods.join(", "), btc_flags.join(", ")
                    )));
                }
                if is_btc_capable(&btc) {
                    btc_capable.push(format!("window.{key}.bitcoin"));
                }
            }
        }
    }

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] BTC-capable providers: [{}]",
        if btc_capable.is_empty() { "none".to_string() } else { btc_capable.join(", ") },
    )));
}

/// True if `obj` exposes enough API to be useful as a BTC wallet.
/// We require only ONE of these — the bar is intentionally low because
/// the bridge handles PSBTs server-side.
fn is_btc_capable(obj: &JsValue) -> bool {
    // Unambiguously BTC-only methods — any one is sufficient
    if has_fn(obj, "requestAccounts")
        || has_fn(obj, "getAccounts")
        || has_fn(obj, "getPublicKey")
        || has_fn(obj, "signPSBT")
        || has_fn(obj, "getSelectedAccount")   // OKX bitcoin namespace
        || has_fn(obj, "sendBitcoin")           // UniSat / OKX bitcoin namespace
        || has_fn(obj, "getAccount")            // Bitget / Trust bitcoin namespace
        || has_fn(obj, "signTransaction")       // Bitget / Trust bitcoin namespace
    {
        return true;
    }
    // `signMessage` alone is ambiguous (EVM wallets have it too); require a
    // second BTC-specific indicator alongside it.
    if has_fn(obj, "signMessage") && (has_fn(obj, "request") || has_fn(obj, "connect")) {
        // Narrow further: must also NOT look like a plain EVM provider.
        // Plain EVM wallets expose `chainId` / `selectedAddress` / `networkVersion`
        // but never expose `signPSBT`, `sendBitcoin`, etc. (already checked above).
        // A SafePal-style BTC provider only has {request, on, signMessage} and
        // sets isSafePal=true — that's enough.
        return true;
    }
    // Sats-connect / WBIP pattern (Xverse, Leather via sats-connect):
    // The provider only has `request()` and maybe `addListener` / `connect`.
    // EVM wallets also have `request()`, so we need a negative guard:
    // reject if it has EVM-specific properties (`eth_chainId`, `isMetaMask`,
    // `selectedAddress`, `chainId` as a string property).
    if has_fn(obj, "request") {
        let looks_evm = has_fn(obj, "enable")
            || js_sys::Reflect::get(obj, &JsValue::from_str("selectedAddress"))
                .ok().map(|v| !v.is_undefined()).unwrap_or(false)
            || js_sys::Reflect::get(obj, &JsValue::from_str("chainId"))
                .ok().map(|v| v.as_string().is_some()).unwrap_or(false)
            || has_bool_flag(obj, "isMetaMask")
            || has_bool_flag(obj, "isCoinbaseWallet")
            || has_bool_flag(obj, "isRabby")
            || has_bool_flag(obj, "isBraveWallet");
        if !looks_evm {
            return true;
        }
    }
    // `connect` alone (Phantom BTC pattern) — Phantom's bitcoin sub-object has
    // `connect`, `signMessage`, `signPSBT`.  `signPSBT` is caught above, so this
    // branch covers a hypothetical provider with only `connect`; keep narrow.
    false
}

/// Read a wallet's self-reported name; fall back to `fallback`.
fn read_provider_name(obj: &JsValue, fallback: &str) -> String {
    js_sys::Reflect::get(obj, &JsValue::from_str("name"))
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Read the wallet's own icon from the provider object.
/// Wallets that follow EIP-6963 (or just good practice) expose
/// `provider.icon` as a data-URI or HTTPS URL.
pub fn read_provider_icon(obj: &JsValue) -> Option<String> {
    js_sys::Reflect::get(obj, &JsValue::from_str("icon"))
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        // Only accept data-URIs and https — reject random garbage values
        .filter(|s| s.starts_with("data:image") || s.starts_with("https://"))
}

/// Return true if `obj[flag]` is the boolean `true`.
fn has_bool_flag(obj: &JsValue, flag: &str) -> bool {
    js_sys::Reflect::get(obj, &JsValue::from_str(flag))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Resolve a dot-path window key like `"safepalProvider.bitcoin"` to the JS object.
pub fn resolve_btc_provider(window_path: &str) -> Option<JsValue> {
    let win = web_sys::window()?;
    let parts: Vec<&str> = window_path.split('.').collect();
    let mut obj = js_sys::Reflect::get(&win, &JsValue::from_str(parts[0])).ok()?;
    for part in &parts[1..] {
        obj = js_sys::Reflect::get(&obj, &JsValue::from_str(part)).ok()?;
        if obj.is_undefined() || obj.is_null() { return None; }
    }
    Some(obj)
}

/// Return the display name of the first EIP-1193 EVM provider at
/// `window.ethereum` — without checking `isMetaMask` or any other brand flag.
pub fn detect_evm_provider() -> Option<(String, Option<String>)> {
    let win = web_sys::window()?;
    let Ok(eth) = js_sys::Reflect::get(&win, &JsValue::from_str("ethereum")) else { return None };
    if eth.is_undefined() || eth.is_null() { return None; }
    // Must implement EIP-1193: provider.request() is required
    if !has_fn(&eth, "request") { return None; }
    // Best-effort display name: prefer provider.name, then scan well-known flags
    let name = js_sys::Reflect::get(&eth, &JsValue::from_str("name"))
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .or_else(|| evm_provider_name_from_flags(&eth))
        .unwrap_or_else(|| "EVM Wallet".to_string());
    let icon_url = read_provider_icon(&eth);
    Some((name, icon_url))
}

// ── EIP-6963 Multi Injected Provider Discovery ───────────────────
//
// Standard event-based wallet discovery — eliminates all ad-hoc
// `window.ethereum` scanning, `isMetaMask` flag sniffing, and
// `providers[]` array walking.
//
// **How it works:**
// 1. On startup we register a listener for `eip6963:announceProvider`.
// 2. We dispatch `eip6963:requestProvider` to ask all installed wallets
//    to re-announce.
// 3. Each wallet sends a CustomEvent whose `detail` contains:
//    `{ info: { uuid, name, icon, rdns }, provider: EIP1193Provider }`.
// 4. We store them in a `thread_local! RefCell<Vec>`.
// 5. When the picker opens, `get_eip6963_providers()` returns the list.

use std::cell::RefCell;

/// One EIP-6963 announced wallet.
#[derive(Clone)]
pub struct Eip6963Provider {
    /// Unique session ID (changes each page load)
    pub uuid: String,
    /// Human-readable wallet name (e.g. "MetaMask")
    pub name: String,
    /// Data URI (SVG or PNG) of the wallet icon — provided by the wallet
    pub icon: String,
    /// Reverse DNS identifier (e.g. "io.metamask", "com.coinbase.wallet")
    pub rdns: String,
    /// The actual EIP-1193 provider object — call `.request()` on this
    pub provider: JsValue,
}

thread_local! {
    static EIP6963_PROVIDERS: RefCell<Vec<Eip6963Provider>> = RefCell::new(Vec::new());
    /// The EIP-1193 provider object that the user actually connected through.
    /// Set by `wallet_picker.rs` during the L2 connect flow, read by `citrea.rs`
    /// for `eth_sendTransaction` / `eth_call`.  This ensures we always talk to
    /// the wallet the user chose — not whichever extension grabbed `window.ethereum`.
    static CONNECTED_L2_PROVIDER: RefCell<Option<JsValue>> = RefCell::new(None);
}

/// Store the EIP-1193 provider the user connected through.
/// Called from `wallet_picker.rs` after `evm_connect_and_switch` succeeds.
pub fn set_connected_l2_provider(provider: JsValue) {
    CONNECTED_L2_PROVIDER.with(|cell| {
        *cell.borrow_mut() = Some(provider);
    });
}

/// Retrieve the stored L2 provider, falling back to `window.ethereum`.
pub fn get_connected_l2_provider() -> Option<JsValue> {
    CONNECTED_L2_PROVIDER.with(|cell| {
        let stored = cell.borrow().clone();
        if stored.is_some() {
            return stored;
        }
        // Fallback: window.ethereum (legacy path)
        web_sys::window()
            .and_then(|w| js_sys::Reflect::get(&w, &JsValue::from_str("ethereum")).ok())
            .filter(|v| !v.is_undefined() && !v.is_null())
    })
}

/// Clear the stored L2 provider (called on disconnect).
pub fn clear_connected_l2_provider() {
    CONNECTED_L2_PROVIDER.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// Register the `eip6963:announceProvider` listener and request all
/// installed wallets to announce.  Call **once** from `lib.rs` startup.
pub fn setup_eip6963_listener() {
    let Some(win) = web_sys::window() else { return };

    // Listener: receives `CustomEvent { detail: { info, provider } }`
    let callback = Closure::wrap(Box::new(move |event: web_sys::CustomEvent| {
        let detail = event.detail();
        if detail.is_undefined() || detail.is_null() { return; }

        // Extract detail.info
        let info = match js_sys::Reflect::get(&detail, &JsValue::from_str("info")) {
            Ok(v) if !v.is_undefined() && !v.is_null() => v,
            _ => return,
        };

        // Extract detail.provider
        let provider = match js_sys::Reflect::get(&detail, &JsValue::from_str("provider")) {
            Ok(v) if !v.is_undefined() && !v.is_null() => v,
            _ => return,
        };

        // Read info fields
        let uuid = js_sys::Reflect::get(&info, &JsValue::from_str("uuid"))
            .ok().and_then(|v| v.as_string()).unwrap_or_default();
        let name = js_sys::Reflect::get(&info, &JsValue::from_str("name"))
            .ok().and_then(|v| v.as_string()).unwrap_or_else(|| "EVM Wallet".to_string());
        let icon = js_sys::Reflect::get(&info, &JsValue::from_str("icon"))
            .ok().and_then(|v| v.as_string()).unwrap_or_default();
        let rdns = js_sys::Reflect::get(&info, &JsValue::from_str("rdns"))
            .ok().and_then(|v| v.as_string()).unwrap_or_default();

        if uuid.is_empty() { return; }

        EIP6963_PROVIDERS.with(|cell| {
            let mut providers = cell.borrow_mut();
            // Dedup by UUID (same wallet re-announcing)
            if !providers.iter().any(|p| p.uuid == uuid) {
                web_sys::console::log_1(&JsValue::from_str(
                    &format!("[EIP-6963] discovered: {name} (rdns={rdns})")
                ));
                providers.push(Eip6963Provider { uuid, name, icon, rdns, provider });
            }
        });
    }) as Box<dyn Fn(web_sys::CustomEvent)>);

    win.add_event_listener_with_callback(
        "eip6963:announceProvider",
        callback.as_ref().unchecked_ref(),
    ).ok();
    callback.forget();

    // Dispatch requestProvider — triggers all installed wallets to announce
    if let Ok(evt) = web_sys::Event::new("eip6963:requestProvider") {
        win.dispatch_event(&evt).ok();
    }
}

/// Re-dispatch `eip6963:requestProvider` to trigger late-arriving wallets.
/// Called every time the picker opens, cheap no-op if all wallets already announced.
pub fn request_eip6963_providers() {
    if let Some(win) = web_sys::window() {
        if let Ok(evt) = web_sys::Event::new("eip6963:requestProvider") {
            win.dispatch_event(&evt).ok();
        }
    }
}

/// Return all EIP-6963 announced providers collected so far.
pub fn get_eip6963_providers() -> Vec<Eip6963Provider> {
    EIP6963_PROVIDERS.with(|cell| cell.borrow().clone())
}

/// Legacy fallback: Probe all known EVM injection points and return every distinct provider.
///
/// Returns a list of `(window_key, display_name, icon_url)` tuples.
/// Only used when no EIP-6963 providers are detected (very old wallets).
pub fn detect_all_evm_providers() -> Vec<(String, String, Option<String>)> {
    let Some(win) = web_sys::window() else { return vec![] };
    let mut found: Vec<(String, String, Option<String>)> = Vec::new();
    let mut seen_objs: Vec<JsValue> = Vec::new(); // identity-based dedup

    let add = |obj: &JsValue, window_key: String, fallback: &str,
                   found: &mut Vec<(String, String, Option<String>)>,
                   seen: &mut Vec<JsValue>| {
        if !has_fn(obj, "request") { return; }
        if seen.iter().any(|s| js_sys::Object::is(s, obj)) { return; }
        seen.push(obj.clone());
        let name = if window_key == "braveEthereum" {
            "Brave Wallet".to_string()
        } else if window_key == "safepalProvider" {
            "SafePal".to_string()
        } else {
            js_sys::Reflect::get(obj, &JsValue::from_str("name"))
                .ok()
                .and_then(|v| v.as_string())
                .filter(|s| !s.is_empty())
                .or_else(|| evm_provider_name_from_flags(obj))
                .unwrap_or_else(|| fallback.to_string())
        };
        let icon_url = read_provider_icon(obj);
        found.push((window_key, name, icon_url));
    };

    // ── 1. Well-known top-level window keys ──────────────────────
    for &(key, fallback) in &[
        ("ethereum",        "EVM Wallet"),
        ("braveEthereum",   "Brave Wallet"),
        ("safepalProvider", "SafePal"),
    ] {
        let Ok(obj) = js_sys::Reflect::get(&win, &JsValue::from_str(key)) else { continue };
        if obj.is_undefined() || obj.is_null() { continue; }
        add(&obj, key.to_string(), fallback, &mut found, &mut seen_objs);
    }

    // ── 2. window.ethereum.providers[] (EIP-5749 multi-wallet) ──
    // When ≥2 EVM extensions are installed each pushes itself here.
    // window.ethereum becomes whichever injected last; others are only
    // reachable via this array.
    if let Ok(eth) = js_sys::Reflect::get(&win, &JsValue::from_str("ethereum")) {
        if let Ok(arr_val) = js_sys::Reflect::get(&eth, &JsValue::from_str("providers")) {
            if let Some(arr) = arr_val.dyn_ref::<js_sys::Array>() {
                for i in 0..arr.length() {
                    let item = arr.get(i);
                    if item.is_undefined() || item.is_null() { continue; }
                    let key: &'static str =
                        Box::leak(format!("ethereum.providers[{i}]").into_boxed_str());
                    add(&item, key.to_string(), "EVM Wallet",
                        &mut found, &mut seen_objs);
                }
            }
        }
    }

    // ── 3. MetaMask static entry ──────────────────────────────────
    // Industry standard (OpenSea, Uniswap, etc.): always show MetaMask if
    // window.ethereum exists, regardless of who currently occupies it.
    // MetaMask intercepts eth_requestAccounts on window.ethereum and shows
    // its own popup — even in privacy mode / before first connection — as
    // long as the extension is installed. If it's not installed, the click
    // falls through to the existing provider (SafePal, etc.) which is fine.
    // Skip if we already detected a MetaMask provider via flags/name above.
    let already_has_metamask = found.iter().any(|(_, name, _)| name == "MetaMask");
    if !already_has_metamask {
        let eth_present = js_sys::Reflect::get(&win, &JsValue::from_str("ethereum"))
            .map(|v| !v.is_undefined() && !v.is_null())
            .unwrap_or(false);
        if eth_present {
            // "ethereum#metamask" is a synthetic key; resolve_window_provider
            // maps it back to window.ethereum at connect time.
            found.push((
                "ethereum#metamask".to_string(),
                "MetaMask".to_string(),
                None,
            ));
        }
    }

    found
}

/// Derive a display name from boolean flag properties that wallets set on their
/// EIP-1193 provider.  Only used as a last resort when `provider.name` is absent.
fn evm_provider_name_from_flags(eth: &JsValue) -> Option<String> {
    // Ordered: more specific flags first so a multi-injecting wallet is labelled correctly
    let flags: &[(&str, &str)] = &[
        ("isCoinbaseWallet", "Coinbase Wallet"),
        ("isBraveWallet",    "Brave Wallet"),
        ("isRabby",          "Rabby"),
        ("isOkxWallet",      "OKX Wallet"),
        ("isTrust",          "Trust Wallet"),
        ("isSafePal",        "SafePal"),
        ("isMetaMask",       "MetaMask"),
    ];
    for (flag, label) in flags {
        if js_sys::Reflect::get(eth, &JsValue::from_str(flag))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Some(label.to_string());
        }
    }
    None
}

/// Return `true` if `obj` has a function property named `method`.
fn has_fn(obj: &JsValue, method: &str) -> bool {
    js_sys::Reflect::get(obj, &JsValue::from_str(method))
        .map(|v| v.is_function())
        .unwrap_or(false)
}

/// Resolve a provider object from the window by a `window_key` that may be
/// a simple name (`"ethereum"`, `"braveEthereum"`) or an indexed path of the
/// form `"ethereum.providers[N]"` as produced by `detect_all_evm_providers()`
/// for EIP-5749 multi-wallet arrays.
pub fn resolve_window_provider(window_key: &str) -> Option<JsValue> {
    let win = web_sys::window()?;

    // Synthetic key: MetaMask in privacy mode — resolve to window.ethereum.
    // MetaMask intercepts eth_requestAccounts on window.ethereum even when
    // another wallet (e.g. SafePal) currently occupies that slot.
    if window_key == "ethereum#metamask" {
        let v = js_sys::Reflect::get(&win, &JsValue::from_str("ethereum")).ok()?;
        return if v.is_undefined() || v.is_null() { None } else { Some(v) };
    }

    // Fast path — simple top-level key with no dot or special chars
    if !window_key.contains('.') {
        let v = js_sys::Reflect::get(&win, &JsValue::from_str(window_key)).ok()?;
        return if v.is_undefined() || v.is_null() { None } else { Some(v) };
    }

    // Path like "ethereum.providers[2]"
    // Split on '.' first, then handle bracket suffixes on each segment.
    let mut current: JsValue = win.into();
    for segment in window_key.split('.') {
        // Does this segment have a bracket index? e.g. "providers[2]"
        if let Some(bracket_pos) = segment.find('[') {
            let prop  = &segment[..bracket_pos];
            let index_str = segment[bracket_pos + 1..].trim_end_matches(']');
            let index: u32 = index_str.parse().ok()?;

            // Navigate into the named property first
            if !prop.is_empty() {
                current = js_sys::Reflect::get(&current, &JsValue::from_str(prop)).ok()?;
                if current.is_undefined() || current.is_null() { return None; }
            }
            // Then index into the array
            let arr = current.dyn_ref::<js_sys::Array>()?;
            current = arr.get(index);
        } else {
            current = js_sys::Reflect::get(&current, &JsValue::from_str(segment)).ok()?;
        }
        if current.is_undefined() || current.is_null() { return None; }
    }
    Some(current)
}

// ── L1 connection (BTC) ───────────────────────────────────────────

/// Connect to a specific BTC provider identified by its `window_path`
/// (e.g. `"unisat"`, `"bitcoin"`, `"safepalProvider.bitcoin"`).
///
/// Protocol (tried in order, first success wins):
///   1. `requestAccounts()` → array of address strings
///   2. `getAccounts()`     → same shape, alternative name
/// Then:
///   3. `getPublicKey()`    → compressed pubkey hex (preferred)
///      or derive pubkey from address if absent
/// Network:
///   4. `getChain()` / `getNetwork()` → best-effort, falls back to "unknown"
///
/// We do NOT require `signPSBT` — the bridge signs PSBTs server-side.
pub async fn connect_btc_provider(window_path: &str, display_name: &str) -> Result<WalletInfo, String> {
    let provider = resolve_btc_provider(window_path)
        .ok_or_else(|| format!("BTC provider at '{window_path}' not found"))?;

    // Clear stale payment address from any previous wallet session.
    // Sats-connect wallets (Xverse) re-populate this in try_sats_connect().
    crate::session::set_l1_payment_address("");

    // ── 1. Request accounts ───────────────────────────────────────
    let (address, embedded_pubkey) = request_btc_accounts(&provider, display_name).await?;

    // ── 2. Get public key (best-effort) ───────────────────────────
    // Try the standard getPublicKey() first, fall back to the pubkey
    // returned by sats-connect (wallet_connect response includes publicKey
    // per-address).
    //
    // Optimisation: if the address hasn't changed from the currently stored
    // one AND we already have a valid pubkey in the DOM, skip calling
    // getPublicKey() again.  UniSat shows a separate permission popup for
    // getPublicKey() on first use; calling it redundantly on every
    // accountsChanged reconnect is unnecessary and confusing.
    let cached_pubkey: Option<String> = {
        let stored_pk = crate::session::l1_pubkey();
        if crate::session::l1_address() == address && !stored_pk.is_empty() {
            Some(stored_pk)
        } else {
            None
        }
    };

    let public_key = if let Some(pk) = cached_pubkey {
        web_sys::console::log_1(&JsValue::from_str(
            "[BINST] reusing cached pubkey (address unchanged)"
        ));
        pk
    } else {
        get_btc_pubkey(&provider).await
            .or(embedded_pubkey)
            .unwrap_or_default()
    };

    // ── 3. Network (best-effort) ──────────────────────────────────
    let network = get_btc_network(&provider).await;

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] BTC provider '{window_path}' ({display_name}) connected: addr={address} pubkey={public_key} net={network}"
    )));

    Ok(WalletInfo {
        address,
        public_key,
        network,
        kind: WalletKind::BtcProvider {
            window_key: Box::leak(window_path.to_string().into_boxed_str()),
            name: display_name.to_string(),
        },
    })
}

/// Race a JS Promise against a timeout.  Resolves with `"__timeout__"` if the
/// timeout fires first.  The original promise is NOT cancelled — JS has no
/// cancellation mechanism — but we stop waiting for it.
async fn race_timeout(promise: js_sys::Promise, ms: i32) -> Result<JsValue, JsValue> {
    let timeout = js_sys::Promise::new(&mut |resolve, _| {
        let win = web_sys::window().unwrap();
        let cb = Closure::once_into_js(move || {
            resolve.call1(&JsValue::NULL, &JsValue::from_str("__timeout__")).ok();
        });
        win.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(), ms,
        ).ok();
    });
    let race = js_sys::Promise::race(&js_sys::Array::of2(&promise, &timeout));
    wasm_bindgen_futures::JsFuture::from(race).await
}

/// Request accounts from a BTC provider object.
///
/// Detection strategy (instant — no timeouts):
///
///  1. **Sats-connect** (Xverse, WBIP wallets): provider has `request` + `connect`
///     but *not* `requestAccounts`.  → `request("wallet_connect", null)`.
///  2. **UniSat-like**: provider has `requestAccounts`.
///     → `requestAccounts()`.
///  3. **Phantom BTC**: provider has `connect` but not `request`.
///     → `connect()` → `{ address }`.
///  4. **EIP-1193 BTC**: provider has `request` but not `connect`.
///     → `request({method: "btc_requestAccounts"})`.
///  5. Fallback: try everything with short timeouts.
///
/// Returns `(address, Option<publicKey>)`.  The public key is available when
/// the provider response embeds it (e.g. sats-connect `wallet_connect`).
async fn request_btc_accounts(provider: &JsValue, wallet_name: &str) -> Result<(String, Option<String>), String> {
    let log = |msg: &str| {
        web_sys::console::log_1(&JsValue::from_str(&format!("[BINST-connect] {wallet_name}: {msg}")));
    };

    let has_request_accounts = has_fn(provider, "requestAccounts");
    let has_request = has_fn(provider, "request");
    let has_connect = has_fn(provider, "connect");

    // ── 1. Sats-connect fast path (Xverse) ────────────────────────────
    // Signature: request ✓, connect ✓, requestAccounts ✗
    if has_request && has_connect && !has_request_accounts {
        log("sats-connect wallet detected → request('wallet_connect', null)");
        if let Some((addr, pubkey)) = try_sats_connect(provider, &log).await? {
            return Ok((addr, pubkey));
        }
    }

    // ── 2. UniSat-like: requestAccounts() → [addr, …] ─────────────────
    if has_request_accounts {
        log("trying requestAccounts()");
        if let Some(addr) = try_simple_call(provider, "requestAccounts", &log).await? {
            return Ok((addr, None));
        }
    }

    // ── 3. Phantom BTC: connect() → { address } ───────────────────────
    if has_connect && !has_request {
        log("trying connect()");
        if let Some(addr) = try_connect(provider, &log).await? {
            return Ok((addr, None));
        }
    }

    // ── 4. EIP-1193 BTC: request({method: "btc_requestAccounts"}) ──────
    if has_request && !has_connect {
        log("trying request({method: 'btc_requestAccounts'})");
        if let Some(addr) = try_eip1193_request(provider, &log).await? {
            return Ok((addr, None));
        }
    }

    // ── 5. Fallback: try everything we haven't tried yet ───────────────
    log("primary detection missed — trying remaining patterns");

    if has_request && !(has_connect && !has_request_accounts) {
        // Haven't tried sats-connect yet
        if let Some((addr, pubkey)) = try_sats_connect(provider, &log).await? {
            return Ok((addr, pubkey));
        }
    }
    if !has_request_accounts {
        // Try getAccounts as a fallback
        if has_fn(provider, "getAccounts") {
            log("trying getAccounts()");
            if let Some(addr) = try_simple_call(provider, "getAccounts", &log).await? {
                return Ok((addr, None));
            }
        }
    }
    if has_connect && has_request {
        // Phantom-style connect() not yet tried
        log("trying connect() (fallback, 3s timeout)");
        if let Some(addr) = try_connect_with_timeout(provider, &log).await? {
            return Ok((addr, None));
        }
    }
    if has_request && has_connect {
        // EIP-1193 not yet tried — use timeout since provider also has sats-connect
        log("trying request({method: 'btc_requestAccounts'}) (fallback, 3s timeout)");
        if let Some(addr) = try_eip1193_request_with_timeout(provider, &log).await? {
            return Ok((addr, None));
        }
    }

    Err(format!("{wallet_name}: no supported account-request method found"))
}

// ── Helper: sats-connect request("wallet_connect"|"getAddresses") ──────
/// Returns `Some((address, Option<publicKey>))` on success.
/// If the response includes a separate "payment" address, stores it in the
/// `#wallet-l1-payment-address` hidden input for UTXO funding.
async fn try_sats_connect(
    provider: &JsValue,
    log: &dyn Fn(&str),
) -> Result<Option<(String, Option<String>)>, String> {
    let Some(f) = get_fn(provider, "request") else { return Ok(None) };
    for method in &["wallet_connect", "getAddresses"] {
        log(&format!("  → request('{method}', null)"));
        let Ok(pv) = f.call2(provider, &JsValue::from_str(method), &JsValue::NULL) else { continue };
        let Ok(promise) = pv.dyn_into::<js_sys::Promise>() else { continue };
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(result) => {
                if let Some((addr, pubkey, payment_addr)) = extract_sats_connect_address(&result) {
                    // Keep the payment address for UTXO funding
                    if let Some(pay) = &payment_addr {
                        crate::session::set_l1_payment_address(pay);
                        log(&format!("  stored payment address: {pay}"));
                    }
                    return Ok(Some((addr, pubkey)));
                }
            }
            Err(e) => {
                if is_user_rejection(&e) {
                    return Err("User rejected the connection request".to_string());
                }
            }
        }
    }
    Ok(None)
}

// ── Helper: simple method() → [addr, …] or { address } ────────────────
async fn try_simple_call(
    provider: &JsValue,
    method: &str,
    log: &dyn Fn(&str),
) -> Result<Option<String>, String> {
    let Some(f) = get_fn(provider, method) else { return Ok(None) };
    let Ok(pv) = f.call0(provider) else { return Ok(None) };
    let Ok(promise) = pv.dyn_into::<js_sys::Promise>() else { return Ok(None) };
    match wasm_bindgen_futures::JsFuture::from(promise).await {
        Ok(result) => {
            if let Some(addr) = extract_first_address(&result) {
                return Ok(Some(addr));
            }
            log(&format!("  {method}() returned no usable address"));
            Ok(None)
        }
        Err(e) => {
            if is_user_rejection(&e) {
                return Err("User rejected the connection request".to_string());
            }
            log(&format!("  {method}() failed"));
            Ok(None)
        }
    }
}

// ── Helper: connect() → { address } ───────────────────────────────────
async fn try_connect(
    provider: &JsValue,
    log: &dyn Fn(&str),
) -> Result<Option<String>, String> {
    let Some(f) = get_fn(provider, "connect") else { return Ok(None) };
    let Ok(pv) = f.call0(provider) else { return Ok(None) };
    let Ok(promise) = pv.dyn_into::<js_sys::Promise>() else { return Ok(None) };
    match wasm_bindgen_futures::JsFuture::from(promise).await {
        Ok(result) => {
            if let Some(addr) = js_sys::Reflect::get(&result, &JsValue::from_str("address"))
                .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty()) {
                return Ok(Some(addr));
            }
            if let Some(addr) = extract_first_address(&result) {
                return Ok(Some(addr));
            }
            log("  connect() returned no usable address");
            Ok(None)
        }
        Err(e) => {
            if is_user_rejection(&e) {
                return Err("User rejected the connection request".to_string());
            }
            log("  connect() failed");
            Ok(None)
        }
    }
}

// ── Helper: connect() with 3s timeout (fallback only) ──────────────────
async fn try_connect_with_timeout(
    provider: &JsValue,
    log: &dyn Fn(&str),
) -> Result<Option<String>, String> {
    let Some(f) = get_fn(provider, "connect") else { return Ok(None) };
    let Ok(pv) = f.call0(provider) else { return Ok(None) };
    let Ok(promise) = pv.dyn_into::<js_sys::Promise>() else { return Ok(None) };
    match race_timeout(promise, 3_000).await {
        Ok(result) => {
            if result.as_string().as_deref() == Some("__timeout__") {
                log("  connect() timed out — skipping");
                return Ok(None);
            }
            if let Some(addr) = js_sys::Reflect::get(&result, &JsValue::from_str("address"))
                .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty()) {
                return Ok(Some(addr));
            }
            if let Some(addr) = extract_first_address(&result) {
                return Ok(Some(addr));
            }
            Ok(None)
        }
        Err(e) => {
            if is_user_rejection(&e) {
                return Err("User rejected the connection request".to_string());
            }
            Ok(None)
        }
    }
}

// ── Helper: EIP-1193 request({method: "btc_requestAccounts"}) ──────────
async fn try_eip1193_request(
    provider: &JsValue,
    log: &dyn Fn(&str),
) -> Result<Option<String>, String> {
    let Some(f) = get_fn(provider, "request") else { return Ok(None) };
    let params = js_sys::Object::new();
    js_sys::Reflect::set(&params, &JsValue::from_str("method"),
        &JsValue::from_str("btc_requestAccounts")).ok();
    let Ok(pv) = f.call1(provider, &params) else { return Ok(None) };
    let Ok(promise) = pv.dyn_into::<js_sys::Promise>() else { return Ok(None) };
    match wasm_bindgen_futures::JsFuture::from(promise).await {
        Ok(result) => {
            if let Some(addr) = extract_first_address(&result) {
                return Ok(Some(addr));
            }
            log("  request({method}) returned no usable address");
            Ok(None)
        }
        Err(e) => {
            if is_user_rejection(&e) {
                return Err("User rejected the connection request".to_string());
            }
            log("  request({method}) failed");
            Ok(None)
        }
    }
}

// ── Helper: EIP-1193 request({method}) with 3s timeout ─────────────────
async fn try_eip1193_request_with_timeout(
    provider: &JsValue,
    log: &dyn Fn(&str),
) -> Result<Option<String>, String> {
    let Some(f) = get_fn(provider, "request") else { return Ok(None) };
    let params = js_sys::Object::new();
    js_sys::Reflect::set(&params, &JsValue::from_str("method"),
        &JsValue::from_str("btc_requestAccounts")).ok();
    let Ok(pv) = f.call1(provider, &params) else { return Ok(None) };
    let Ok(promise) = pv.dyn_into::<js_sys::Promise>() else { return Ok(None) };
    match race_timeout(promise, 3_000).await {
        Ok(result) => {
            if result.as_string().as_deref() == Some("__timeout__") {
                log("  request({method}) timed out — skipping");
                return Ok(None);
            }
            if let Some(addr) = extract_first_address(&result) {
                return Ok(Some(addr));
            }
            Ok(None)
        }
        Err(e) => {
            if is_user_rejection(&e) {
                return Err("User rejected the connection request".to_string());
            }
            Ok(None)
        }
    }
}

/// Get a function from a JS object by name, or None.
fn get_fn(obj: &JsValue, name: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(obj, &JsValue::from_str(name))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
}

/// Extract the first payment address (and optional publicKey) from a
/// sats-connect style response.
/// Handles both `{ result: { addresses: [...] } }` (Xverse v2) and
/// `{ addresses: [...] }` (direct) response shapes.
///
/// Each address entry may contain `{ address, publicKey, purpose, addressType }`.
/// We prefer the "ordinals" (P2TR) address for inscriptions, but also capture
/// the "payment" address for UTXO funding.
///
/// Returns `(primary_address, Option<publicKey>, Option<payment_address>)`.
fn extract_sats_connect_address(val: &JsValue) -> Option<(String, Option<String>, Option<String>)> {
    // Try nested: val.result.addresses or val.addresses
    let addresses_val = js_sys::Reflect::get(val, &JsValue::from_str("result"))
        .ok()
        .and_then(|r| {
            if r.is_undefined() || r.is_null() { return None; }
            js_sys::Reflect::get(&r, &JsValue::from_str("addresses")).ok()
        })
        .or_else(|| js_sys::Reflect::get(val, &JsValue::from_str("addresses")).ok());

    let Some(arr_val) = addresses_val else { return None };
    let Some(arr) = arr_val.dyn_ref::<js_sys::Array>() else {
        // Maybe it's a single address object
        let addr = js_sys::Reflect::get(&arr_val, &JsValue::from_str("address"))
            .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty())?;
        let pubkey = js_sys::Reflect::get(&arr_val, &JsValue::from_str("publicKey"))
            .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty());
        return Some((addr, pubkey, None));
    };

    // Scan all entries, collect ordinals + payment separately.
    let mut ordinals: Option<(String, Option<String>)> = None;
    let mut payment_addr: Option<String> = None;
    let mut fallback: Option<(String, Option<String>)> = None;
    for i in 0..arr.length() {
        let item = arr.get(i);
        let addr = js_sys::Reflect::get(&item, &JsValue::from_str("address"))
            .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty());
        let Some(addr) = addr else { continue };
        let pubkey = js_sys::Reflect::get(&item, &JsValue::from_str("publicKey"))
            .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty());
        let purpose = js_sys::Reflect::get(&item, &JsValue::from_str("purpose"))
            .ok().and_then(|v| v.as_string()).unwrap_or_default().to_lowercase();
        match purpose.as_str() {
            "ordinals" => { if ordinals.is_none() { ordinals = Some((addr, pubkey)); } }
            "payment"  => { if payment_addr.is_none() { payment_addr = Some(addr); } }
            _ => { if fallback.is_none() { fallback = Some((addr, pubkey)); } }
        }
    }
    // Primary = ordinals (P2TR), else payment, else fallback.
    // payment_addr is kept separately for UTXO funding.
    let (primary, pubkey) = ordinals.or(fallback)?;
    Some((primary, pubkey, payment_addr))
}

/// Extract the first address string from an array, object, or plain string result.
fn extract_first_address(val: &JsValue) -> Option<String> {
    if let Some(s) = val.as_string() {
        if !s.is_empty() { return Some(s); }
    }
    if let Some(arr) = val.dyn_ref::<js_sys::Array>() {
        for i in 0..arr.length() {
            let item = arr.get(i);
            if let Some(s) = item.as_string().filter(|s| !s.is_empty()) { return Some(s); }
            // Array of objects with .address field
            if let Some(s) = js_sys::Reflect::get(&item, &JsValue::from_str("address"))
                .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty())
            { return Some(s); }
        }
    }
    // Plain object with .address field
    js_sys::Reflect::get(val, &JsValue::from_str("address"))
        .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty())
}

fn is_user_rejection(e: &JsValue) -> bool {
    let msg = format!("{e:?}").to_lowercase();
    msg.contains("reject") || msg.contains("cancel") || msg.contains("denied")
}

/// Get the compressed public key from a BTC provider (best-effort).
async fn get_btc_pubkey(provider: &JsValue) -> Option<String> {
    // Standard: getPublicKey() → hex string
    for method in &["getPublicKey"] {
        let f = js_sys::Reflect::get(provider, &JsValue::from_str(method)).ok()?;
        let f = f.dyn_into::<js_sys::Function>().ok()?;
        let promise = f.call0(provider).ok()?;
        let promise: js_sys::Promise = promise.dyn_into().ok()?;
        if let Ok(val) = wasm_bindgen_futures::JsFuture::from(promise).await {
            if let Some(s) = val.as_string().filter(|s| !s.is_empty()) { return Some(s); }
            // Some wallets return {publicKey: "…"}
            if let Some(s) = js_sys::Reflect::get(&val, &JsValue::from_str("publicKey"))
                .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty())
            { return Some(s); }
        }
    }
    None
}

/// Get the Bitcoin network name from a BTC provider (best-effort).
async fn get_btc_network(provider: &JsValue) -> String {
    // Try getChain() first (UniSat-style), then getNetwork() (legacy)
    for method in &["getChain", "getNetwork"] {
        if let Some(val) = call_async_str(provider, method).await {
            // getChain returns an object {name, network}; getNetwork returns a string
            if let Ok(obj) = js_sys::Reflect::get(&JsValue::from_str(""), &JsValue::from_str("")) {
                let _ = obj; // just to avoid dead branch warnings
            }
            // Try to read .name or .network fields if it's an object
            let name_field = js_sys::Reflect::get(&val, &JsValue::from_str("name"))
                .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty());
            let net_field = js_sys::Reflect::get(&val, &JsValue::from_str("network"))
                .ok().and_then(|v| v.as_string()).filter(|s| !s.is_empty());
            if let Some(n) = name_field.or(net_field).or_else(|| val.as_string()) {
                if !n.is_empty() { return n; }
            }
        }
    }
    "unknown".to_string()
}

/// Call `obj.method()` async and return the raw JsValue result.
async fn call_async_str(obj: &JsValue, method: &str) -> Option<JsValue> {
    let f = js_sys::Reflect::get(obj, &JsValue::from_str(method)).ok()?;
    let f = f.dyn_into::<js_sys::Function>().ok()?;
    let promise = f.call0(obj).ok()?;
    let promise: js_sys::Promise = promise.dyn_into().ok()?;
    wasm_bindgen_futures::JsFuture::from(promise).await.ok()
}

// ── L2 connection (EVM) ───────────────────────────────────────────

// ── L2 signed transaction sender ─────────────────────────────────

/// Send a raw EVM transaction via the connected L2 wallet.
/// `to` — contract address (hex), `data` — ABI-encoded calldata (hex).
/// Returns the transaction hash or an error string.
pub async fn send_evm_tx(to: &str, data: &str, value_wei: &str) -> Result<String, String> {
    let from = crate::session::l2_address();
    if from.is_empty() {
        return Err("No L2 wallet connected".to_string());
    }

    // Use the provider the user actually connected through
    let provider = get_connected_l2_provider()
        .ok_or("No EVM wallet available — connect a wallet first")?;

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] Sending EVM tx via connected L2 provider: to={to} from={from}"
    )));

    let request_fn = js_sys::Reflect::get(&provider, &JsValue::from_str("request"))
        .map_err(|e| format!("no request method: {e:?}"))?;
    let request_fn: js_sys::Function = request_fn.dyn_into()
        .map_err(|_| "request is not a function".to_string())?;

    // Build eth_sendTransaction params
    let tx = js_sys::Object::new();
    js_sys::Reflect::set(&tx, &JsValue::from_str("from"), &JsValue::from_str(&from)).ok();
    js_sys::Reflect::set(&tx, &JsValue::from_str("to"),   &JsValue::from_str(to)).ok();
    js_sys::Reflect::set(&tx, &JsValue::from_str("data"), &JsValue::from_str(data)).ok();
    if !value_wei.is_empty() && value_wei != "0" {
        js_sys::Reflect::set(&tx, &JsValue::from_str("value"), &JsValue::from_str(value_wei)).ok();
    }

    let tx_array = js_sys::Array::new();
    tx_array.push(&tx);

    let params = js_sys::Object::new();
    js_sys::Reflect::set(&params, &JsValue::from_str("method"),
        &JsValue::from_str("eth_sendTransaction")).ok();
    js_sys::Reflect::set(&params, &JsValue::from_str("params"), &tx_array).ok();

    let promise: js_sys::Promise = request_fn.call1(&provider, &params)
        .map_err(|e| format!("eth_sendTransaction call failed: {e:?}"))?
        .dyn_into()
        .map_err(|_| "did not return a promise".to_string())?;

    let result = wasm_bindgen_futures::JsFuture::from(promise).await
        .map_err(|e| {
            let msg = format!("{e:?}");
            if msg.contains("reject") || msg.contains("cancel") || msg.contains("denied") {
                "User rejected the transaction".to_string()
            } else {
                format!("Transaction rejected: {msg}")
            }
        })?;

    result.as_string().ok_or_else(|| "transaction hash is not a string".to_string())
}

// ── UI update callbacks ───────────────────────────────────────────

// ── Wallet bar setup ──────────────────────────────────────────────

// ── Read connected state ──────────────────────────────────────────

/// Synchronously get the L1 public key (x-only hex) from the session store.
pub fn get_connected_pubkey() -> String {
    let pubkey = crate::session::l1_pubkey();
    if pubkey.is_empty() {
        return "<connect wallet>".to_string();
    }
    // Wallets return a compressed 33-byte pubkey ("02…"/"03…"); inscriptions
    // need the 32-byte x-only form.
    normalize_to_xonly(&pubkey)
}

/// Convert a compressed SEC-encoded pubkey hex (66 chars, prefix 02/03) to
/// the 32-byte x-only hex (64 chars) that Taproot/inscription uses.
/// If the key is already 64 chars, return it unchanged.
/// Any other format is returned as-is (caller's validation will catch it).
pub fn normalize_to_xonly(pubkey: &str) -> String {
    let s = pubkey.trim();
    if s.len() == 66 {
        // Compressed: 1 prefix byte (02/03) + 32 x-bytes
        let prefix = &s[..2];
        if prefix == "02" || prefix == "03" {
            return s[2..].to_string(); // drop prefix, return 64-char x-only
        }
    }
    s.to_string()
}

/// Synchronously get the connected L2 EVM address.
pub fn get_connected_l2_address() -> String {
    let addr = crate::session::l2_address();
    if addr.is_empty() { "<connect EVM wallet>".to_string() } else { addr }
}

/// Synchronously get the stored L1 provider window_path (e.g. "BitcoinProvider", "unisat").
pub fn get_connected_l1_provider() -> String {
    crate::session::l1_provider_key()
}

/// Check whether a given provider window_path is a sats-connect provider
/// (has `request` + `connect` but NOT `requestAccounts`).
pub fn is_sats_connect_provider(provider: &JsValue) -> bool {
    has_fn(provider, "request")
        && has_fn(provider, "connect")
        && !has_fn(provider, "requestAccounts")
}

// ── Disconnect ────────────────────────────────────────────────────

// ── signMessage (BTC auth challenge) ─────────────────────────────

/// Ask the connected L1 wallet to sign an arbitrary message string.
/// Returns the base64-encoded signature, or an error.
/// Uses `detect_btc_provider()` to find the first capable BTC provider —
/// supports any window path including nested ones like `safepalProvider.bitcoin`.
pub async fn sign_message_l1(message: &str) -> Result<String, String> {
    let handle = detect_all_btc_providers()
        .into_iter()
        .next()
        .ok_or_else(|| "No Bitcoin wallet detected".to_string())?;

    let provider = resolve_btc_provider(handle.window_path)
        .ok_or_else(|| format!("BTC provider at '{}' not found", handle.window_path))?;

    let sign_fn = js_sys::Reflect::get(&provider, &JsValue::from_str("signMessage"))
        .map_err(|e| format!("no signMessage on provider: {e:?}"))?;
    let sign_fn: js_sys::Function = sign_fn
        .dyn_into()
        .map_err(|_| "signMessage is not a function".to_string())?;

    let promise: js_sys::Promise = sign_fn
        .call1(&provider, &JsValue::from_str(message))
        .map_err(|e| format!("signMessage call failed: {e:?}"))?
        .dyn_into()
        .map_err(|_| "signMessage did not return a promise".to_string())?;

    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| {
            let msg = format!("{e:?}");
            if msg.contains("reject") || msg.contains("cancel") || msg.contains("denied") {
                "User rejected the signing request".to_string()
            } else {
                format!("signMessage rejected: {msg}")
            }
        })?;

    result
        .as_string()
        .ok_or_else(|| "signature is not a string".to_string())
}

/// Fetch all inscription IDs owned by the connected wallet.
///
/// - For UniSat providers: calls `window.unisat.getInscriptions(0, 100)` which
///   returns inscriptions regardless of address type (P2WPKH or P2TR).
/// - For other providers (Xverse etc.): returns the ordinals address stored in
///   `#wallet-l1-address` so the caller can query the Xverse indexer.
///
/// Returns `Ok(ids)` where each id is a full inscription id string (e.g. `"abc…i0"`).
pub async fn get_unisat_inscription_ids(window_path: &str) -> Result<Vec<String>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let provider = resolve_btc_provider(window_path)
        .ok_or_else(|| format!("provider '{window_path}' not found"))?;

    // Only UniSat exposes getInscriptions
    let Some(f) = get_fn(&provider, "getInscriptions") else {
        return Err("getInscriptions not available".to_string());
    };

    // getInscriptions(cursor: number, size: number) → Promise<{total, list: [{inscriptionId, ...}]}>
    let cursor = JsValue::from_f64(0.0);
    let size = JsValue::from_f64(100.0);
    let Ok(pv) = f.call2(&provider, &cursor, &size) else {
        return Err("getInscriptions call failed".to_string());
    };
    let promise = pv.dyn_into::<js_sys::Promise>()
        .map_err(|_| "getInscriptions did not return a Promise")?;

    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("getInscriptions error: {e:?}"))?;

    // result shape: { total: N, list: [{ inscriptionId: "...", ... }] }
    let list = js_sys::Reflect::get(&result, &JsValue::from_str("list"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Array>().ok())
        .ok_or("getInscriptions: no list")?;

    let mut ids = Vec::new();
    for i in 0..list.length() {
        let item = list.get(i);
        if let Some(id) = js_sys::Reflect::get(&item, &JsValue::from_str("inscriptionId"))
            .ok()
            .and_then(|v| v.as_string())
        {
            ids.push(id);
        }
    }
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[BINST] UniSat getInscriptions: {} ids", ids.len()
    )));
    Ok(ids)
}
