//! Connecting wallets and keeping their sessions across reloads.
//!
//! The EVM connect-and-switch flow is adapted from
//! binst-pilot-webapp@dda2d1a:src/wallet_picker.rs (`evm_connect_and_switch`):
//! it returns the session instead of writing to the DOM.

use js_sys::{Array, Function, Object, Promise, Reflect};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::model::Layer;
use crate::state::{BtcSession, EvmSession, Msg, Registry};
use crate::storage::{get_l2_network, get_local_storage, L2Network};
use crate::store::{self, dispatch};
use crate::wallet;

const SESSIONS_KEY: &str = "binst_next:v1:sessions";
const REGISTRY_KEY: &str = "binst_next:v1:registry";

#[derive(Default, Serialize, Deserialize)]
struct SavedSessions {
    btc: Option<BtcSession>,
    evm: Option<EvmSession>,
}

// ── Persistence ──────────────────────────────────────────────────

pub fn save_sessions() {
    let saved = store::with(|s| SavedSessions { btc: s.btc.clone(), evm: s.evm.clone() });
    if let (Some(storage), Ok(json)) = (get_local_storage(), serde_json::to_string(&saved)) {
        storage.set_item(SESSIONS_KEY, &json).ok();
    }
}

pub fn save_registry() {
    let json = store::with(|s| serde_json::to_string(&s.registry));
    if let (Some(storage), Ok(json)) = (get_local_storage(), json) {
        storage.set_item(REGISTRY_KEY, &json).ok();
    }
}

