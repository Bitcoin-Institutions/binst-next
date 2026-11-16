// reused-from: binst-pilot-webapp@dda2d1a:src/auth.rs (2026-11-16) — adapted: none (verbatim)
//! Bitcoin-key authentication for BINST.
//!
//! ## Flow
//!
//! 1. Build a deterministic challenge string from the institution's inscription ID
//!    and the current Unix timestamp (rounded to a 5-minute window for replay
//!    resistance without a server round-trip).
//!
//! 2. Ask the connected BTC wallet to sign it (`wallet::sign_message_l1`).
//!    Both UniSat and SafePal use Bitcoin message signing (BIP-322 simple or
//!    the legacy `OP_RETURN` variant).  Both return a base64 signature.
//!
//! 3. Recover the public key from the signature + message and compare it to
//!    the admin pubkey stored in `Institution.btcPubkey` (32-byte x-only hex).
//!
//! ## Signature format
//!
//! UniSat and SafePal both return a **base64-encoded 65-byte compact signature**
//! (1-byte recovery flag + 32-byte r + 32-byte s), identical to Bitcoin Core's
//! `signmessage` RPC output.  We recover the secp256k1 public key using the
//! pure-Rust `k256` crate (WASM-compatible) and compare it to the stored pubkey.
//!
//! ## What this proves
//!
//! Signing succeeds ↔ the user controls the private key corresponding to the
//! stored `btcPubkey`.  Because `btcPubkey` is set by `Institution.sol.setBtcPubkey()`
//! (which only the admin can call), a successful verification means:
//!   "the connected wallet IS the institution admin".

use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use sha2::{Digest, Sha256};

// ── Challenge ────────────────────────────────────────────────────

/// Number of seconds in a time window.  Signatures are valid within the same
/// window as when they were requested (±5 min).
const WINDOW_SECS: u64 = 300;

/// Build the challenge message the user will be asked to sign.
///
/// Format:  `BINST auth | <inscription_id> | window:<N>`
///
/// The window counter prevents replay attacks: a signature produced in window N
/// is invalid in window N+1.  No server state required.
pub fn build_challenge(inscription_id: &str) -> String {
    let window = current_window();
    format!("BINST auth | {inscription_id} | window:{window}")
}

/// Return the current 5-minute window index (seconds since epoch / 300).
fn current_window() -> u64 {
    #[cfg(not(test))]
    let now_ms = js_sys::Date::now() as u64;
    // In native unit tests js_sys is unavailable; use a fixed epoch offset.
    #[cfg(test)]
    let now_ms: u64 = 1_700_000_000_000; // arbitrary fixed time (ms)
    (now_ms / 1000) / WINDOW_SECS
}

// ── Verification ─────────────────────────────────────────────────

/// Result of an authentication attempt.
#[derive(Debug, PartialEq)]
pub enum AuthResult {
    /// Signature verified — the connected wallet is the institution admin.
    Admin,
    /// The signature is valid Bitcoin message signing but belongs to a different
    /// key.  The user is NOT the admin of this institution.
    NotAdmin,
    /// Signature could not be parsed or verified.
    Invalid(String),
}

