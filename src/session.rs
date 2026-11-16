//! Wallet session accessors for the copied modules.
//!
//! `wallet.rs`, `inscribe.rs` and `citrea.rs` used to read hidden DOM inputs.
//! They now call these functions, which read the store.

use crate::state::Msg;
use crate::store;

/// Address of the connected Bitcoin wallet, or empty.
pub fn l1_address() -> String {
    store::with(|s| s.btc.as_ref().map(|b| b.address.clone()).unwrap_or_default())
}

/// Public key of the connected Bitcoin wallet as the wallet returned it, or empty.
pub fn l1_pubkey() -> String {
    store::with(|s| s.btc.as_ref().map(|b| b.pubkey.clone()).unwrap_or_default())
}

/// `window` path of the connected Bitcoin provider (e.g. "unisat"), or empty.
pub fn l1_provider_key() -> String {
    store::with(|s| s.btc.as_ref().map(|b| b.provider_key.clone()).unwrap_or_default())
}

/// Address of the connected Citrea wallet, or empty.
pub fn l2_address() -> String {
    store::with(|s| s.evm.as_ref().map(|e| e.address.clone()).unwrap_or_default())
}

/// Record the wallet's separate funding address (Xverse reports one while connecting).
pub fn set_l1_payment_address(address: &str) {
    store::dispatch(Msg::SetPaymentAddress(address.to_string()));
}
