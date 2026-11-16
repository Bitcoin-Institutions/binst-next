//! Typed domain model: institution → process definitions → instances.

use serde::{Deserialize, Serialize};

/// A failure the user can read and act on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserError {
    /// What was being attempted, e.g. "Loading the institution".
    pub what: String,
    /// The reason, in the indexer's or wallet's own words.
    pub why: String,
}

impl UserError {
    pub fn new(what: &str, why: impl Into<String>) -> Self {
        UserError { what: what.to_string(), why: why.into() }
    }
}

/// State of something fetched from the network.
#[derive(Clone, Debug, PartialEq)]
pub enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(UserError),
}

/// State of one stage inside a tree that is still filling in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Stage {
    Loading,
    #[default]
    Ready,
    Failed(String),
}

/// How we know a node belongs to its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provenance {
    /// The indexer validated the parent: its sat was spent in the reveal.
    Verified,
    /// The body names this institution, but the reveal did not spend its
    /// sat, so nothing on Bitcoin backs the claim.
    Declared,
    /// Broadcast from this browser and not indexed yet.
    Pending,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Institution {
    pub id: String,
    pub name: String,
    pub description: String,
    pub website: String,
    /// x-only admin key named in the inscription body.
    pub admin: String,
    /// Address currently holding the inscription's sat (empty while pending).
    pub holder: String,
    pub pending: bool,
    /// Block that confirmed the inscription; 0 when unknown.
    #[serde(default)]
    pub height: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub action: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub address: String,
    pub creator: String,
    pub current_step: u32,
    pub total_steps: u32,
    pub completed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
    pub provenance: Provenance,
    pub instances: Vec<Instance>,
    #[serde(default)]
    pub instances_stage: Stage,
}

/// Where a tree's data came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Source {
    /// Read from the indexer and Citrea just now.
    Live,
    /// Bundled copy shown because the indexer could not be reached.
    Snapshot { date: String },
    /// Only this browser's registry knows it (not indexed yet).
    Local,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    pub institution: Institution,
    pub definitions: Vec<Definition>,
    #[serde(default)]
    pub definitions_stage: Stage,
    pub source: Source,
    /// Honest caveats shown under the tree.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Tree {
    pub fn definition(&self, id: &str) -> Option<&Definition> {
        self.definitions.iter().find(|d| d.id == id)
    }

    pub fn definition_mut(&mut self, id: &str) -> Option<&mut Definition> {
        self.definitions.iter_mut().find(|d| d.id == id)
    }

    /// The definition that owns an instance address.
    pub fn instance(&self, address: &str) -> Option<(&Definition, &Instance)> {
        self.definitions.iter().find_map(|d| {
            d.instances.iter().find(|i| i.address.eq_ignore_ascii_case(address)).map(|i| (d, i))
        })
    }
}

/// A node the user selected on a tree.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeRef {
    Institution(String),
    Definition { institution: String, id: String },
    Instance { institution: String, address: String },
}

/// Which wallet layer an action needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Bitcoin testnet4 — creating institutions and defining processes.
    Bitcoin,
    /// Citrea testnet — starting instances and executing steps.
    Citrea,
}

/// `abcdef12…7890` — shortened ids and addresses for display.
pub fn short(s: &str) -> String {
    let n = s.chars().count();
    if n <= 16 {
        return s.to_string();
    }
    let head: String = s.chars().take(8).collect();
    let tail: String = s.chars().skip(n - 6).collect();
    format!("{head}…{tail}")
}

/// True for `<64 hex>i<index>`.
pub fn is_inscription_id(s: &str) -> bool {
    let Some((txid, index)) = s.split_once('i') else { return false };
    txid.len() == 64
        && txid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && !index.is_empty()
        && index.len() <= 6
        && index.bytes().all(|b| b.is_ascii_digit())
}

/// True for a `0x` + 40 hex EVM address.
pub fn is_evm_address(s: &str) -> bool {
    s.len() == 42 && s.starts_with("0x") && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";

    #[test]
    fn inscription_ids_are_validated() {
        assert!(is_inscription_id(ID));
        assert!(!is_inscription_id(&ID[..60]));
        assert!(!is_inscription_id(&ID.to_uppercase()));
        assert!(!is_inscription_id("46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226"));
        assert!(!is_inscription_id("<script>i0"));
    }

    #[test]
    fn evm_addresses_are_validated() {
        assert!(is_evm_address("0x6a1d2adbac8682773ed6700d2118c709c8ce5000"));
        assert!(!is_evm_address("6a1d2adbac8682773ed6700d2118c709c8ce5000"));
        assert!(!is_evm_address("0x6a1d"));
    }

    #[test]
    fn short_keeps_head_and_tail() {
        assert_eq!(short(ID), "46f5557a…5226i0");
        assert_eq!(short("tb1pabc"), "tb1pabc");
    }

    #[test]
    fn tree_finds_instance_owner_case_insensitively() {
        let tree = Tree {
            institution: Institution {
                id: ID.into(), name: "Acme".into(), description: String::new(), website: String::new(),
                admin: String::new(), holder: String::new(), pending: false, height: 0,
            },
            definitions: vec![Definition {
                id: "d".into(), name: "KYC".into(), description: String::new(), steps: vec![],
                provenance: Provenance::Verified,
                instances: vec![Instance {
                    address: "0xAbC0000000000000000000000000000000000001".into(), creator: String::new(),
                    current_step: 0, total_steps: 2, completed: false,
                }],
                instances_stage: Stage::Ready,
            }],
            definitions_stage: Stage::Ready,
            source: Source::Live,
            notes: vec![],
        };
        let (def, inst) = tree.instance("0xabc0000000000000000000000000000000000001").unwrap();
        assert_eq!(def.name, "KYC");
        assert_eq!(inst.total_steps, 2);
        assert!(tree.instance("0x0").is_none());
    }
}
