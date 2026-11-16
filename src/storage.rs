// reused-from: binst-pilot-webapp@dda2d1a:src/storage.rs (2026-11-16) — adapted: none (verbatim)
//! localStorage inscription registry.
//!
//! Stores inscriptions we created locally so search always finds them
//! even when the indexer hasn't caught up yet.
//!
//! Each entry includes `reveal_txid` and `confirmed` so the pipeline
//! poller can resume in-flight inscriptions across page reloads.

use wasm_bindgen::prelude::*;

const LS_KEY: &str = "binst_inscriptions";
const LS_FACTORY_KEY: &str = "binst_factory_address";
const LS_L2_NETWORK_KEY: &str = "binst_l2_network";

/// Well-known BINSTProcessFactory address on Citrea testnet (chain 5115).
/// Deployed via `deploy-factory-citrea.ts` on 2026-04-09.
pub const CITREA_FACTORY_ADDRESS: &str = "0x6a1d2adbac8682773ed6700d2118c709c8ce5000";

/// Well-known BINSTProcessFactory address deployed by Hardhat's default
/// deterministic deployment (`npx hardhat ignition deploy` or deploy script).
/// This address is the same for every `npx hardhat node` fresh start because
/// Hardhat uses deterministic deployer nonces.
pub const HARDHAT_FACTORY_ADDRESS: &str = "0x9fe46736679d2d9a65f0992f2272de9f3c7fa6e0";

pub fn get_local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

// ── L2 network configuration ─────────────────────────────────────

/// Known L2 network targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2Network {
    /// Citrea testnet (chain 5115 / 0x13fb) — real testnet tokens
    CitreaTestnet,
    /// Hardhat local (chain 31337 / 0x7a69) — free local dev
    HardhatLocal,
}

impl L2Network {
    pub fn chain_id_hex(&self) -> &'static str {
        match self {
            L2Network::CitreaTestnet => "0x13fb",
            L2Network::HardhatLocal  => "0x7a69",
        }
    }

    pub fn chain_name(&self) -> &'static str {
        match self {
            L2Network::CitreaTestnet => "Citrea Testnet",
            L2Network::HardhatLocal  => "Hardhat Local",
        }
    }

    pub fn rpc_url(&self) -> &'static str {
        match self {
            L2Network::CitreaTestnet => "https://rpc.testnet.citrea.xyz",
            L2Network::HardhatLocal  => "http://127.0.0.1:8545",
        }
    }

    pub fn native_currency(&self) -> (&'static str, &'static str, u8) {
        match self {
            L2Network::CitreaTestnet => ("cBTC", "cBTC", 18),
            L2Network::HardhatLocal  => ("ETH", "ETH", 18),
        }
    }

    pub fn network_label(&self) -> &'static str {
        match self {
            L2Network::CitreaTestnet => "citrea-testnet",
            L2Network::HardhatLocal  => "hardhat-local",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "hardhat-local" => L2Network::HardhatLocal,
            _ => L2Network::CitreaTestnet,
        }
    }
}

/// Read the configured L2 network from localStorage.
/// Defaults to Citrea testnet (factory is deployed there).
pub fn get_l2_network() -> L2Network {
    get_local_storage()
        .and_then(|s| s.get_item(LS_L2_NETWORK_KEY).ok())
        .flatten()
        .map(|s| L2Network::from_str(&s))
        .unwrap_or(L2Network::CitreaTestnet)
}

/// Set the L2 network in localStorage.
pub fn set_l2_network(network: L2Network) {
    if let Some(storage) = get_local_storage() {
        storage.set_item(LS_L2_NETWORK_KEY, network.network_label()).ok();
    }
}

// ── Global factory address (per-network) ─────────────────────────

/// Return the well-known factory address for the current L2 network.
pub fn default_factory_address() -> Option<&'static str> {
    match get_l2_network() {
        L2Network::CitreaTestnet => Some(CITREA_FACTORY_ADDRESS),
        L2Network::HardhatLocal  => Some(HARDHAT_FACTORY_ADDRESS),
    }
}