/// Verify a base64 Bitcoin message signature against a known x-only pubkey.
///
/// `sig_b64`            — base64-encoded 65-byte compact sig from the wallet.
/// `message`            — the exact challenge string that was signed.
/// `expected_xonly_hex` — 32-byte x-only pubkey (64 hex chars) from `Institution.btcPubkey`.
pub fn verify_btc_auth(
    sig_b64: &str,
    message: &str,
    expected_xonly_hex: &str,
) -> AuthResult {
    // 1. Decode base64 signature (65 bytes: 1 recovery flag + 32r + 32s)
    let sig_bytes = match base64_decode(sig_b64) {
        Ok(b) => b,
        Err(e) => return AuthResult::Invalid(format!("base64 decode failed: {e}")),
    };
    if sig_bytes.len() != 65 {
        return AuthResult::Invalid(format!(
            "expected 65-byte signature, got {}",
            sig_bytes.len()
        ));
    }

    // 2. Extract recovery id (first byte, range 27–30 for mainnet, 31–34 testnet)
    let rec_flag = sig_bytes[0];
    let rec_id_raw: u8 = if rec_flag >= 31 {
        // compressed testnet / Electrum style
        (rec_flag - 31) & 1
    } else if rec_flag >= 27 {
        // compressed mainnet style
        (rec_flag - 27) & 1
    } else {
        return AuthResult::Invalid(format!("unexpected recovery flag: {rec_flag}"));
    };

    let rec_id = match RecoveryId::try_from(rec_id_raw) {
        Ok(id) => id,
        Err(_) => return AuthResult::Invalid(format!("invalid recovery id: {rec_id_raw}")),
    };

    // 3. Parse the 64-byte (r || s) signature
    let sig = match Signature::try_from(&sig_bytes[1..65]) {
        Ok(s) => s,
        Err(e) => return AuthResult::Invalid(format!("sig parse failed: {e}")),
    };

    // 4. Hash the message with Bitcoin's message magic prefix
    //    SHA256d("\x18Bitcoin Signed Message:\n" + varint(len) + message)
    let msg_hash = bitcoin_message_hash(message);

    // 5. Recover the verifying key (public key) from signature + prehash
    let recovered = match VerifyingKey::recover_from_prehash(&msg_hash, &sig, rec_id) {
        Ok(vk) => vk,
        Err(e) => return AuthResult::Invalid(format!("key recovery failed: {e}")),
    };

    // 6. Extract x-only (32-byte x coordinate)
    //    to_encoded_point(true) = compressed (33 bytes): 02/03 || x
    let compressed = recovered.to_encoded_point(true);
    let compressed_bytes = compressed.as_bytes();
    if compressed_bytes.len() != 33 {
        return AuthResult::Invalid("unexpected compressed key length".to_string());
    }
    let xonly_hex = hex_encode(&compressed_bytes[1..]); // bytes 1..33 = x coordinate

    // 7. Compare (case-insensitive)
    if xonly_hex.to_lowercase() == expected_xonly_hex.to_lowercase() {
        AuthResult::Admin
    } else {
        AuthResult::NotAdmin
    }
}

// ── Bitcoin message hashing ───────────────────────────────────────

/// Compute SHA256d of the Bitcoin signed message:
///   `\x18Bitcoin Signed Message:\n` + varint(len(msg)) + msg
fn bitcoin_message_hash(message: &str) -> [u8; 32] {
    let prefix = b"\x18Bitcoin Signed Message:\n";
    let msg_bytes = message.as_bytes();
    let varint = encode_varint(msg_bytes.len() as u64);

    // SHA256(SHA256(prefix || varint || message))
    let first = Sha256::new()
        .chain_update(prefix)
        .chain_update(&varint)
        .chain_update(msg_bytes)
        .finalize();

    Sha256::new().chain_update(first).finalize().into()
}

fn encode_varint(n: u64) -> Vec<u8> {
    if n < 0xfd {
        vec![n as u8]
    } else if n <= 0xffff {
        let mut v = vec![0xfd];
        v.extend_from_slice(&(n as u16).to_le_bytes());
        v
    } else {
        let mut v = vec![0xfe];
        v.extend_from_slice(&(n as u32).to_le_bytes());
        v
    }
}

// ── Base64 decode ─────────────────────────────────────────────────

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use bitcoin::base64::{engine::general_purpose::STANDARD, Engine};
    STANDARD
        .decode(input.trim())
        .map_err(|e| format!("{e}"))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_contains_inscription_id() {
        let c = build_challenge("abc123i0");
        assert!(c.contains("abc123i0"));
        assert!(c.starts_with("BINST auth |"));
        assert!(c.contains("window:"));
    }

    #[test]
    fn varint_single_byte() {
        assert_eq!(encode_varint(0), vec![0x00]);
        assert_eq!(encode_varint(0xfc), vec![0xfc]);
    }

    #[test]
    fn varint_two_bytes() {
        let v = encode_varint(0x100);
        assert_eq!(v[0], 0xfd);
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn hex_encode_roundtrip() {
        let bytes = [0xde, 0xad, 0xbe, 0xef];
        assert_eq!(hex_encode(&bytes), "deadbeef");
    }

    #[test]
    fn verify_garbage_sig_returns_invalid() {
        let result = verify_btc_auth("notbase64!!!", "test message", "aabbcc");
        assert!(matches!(result, AuthResult::Invalid(_)));
    }

    #[test]
    fn verify_short_sig_returns_invalid() {
        use bitcoin::base64::{engine::general_purpose::STANDARD, Engine};
        let short = STANDARD.encode(b"tooshort");
        let result = verify_btc_auth(&short, "msg", &"aa".repeat(32));
        assert!(matches!(result, AuthResult::Invalid(_)));
    }

    #[test]
    fn bitcoin_message_hash_is_deterministic() {
        let h1 = bitcoin_message_hash("hello");
        let h2 = bitcoin_message_hash("hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn bitcoin_message_hash_differs_for_different_messages() {
        let h1 = bitcoin_message_hash("hello");
        let h2 = bitcoin_message_hash("world");
        assert_ne!(h1, h2);
    }
}
