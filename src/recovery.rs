//! One-time reveal keys, kept until the user forgets them.
//!
//! Each inscription locks its commit output to a fresh key that only this
//! browser holds (see `reveal.rs`). If the reveal never confirms, importing the
//! saved `rawtr(...)` descriptor into Bitcoin Core or Sparrow spends that
//! output back. The key controls only the commit output (the reveal fee plus
//! 546 sats), never the user's wallet. It sits in localStorage unencrypted:
//! there is nothing to encrypt it with that this page would not also hold.

use serde::{Deserialize, Serialize};

use crate::storage::get_local_storage;

const KEY: &str = "binst_next:v1:recovery";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Planned commit txid (it does not change when the wallet signs).
    pub commit_txid: String,
    /// Empty until the reveal was broadcast.
    pub reveal_txid: String,
    /// The fully signed reveal (hex), once both sides signed it: broadcast it
    /// again if the first attempt failed after the commit went out.
    #[serde(default)]
    pub reveal_hex: String,
    /// `rawtr(<WIF>)#checksum`.
    pub descriptor: String,
    pub label: String,
    pub created_ms: u64,
}

pub fn list() -> Vec<Record> {
    get_local_storage()
        .and_then(|s| s.get_item(KEY).ok().flatten())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write(records: &[Record]) -> bool {
    match (get_local_storage(), serde_json::to_string(records)) {
        (Some(storage), Ok(json)) => storage.set_item(KEY, &json).is_ok(),
        _ => false,
    }
}

/// Save a record and read it back. False when the browser did not keep it.
#[must_use]
pub fn remember(record: Record) -> bool {
    let mut all = list();
    all.retain(|r| r.commit_txid != record.commit_txid);
    let wanted = record.clone();
    all.push(record);
    write(&all) && list().iter().any(|r| *r == wanted)
}

pub fn signed_reveal(commit_txid: &str, reveal_hex: &str) {
    let mut all = list();
    for r in all.iter_mut().filter(|r| r.commit_txid == commit_txid) {
        r.reveal_hex = reveal_hex.to_string();
    }
    write(&all);
}

pub fn revealed(commit_txid: &str, reveal_txid: &str) {
    let mut all = list();
    for r in all.iter_mut().filter(|r| r.commit_txid == commit_txid) {
        r.reveal_txid = reveal_txid.to_string();
    }
    write(&all);
}

pub fn forget(commit_txid: &str) {
    let mut all = list();
    all.retain(|r| r.commit_txid != commit_txid);
    write(&all);
}