/// Network-scoped localStorage key for the factory address.
/// E.g. `binst_factory_address_citrea-testnet` or `binst_factory_address_hardhat-local`.
fn factory_key() -> String {
    format!("{}_{}", LS_FACTORY_KEY, get_l2_network().network_label())
}

/// Read the BINSTProcessFactory address for the current L2 network.
///
/// Priority: network-scoped localStorage → well-known default for the network.
/// Also clears the old global `binst_factory_address` key on first call so
/// stale addresses from a different network don't leak through.
pub fn get_factory_address() -> Option<String> {
    // Migrate: if the old global key exists, move its value to the *current*
    // network's scoped key, then delete the global key so it never shadows again.
    if let Some(storage) = get_local_storage() {
        if let Ok(Some(old_val)) = storage.get_item(LS_FACTORY_KEY) {
            if !old_val.is_empty() {
                // Only adopt the old value if there's no scoped key yet
                let scoped = factory_key();
                if storage.get_item(&scoped).ok().flatten().is_none() {
                    storage.set_item(&scoped, &old_val).ok();
                }
                storage.remove_item(LS_FACTORY_KEY).ok();
            }
        }
    }

    // Try explicit per-network override
    let stored = get_local_storage()
        .and_then(|s| s.get_item(&factory_key()).ok())
        .flatten()
        .filter(|s| !s.is_empty());
    if stored.is_some() {
        return stored;
    }
    // Fall back to well-known address for the active network
    default_factory_address().map(|s| s.to_string())
}

/// Store the BINSTProcessFactory address for the current L2 network.
pub fn set_factory_address(address: &str) {
    if let Some(storage) = get_local_storage() {
        storage.set_item(&factory_key(), address).ok();
    }
}

/// Load all locally-saved inscriptions from localStorage.
/// Also runs a one-time migration to normalise the `"type"` field on existing entries:
///   - Entries with `"entity_type": "process_template"` → `"type": "process_template"`
///   - Entries with no type field at all → `"type": "institution"`
pub fn load_local_inscriptions() -> Vec<serde_json::Value> {
    let storage = match get_local_storage() {
        Some(s) => s,
        None => return vec![],
    };
    let raw = match storage.get_item(LS_KEY) {
        Ok(Some(s)) => s,
        _ => return vec![],
    };
    let mut list = match serde_json::from_str::<Vec<serde_json::Value>>(&raw) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    // ── Migration: normalise "type" field ──────────────────────────────────────
    let mut migrated = false;
    for entry in list.iter_mut() {
        if entry.get("type").is_some() { continue; } // already normalised

        let legacy = entry["entity_type"].as_str().unwrap_or("").to_string();
        let canonical = if legacy == "process_template" {
            "process_template"
        } else {
            // No entity_type → old institution entry saved before this field was added
            "institution"
        };
        entry["type"] = serde_json::Value::String(canonical.to_string());
        // Remove the legacy field to keep localStorage clean
        if let Some(obj) = entry.as_object_mut() {
            obj.remove("entity_type");
        }
        migrated = true;
    }
    if migrated {
        if let Ok(json) = serde_json::to_string(&list) {
            storage.set_item(LS_KEY, &json).ok();
        }
    }
    // ── End migration ──────────────────────────────────────────────────────────

    list
}