fn read<T: for<'de> Deserialize<'de>>(key: &str) -> Option<T> {
    let raw = get_local_storage()?.get_item(key).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

/// Restore the registry and the wallet sessions saved by an earlier visit.
/// Sessions show at once and are then checked against the wallets without
/// prompting; one that no longer matches is dropped.
pub fn restore() {
    dispatch(Msg::SettingsRestored(crate::settings::load()));
    if let Some(registry) = read::<Registry>(REGISTRY_KEY) {
        dispatch(Msg::RegistryRestored(registry));
    }
    let Some(saved) = read::<SavedSessions>(SESSIONS_KEY) else { return };
    let (btc, evm) = (saved.btc.clone(), saved.evm.clone());
    if btc.is_none() && evm.is_none() {
        return;
    }
    dispatch(Msg::SessionsRestored { btc: saved.btc, evm: saved.evm });
    wasm_bindgen_futures::spawn_local(async move {
        // Extensions inject their providers shortly after the page loads.
        if let Some(window) = web_sys::window() {
            crate::fetch::sleep_ms(&window, 600).await;
        }
        if let Some(btc) = btc {
            if !btc_session_still_valid(&btc).await {
                dispatch(Msg::Disconnect(Layer::Bitcoin));
            }
        }
        if let Some(evm) = evm {
            match evm_provider_for(&evm).await {
                Some(provider) => wallet::set_connected_l2_provider(provider),
                None => dispatch(Msg::Disconnect(Layer::Citrea)),
            }
        }
    });
}

/// A stored Bitcoin session is kept when its provider is still injected and,
/// for wallets that can say so without a prompt, still reports the address.
async fn btc_session_still_valid(session: &BtcSession) -> bool {
    if session.is_handoff() {
        return true; // a watch-only account has no provider to check
    }
    let Some(provider) = wallet::resolve_btc_provider(&session.provider_key) else { return false };
    let Some(get_accounts) = function(&provider, "getAccounts") else { return true };
    let Ok(value) = get_accounts.call0(&provider) else { return true };
    let Ok(promise) = value.dyn_into::<Promise>() else { return true };
    match JsFuture::from(promise).await {
        Ok(accounts) => Array::from(&accounts).iter().any(|a| a.as_string().as_deref() == Some(session.address.as_str())),
        Err(_) => false,
    }
}

/// Find the provider a stored Citrea session used and confirm, without a
/// prompt, that it still exposes the account.
async fn evm_provider_for(session: &EvmSession) -> Option<JsValue> {
    let candidates = evm_candidates();
    let candidate = candidates
        .iter()
        .find(|c| !session.rdns.is_empty() && c.rdns == session.rdns)
        .or_else(|| candidates.iter().find(|c| c.name == session.wallet_name))?;
    let accounts = request(&candidate.provider, "eth_accounts", None).await.ok()?;
    let found = Array::from(&accounts)
        .iter()
        .any(|a| a.as_string().is_some_and(|a| a.eq_ignore_ascii_case(&session.address)));
    found.then(|| candidate.provider.clone())
}

// ── Bitcoin ──────────────────────────────────────────────────────

pub async fn connect_btc(key: String, name: String) {
    match wallet::connect_btc_provider(&key, &name).await {
        Ok(info) => {
            let network = info.network.to_lowercase();
            if network.contains("livenet") || network.contains("mainnet") || network == "bitcoin" {
                dispatch(Msg::ConnectFailed(format!(
                    "{name} is on Bitcoin mainnet. Switch it to testnet4 and connect again."
                )));
                return;
            }
            dispatch(Msg::BtcConnected(BtcSession {
                address: info.address,
                pubkey: info.public_key,
                provider_key: key,
                payment_address: String::new(),
                wallet_name: name,
                ..Default::default()
            }));
        }
        Err(why) => dispatch(Msg::ConnectFailed(friendly(&why))),
    }
}

// ── Citrea (EVM) ─────────────────────────────────────────────────

/// An EVM wallet the picker can offer.
#[derive(Clone)]
pub struct EvmCandidate {
    pub name: String,
    pub rdns: String,
    /// `data:` or `https:` icon supplied by the wallet, if any.
    pub icon: String,
    pub provider: JsValue,
}

/// Installed EVM wallets: EIP-6963 announcements first, legacy `window.*`
/// injection points when nothing announced itself.
pub fn evm_candidates() -> Vec<EvmCandidate> {
    wallet::request_eip6963_providers();
    let announced = wallet::get_eip6963_providers();
    if !announced.is_empty() {
        return announced
            .into_iter()
            .map(|p| EvmCandidate { name: p.name, rdns: p.rdns, icon: p.icon, provider: p.provider })
            .collect();
    }
    wallet::detect_all_evm_providers()
        .into_iter()
        .filter_map(|(window_key, name, icon)| {
            wallet::resolve_window_provider(&window_key).map(|provider| EvmCandidate {
                name,
                rdns: String::new(),
                icon: icon.unwrap_or_default(),
                provider,
            })
        })
        .collect()
}

pub async fn connect_evm(index: usize) {
    let Some(candidate) = evm_candidates().into_iter().nth(index) else {
        dispatch(Msg::ConnectFailed("That wallet is no longer available. Open the picker again.".to_string()));
        return;
    };
    connect_evm_provider(candidate.provider, candidate.name, candidate.rdns).await;
}

async fn connect_evm_provider(provider: JsValue, name: String, rdns: String) {
    match evm_connect_and_switch(&provider).await {
        Ok((address, network)) => {
            wallet::set_connected_l2_provider(provider);
            dispatch(Msg::EvmConnected(EvmSession { address, wallet_name: name, network, rdns }));
        }
        Err(why) => dispatch(Msg::ConnectFailed(friendly(&why))),
    }
}

/// Request accounts, then make sure the wallet is on the target L2 chain:
/// switch to it, or add it when the wallet does not know it.
async fn evm_connect_and_switch(provider: &JsValue) -> Result<(String, String), String> {
    let target = get_l2_network();
    let accounts = request(provider, "eth_requestAccounts", None).await?;
    let address = Array::from(&accounts).get(0).as_string().filter(|a| !a.is_empty()).ok_or("No account returned")?;
    let chain_id = request(provider, "eth_chainId", None).await.ok().and_then(|v| v.as_string()).unwrap_or_default();
    if chain_id.eq_ignore_ascii_case(target.chain_id_hex()) {
        return Ok((address, target.network_label().to_string()));
    }

    let chain = Object::new();
    set(&chain, "chainId", &JsValue::from_str(target.chain_id_hex()));
    if request(provider, "wallet_switchEthereumChain", Some(&Array::of1(&chain))).await.is_ok() {
        return Ok((address, target.network_label().to_string()));
    }

    let (currency_name, symbol, decimals) = target.native_currency();
    let currency = Object::new();
    set(&currency, "name", &JsValue::from_str(currency_name));
    set(&currency, "symbol", &JsValue::from_str(symbol));
    set(&currency, "decimals", &JsValue::from_f64(decimals as f64));
    set(&chain, "chainName", &JsValue::from_str(target.chain_name()));
    set(&chain, "nativeCurrency", &currency);
    set(&chain, "rpcUrls", &Array::of1(&JsValue::from_str(target.rpc_url())));
    if matches!(target, L2Network::CitreaTestnet) {
        set(&chain, "blockExplorerUrls", &Array::of1(&JsValue::from_str("https://explorer.testnet.citrea.xyz")));
    }
    if request(provider, "wallet_addEthereumChain", Some(&Array::of1(&chain))).await.is_ok() {
        return Ok((address, target.network_label().to_string()));
    }
    Ok((address, format!("chain {chain_id}")))
}

// ── WalletConnect ────────────────────────────────────────────────

/// Connect a Citrea wallet through WalletConnect. The page's module script
/// exposes `window.__wc.connect(chainIdHex)`, which shows the QR dialog and
/// resolves with an EIP-1193 provider.
pub async fn connect_walletconnect() {
    let wc = web_sys::window().and_then(|w| Reflect::get(&w, &JsValue::from_str("__wc")).ok());
    let Some(connect) = wc.as_ref().and_then(|wc| function(wc, "connect")) else {
        dispatch(Msg::ConnectFailed("WalletConnect is not configured on this build.".to_string()));
        return;
    };
    let chain = JsValue::from_str(get_l2_network().chain_id_hex());
    let promise = connect.call1(wc.as_ref().unwrap_or(&JsValue::NULL), &chain).ok().and_then(|v| v.dyn_into::<Promise>().ok());
    let Some(promise) = promise else {
        dispatch(Msg::ConnectFailed("WalletConnect could not start.".to_string()));
        return;
    };
    match JsFuture::from(promise).await {
        Ok(provider) if !provider.is_null() && !provider.is_undefined() => {
            connect_evm_provider(provider, "WalletConnect".to_string(), "walletconnect".to_string()).await;
        }
        Ok(_) => dispatch(Msg::ConnectFailed("WalletConnect was cancelled.".to_string())),
        Err(e) => dispatch(Msg::ConnectFailed(friendly(&js_error(&e)))),
    }
}

// ── EIP-1193 helpers ─────────────────────────────────────────────

fn function(object: &JsValue, name: &str) -> Option<Function> {
    Reflect::get(object, &JsValue::from_str(name)).ok()?.dyn_into().ok()
}

fn set(object: &Object, key: &str, value: &JsValue) {
    Reflect::set(object, &JsValue::from_str(key), value).ok();
}

fn js_error(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| Reflect::get(e, &JsValue::from_str("message")).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| "the wallet returned an error".to_string())
}

async fn request(provider: &JsValue, method: &str, params: Option<&JsValue>) -> Result<JsValue, String> {
    let request = function(provider, "request").ok_or("The wallet has no request() method")?;
    let args = Object::new();
    set(&args, "method", &JsValue::from_str(method));
    if let Some(params) = params {
        set(&args, "params", params);
    }
    let promise: Promise = request
        .call1(provider, &args)
        .map_err(|e| js_error(&e))?
        .dyn_into()
        .map_err(|_| format!("{method} did not return a promise"))?;
    JsFuture::from(promise).await.map_err(|e| js_error(&e))
}

/// Wallet errors in plain words.
pub fn friendly(why: &str) -> String {
    let lower = why.to_lowercase();
    if lower.contains("reject") || lower.contains("cancel") || lower.contains("denied") {
        "You cancelled the request in the wallet.".to_string()
    } else {
        why.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejections_read_as_a_cancel() {
        assert_eq!(friendly("User rejected the request."), "You cancelled the request in the wallet.");
        assert_eq!(friendly("user denied transaction"), "You cancelled the request in the wallet.");
        assert_eq!(friendly("No UTXOs found"), "No UTXOs found");
    }
}
