//! Where the page reads chain data from: the defaults, or the user's own
//! servers (their ord, their esplora/mempool, their Citrea node).
//!
//! An empty field means the default. Saved in localStorage per browser.

use serde::{Deserialize, Serialize};

use crate::storage::{get_local_storage, L2Network};
use crate::store;

const KEY: &str = "binst_next:v1:settings";

/// Public mempool.space API for testnet4.
pub const DEFAULT_ESPLORA: &str = "https://mempool.space/testnet4/api";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The user's ord server (e.g. `http://127.0.0.1:8080`). It must run with
    /// `--index-addresses`. Empty: the UniSat indexer through `/api/indexer`.
    #[serde(default)]
    pub ord_url: String,
    /// Esplora-compatible API (a self-hosted mempool or esplora) for UTXOs,
    /// transactions, fees and broadcast. Empty: mempool.space.
    #[serde(default)]
    pub esplora_url: String,
    /// Citrea testnet JSON-RPC for reads. Empty: rpc.testnet.citrea.xyz.
    #[serde(default)]
    pub citrea_rpc: String,
}

impl Settings {
    /// Trim, drop trailing slashes, and refuse anything but http(s) URLs.
    pub fn normalized(self) -> Result<Settings, String> {
        fn url(field: &str, value: String) -> Result<String, String> {
            let v = value.trim().trim_end_matches('/').to_string();
            if v.is_empty() || v.starts_with("http://") || v.starts_with("https://") {
                Ok(v)
            } else {
                Err(format!("{field} must start with http:// or https://"))
            }
        }
        Ok(Settings {
            ord_url: url("ord server", self.ord_url)?,
            esplora_url: url("Chain API", self.esplora_url)?,
            citrea_rpc: url("Citrea RPC", self.citrea_rpc)?,
        })
    }
}

pub fn load() -> Settings {
    get_local_storage()
        .and_then(|s| s.get_item(KEY).ok().flatten())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save() {
    let json = store::with(|s| serde_json::to_string(&s.settings));
    if let (Some(storage), Ok(json)) = (get_local_storage(), json) {
        storage.set_item(KEY, &json).ok();
    }
}

fn current() -> Settings {
    store::with(|s| s.settings.clone())
}

/// Esplora-style API base, without a trailing slash.
pub fn esplora_base() -> String {
    let s = current();
    if s.esplora_url.is_empty() { DEFAULT_ESPLORA.to_string() } else { s.esplora_url }
}

/// The user's ord server, when set.
pub fn ord_base() -> Option<String> {
    let s = current();
    (!s.ord_url.is_empty()).then_some(s.ord_url)
}

/// JSON-RPC URL for Citrea reads.
pub fn citrea_rpc(net: L2Network) -> String {
    let s = current();
    match net {
        L2Network::CitreaTestnet if !s.citrea_rpc.is_empty() => s.citrea_rpc,
        _ => net.rpc_url().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_trims_and_checks_schemes() {
        let s = Settings {
            ord_url: " http://127.0.0.1:8080/ ".into(),
            esplora_url: String::new(),
            citrea_rpc: "https://my.node/rpc//".into(),
        }
        .normalized()
        .unwrap();
        assert_eq!(s.ord_url, "http://127.0.0.1:8080");
        assert_eq!(s.esplora_url, "");
        assert_eq!(s.citrea_rpc, "https://my.node/rpc");
        let bad = Settings { ord_url: "javascript:alert(1)".into(), ..Default::default() }.normalized();
        assert!(bad.is_err());
    }
}