/// Return entries that have not yet reached 6 confirmations.
/// Each item is guaranteed to have `reveal_txid` and `name` fields.
///
/// Migration: entries saved before the `reveal_txid` field was added
/// can be recovered by stripping the trailing `i0` from `inscription_id`.
pub fn get_pending_inscriptions() -> Vec<serde_json::Value> {
    let storage = get_local_storage();
    let mut list = load_local_inscriptions();
    let mut patched = false;

    for entry in list.iter_mut() {
        if entry["confirmed"].as_bool() == Some(true) { continue; }
        let has_reveal = entry["reveal_txid"].as_str().map(|s| !s.is_empty()).unwrap_or(false);
        if !has_reveal {
            // Recover: inscription_id is "{reveal_txid}i0"
            if let Some(iid) = entry["inscription_id"].as_str() {
                if let Some(txid) = iid.strip_suffix("i0") {
                    if txid.len() == 64 {
                        entry["reveal_txid"] = serde_json::Value::String(txid.to_string());
                        if entry.get("confirmed").is_none() {
                            entry["confirmed"] = serde_json::Value::Bool(false);
                        }
                        patched = true;
                    }
                }
            }
        }
    }

    // Write back migrations
    if patched {
        if let Some(s) = &storage {
            if let Ok(json) = serde_json::to_string(&list) {
                s.set_item(LS_KEY, &json).ok();
            }
        }
    }

    list.into_iter()
        .filter(|v| {
            v["confirmed"].as_bool() != Some(true)
                && v["reveal_txid"].as_str().map(|s| !s.is_empty()).unwrap_or(false)
        })
        .collect()
}

/// Mark an inscription as confirmed (6+ confs) by reveal_txid.
/// Prevents the pipeline from re-polling it after a page reload.
pub fn mark_inscription_confirmed(reveal_txid: &str) {
    let storage = match get_local_storage() {
        Some(s) => s,
        None => return,
    };
    let mut list = load_local_inscriptions();
    for entry in list.iter_mut() {
        if entry["reveal_txid"].as_str() == Some(reveal_txid) {
            entry["confirmed"] = serde_json::Value::Bool(true);
        }
    }
    if let Ok(json) = serde_json::to_string(&list) {
        storage.set_item(LS_KEY, &json).ok();
    }
}

/// Save an inscription to localStorage after successful creation.
/// `reveal_txid` is stored separately so the pipeline can resume polling
/// across page reloads.  `confirmed` starts as `false`.
pub fn save_local_inscription(
    inscription_id: &str,
    reveal_txid: &str,
    name: &str,
    admin: &str,
    desc: &str,
    website: &str,
    network: &str,
) {
    let storage = match get_local_storage() {
        Some(s) => s,
        None => return,
    };
    let short_id = &inscription_id[..8.min(inscription_id.len())];
    let mut list = load_local_inscriptions();
    // Check if already exists
    if let Some(pos) = list
        .iter()
        .position(|v| v["inscription_id"].as_str() == Some(inscription_id))
    {
        // Always patch reveal_txid if it was missing (saved before that field existed)
        let existing_reveal = list[pos]["reveal_txid"].as_str().unwrap_or("").to_string();
        if existing_reveal.is_empty() && !reveal_txid.is_empty() {
            list[pos]["reveal_txid"] = serde_json::Value::String(reveal_txid.to_string());
            if list[pos].get("confirmed").is_none() {
                list[pos]["confirmed"] = serde_json::Value::Bool(false);
            }
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[BINST] save_local_inscription: patched reveal_txid for id={short_id}…"
            )));
            if let Ok(json) = serde_json::to_string(&list) {
                storage.set_item(LS_KEY, &json).ok();
            }
            return;
        }

        // Update only when the incoming data is more complete than what's stored.
        // Never overwrite a non-empty name/admin with an empty string (e.g. from a
        // CDN fetch that returned an error body with no BINST fields).
        let existing_name  = list[pos]["name"].as_str().unwrap_or("");
        let existing_admin = list[pos]["admin"].as_str().unwrap_or("");
        let existing_desc  = list[pos]["description"].as_str().unwrap_or("");
        let existing_website = list[pos]["website"].as_str().unwrap_or("");

        // Only proceed if the new data adds something (non-empty name or website)
        // and improves on what's already stored.
        let has_new_data = !name.is_empty() || !website.is_empty();
        let existing_stale = existing_website.is_empty()
            || (existing_desc != desc && !desc.is_empty())
            || (existing_website != website && !website.is_empty())
            || (existing_name.is_empty() && !name.is_empty());
        if has_new_data && existing_stale {
            list[pos] = serde_json::json!({
                "type": "institution",
                "inscription_id": inscription_id,
                "reveal_txid": if reveal_txid.is_empty() { list[pos]["reveal_txid"].as_str().unwrap_or("") } else { reveal_txid },
                "confirmed": list[pos]["confirmed"].as_bool().unwrap_or(false),
                "name":    if name.is_empty()    { existing_name    } else { name    },
                "admin":   if admin.is_empty()   { existing_admin   } else { admin   },
                "description": if desc.is_empty() { existing_desc   } else { desc   },
                "website": if website.is_empty() { existing_website } else { website },
                "network": network,
                "source": "local",
            });
            if let Ok(json) = serde_json::to_string(&list) {
                storage.set_item(LS_KEY, &json).ok();
            }
        }
        return;
    }
    list.push(serde_json::json!({
        "type": "institution",
        "inscription_id": inscription_id,
        "reveal_txid": reveal_txid,
        "confirmed": false,
        "name": name,
        "admin": admin,
        "description": desc,
        "website": website,
        "network": network,
        "source": "local",
    }));
    if let Ok(json) = serde_json::to_string(&list) {
        storage.set_item(LS_KEY, &json).ok();
    }
}

/// Search localStorage inscriptions by query (name, admin, description, or inscription id).
/// Returns all BINST entries that match the query — institutions and process templates alike.
/// Type filtering for display purposes is handled at the call site (e.g. institution view).
pub fn search_local_inscriptions(query: &str) -> Vec<serde_json::Value> {
    let q = query.to_lowercase();
    load_local_inscriptions()
        .into_iter()
        .filter(|v| entry_matches_query(v, &q))
        .collect()
}

/// Returns `true` for institution-level entries.
///
/// Checks `"type"` first (canonical on-chain field from `#[serde(tag = "type")]`),
/// then falls back to `"entity_type"` for entries saved before the field was normalised.
/// An entry with neither field is treated as an institution (legacy pre-type entries).
#[inline]
pub(crate) fn is_institution_entry(v: &serde_json::Value) -> bool {
    // Canonical field: "type" — written by binst-inscription serialisation and new saves
    if let Some(t) = v["type"].as_str() {
        return t == "institution";
    }
    // Legacy fallback: "entity_type" — written by old save_local_template / save_local_inscription
    let entity_type = v["entity_type"].as_str().unwrap_or("");
    entity_type.is_empty() || entity_type == "institution"
}

/// Returns `true` if the entry's name/admin/description/id contains the query string.
#[inline]
fn entry_matches_query(v: &serde_json::Value, q: &str) -> bool {
    let name = v["name"].as_str().unwrap_or("").to_lowercase();
    let admin = v["admin"].as_str().unwrap_or("").to_lowercase();
    let desc = v["description"].as_str().unwrap_or("").to_lowercase();
    let id = v["inscription_id"].as_str().unwrap_or("").to_lowercase();
    name.contains(q) || admin.contains(q) || desc.contains(q) || id.contains(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn institution(name: &str) -> serde_json::Value {
        serde_json::json!({ "name": name, "description": "", "admin": "", "inscription_id": "aaai0" })
    }

    fn institution_explicit(name: &str) -> serde_json::Value {
        serde_json::json!({ "type": "institution", "name": name, "description": "", "admin": "", "inscription_id": "aaai0" })
    }

    fn process_template(name: &str) -> serde_json::Value {
        serde_json::json!({ "type": "process_template", "name": name, "institution_id": "aaai0" })
    }

    fn process_template_legacy(name: &str) -> serde_json::Value {
        // Old entries written by save_local_template before this fix
        serde_json::json!({ "entity_type": "process_template", "name": name, "institution_id": "aaai0" })
    }

    #[test]
    fn is_institution_entry_absent_type() {
        // Old entries saved by save_local_inscription before "type" was added — treated as institution
        assert!(is_institution_entry(&institution("Acme")));
    }

    #[test]
    fn is_institution_entry_explicit_institution() {
        assert!(is_institution_entry(&institution_explicit("Acme")));
    }

    #[test]
    fn is_institution_entry_rejects_process_template() {
        assert!(!is_institution_entry(&process_template("Onboarding")));
    }

    #[test]
    fn is_institution_entry_rejects_legacy_process_template() {
        // "entity_type": "process_template" (old saves) must also be filtered out
        assert!(!is_institution_entry(&process_template_legacy("Onboarding")));
    }

    #[test]
    fn is_institution_entry_rejects_unknown_type() {
        let v = serde_json::json!({ "type": "process_instance", "name": "x" });
        assert!(!is_institution_entry(&v));
    }

    #[test]
    fn is_institution_entry_rejects_unknown_entity_type() {
        let v = serde_json::json!({ "entity_type": "process_instance", "name": "x" });
        assert!(!is_institution_entry(&v));
    }
}

/// Return all locally-saved process_template inscriptions whose `institution_id`
/// matches the given institution inscription ID.
/// Checks `"type"` first, then falls back to `"entity_type"` for legacy entries.
pub fn get_local_templates_for_institution(institution_id: &str) -> Vec<serde_json::Value> {
    load_local_inscriptions()
        .into_iter()
        .filter(|v| {
            let is_template = v["type"].as_str() == Some("process_template")
                || v["entity_type"].as_str() == Some("process_template");
            is_template && v["institution_id"].as_str() == Some(institution_id)
        })
        .collect()
}

/// Save a process_template inscription with its `institution_id` linkage.
/// Wraps `save_local_inscription` and then patches the extra fields.
///
/// `provenance` should be `"verified"` when the template was discovered via the
/// indexer's validated `parents` (on-chain parent-sat spend confirmed), or
/// `"declared"` when it was found only via the JSON `institution_id` field.
pub fn save_local_template(
    inscription_id: &str,
    reveal_txid: &str,
    name: &str,
    desc: &str,
    institution_id: &str,
    steps_json: &str, // JSON array string e.g. `[{"name":"Step 1"}]`
    provenance: &str, // "verified" | "declared"
    network: &str,
) {
    save_local_inscription(inscription_id, reveal_txid, name, "", desc, "", network);

    // Patch entity_type, institution_id, steps, and provenance onto the saved entry
    let storage = match get_local_storage() {
        Some(s) => s,
        None => return,
    };
    let mut list = load_local_inscriptions();
    if let Some(entry) = list.iter_mut().find(|v| v["inscription_id"].as_str() == Some(inscription_id)) {
        // Use canonical "type" field (matches #[serde(tag = "type")] in binst-inscription)
        entry["type"] = serde_json::Value::String("process_template".to_string());
        // Remove legacy field if present
        if let Some(obj) = entry.as_object_mut() {
            obj.remove("entity_type");
        }
        entry["institution_id"] = serde_json::Value::String(institution_id.to_string());
        // Provenance hierarchy: verified > linked > declared — only upgrade, never downgrade
        let existing = entry["provenance"].as_str().unwrap_or("declared");
        let effective = match (existing, provenance) {
            ("verified", _) => "verified",
            (_, "verified") => "verified",
            ("linked", _)   => "linked",
            (_, "linked")   => "linked",
            _               => "declared",
        };
        entry["provenance"] = serde_json::Value::String(effective.to_string());
        if !steps_json.is_empty() {
            if let Ok(steps) = serde_json::from_str::<serde_json::Value>(steps_json) {
                entry["steps"] = steps;
            }
        }
    }
    if let Ok(json) = serde_json::to_string(&list) {
        storage.set_item(LS_KEY, &json).ok();
    }
}

/// Look up the `citrea_contract` address stored in a process template's
/// localStorage entry. Returns `None` if the field is absent or empty.
pub fn get_template_citrea_contract(inscription_id: &str) -> Option<String> {
    load_local_inscriptions()
        .into_iter()
        .find(|v| v["inscription_id"].as_str() == Some(inscription_id))
        .and_then(|v| v["citrea_contract"].as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
}

/// Write a `citrea_contract` address into a process template's localStorage entry.
pub fn set_template_citrea_contract(inscription_id: &str, contract_address: &str) {
    let storage = match get_local_storage() {
        Some(s) => s,
        None => return,
    };
    let mut list = load_local_inscriptions();
    if let Some(entry) = list.iter_mut().find(|v| v["inscription_id"].as_str() == Some(inscription_id)) {
        entry["citrea_contract"] = serde_json::Value::String(contract_address.to_string());
    }
    if let Ok(json) = serde_json::to_string(&list) {
        storage.set_item(LS_KEY, &json).ok();
    }
}

/// Look up a process template entry by inscription ID.
pub fn get_local_template(inscription_id: &str) -> Option<serde_json::Value> {
    load_local_inscriptions()
        .into_iter()
        .find(|v| {
            let is_template = v["type"].as_str() == Some("process_template")
                || v["entity_type"].as_str() == Some("process_template");
            is_template && v["inscription_id"].as_str() == Some(inscription_id)
        })
}

// ── L2 instance finality tracking ─────────────────────────────────
//
// Stores tx hashes for instance creation and step executions so the
// finality tracker can look up L2 block heights and classify them.
// Key: `binst_l2_instance_{instance_address_lowercase}` → JSON object:
//   { "creation_tx": "0x…", "step_txs": { "0": "0x…", "1": "0x…" } }

fn l2_instance_key(instance_address: &str) -> String {
    format!("binst_l2_instance_{}", instance_address.to_lowercase())
}

/// Save the creation tx hash for an L2 instance.
pub fn save_instance_creation_tx(instance_address: &str, tx_hash: &str) {
    let storage = match get_local_storage() {
        Some(s) => s,
        None => return,
    };
    let key = l2_instance_key(instance_address);
    let mut obj = storage
        .get_item(&key)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    obj["creation_tx"] = serde_json::Value::String(tx_hash.to_string());
    if let Ok(json) = serde_json::to_string(&obj) {
        storage.set_item(&key, &json).ok();
    }
}

/// Save a step execution tx hash for an L2 instance.
pub fn save_instance_step_tx(instance_address: &str, step_index: u32, tx_hash: &str) {
    let storage = match get_local_storage() {
        Some(s) => s,
        None => return,
    };
    let key = l2_instance_key(instance_address);
    let mut obj = storage
        .get_item(&key)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !obj.get("step_txs").map_or(false, |v| v.is_object()) {
        obj["step_txs"] = serde_json::json!({});
    }
    obj["step_txs"][step_index.to_string()] = serde_json::Value::String(tx_hash.to_string());
    if let Ok(json) = serde_json::to_string(&obj) {
        storage.set_item(&key, &json).ok();
    }
}

/// Load the creation tx hash for an L2 instance.
pub fn get_instance_creation_tx(instance_address: &str) -> Option<String> {
    let storage = get_local_storage()?;
    let key = l2_instance_key(instance_address);
    let json_str = storage.get_item(&key).ok()??;
    let obj: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    obj["creation_tx"].as_str().map(String::from)
}

/// Load all step execution tx hashes for an L2 instance.
/// Returns a Vec of (step_index, tx_hash) sorted by step index.
pub fn get_instance_step_txs(instance_address: &str) -> Vec<(u32, String)> {
    let storage = match get_local_storage() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let key = l2_instance_key(instance_address);
    let json_str = match storage.get_item(&key).ok().flatten() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let obj: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let step_txs = match obj.get("step_txs") {
        Some(v) if v.is_object() => v,
        _ => return Vec::new(),
    };
    let mut result: Vec<(u32, String)> = step_txs
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(k, v)| {
            let idx = k.parse::<u32>().ok()?;
            let hash = v.as_str()?.to_string();
            Some((idx, hash))
        })
        .collect();
    result.sort_by_key(|(idx, _)| *idx);
    result
}
