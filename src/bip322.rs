//! BIP-322 "simple" message verification for a Taproot key-path address.
//!
//! This is what a wallet produces with `signMessage(msg, "bip322-simple")`
//! from a `tb1p…` address. It is how an institution's holder vouches for
//! something off-chain (here: the Nostr key that speaks for the institution).
//! The R7 contracts in binst-pilot hash the same way on Citrea.

use bitcoin::base64::{engine::general_purpose::STANDARD, Engine};
use bitcoin::secp256k1::{schnorr::Signature, Message, Secp256k1, XOnlyPublicKey};
use bitcoin::{Address, WitnessProgram, WitnessVersion};
use sha2::{Digest, Sha256};

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn tagged_hash(tag: &str, parts: &[&[u8]]) -> [u8; 32] {
    let tag_hash = sha256(&[tag.as_bytes()]);
    let mut all: Vec<&[u8]> = vec![&tag_hash, &tag_hash];
    all.extend_from_slice(parts);
    sha256(&all)
}

/// `SHA256(tag ‖ tag ‖ message)` with tag `BIP0322-signed-message`.
pub fn message_hash(message: &[u8]) -> [u8; 32] {
    tagged_hash("BIP0322-signed-message", &[message])
}

/// The BIP-341 key-path signature hash of BIP-322's virtual `to_sign`
/// transaction for a P2TR `output_key`.
pub fn sighash(output_key: &[u8; 32], message: &[u8], hash_type: u8) -> [u8; 32] {
    // to_spend: version 0, one input 000…000:0xffffffff with scriptSig
    // OP_0 PUSH32 message_hash, one 0-value output paying OP_1 PUSH32 key.
    let mut to_spend = Vec::with_capacity(128);
    to_spend.extend_from_slice(&[0, 0, 0, 0, 1]);
    to_spend.extend_from_slice(&[0u8; 32]);
    to_spend.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x22, 0x00, 0x20]);
    to_spend.extend_from_slice(&message_hash(message));
    to_spend.extend_from_slice(&[0, 0, 0, 0, 1]);
    to_spend.extend_from_slice(&[0u8; 8]);
    to_spend.extend_from_slice(&[0x22, 0x51, 0x20]);
    to_spend.extend_from_slice(output_key);
    to_spend.extend_from_slice(&[0, 0, 0, 0]);
    let txid = sha256(&[&sha256(&[&to_spend])]);

    // to_sign spends to_spend:0 (sequence 0) into one 0-value OP_RETURN output.
    let sha_prevouts = sha256(&[&txid, &[0, 0, 0, 0]]);
    let sha_amounts = sha256(&[&[0u8; 8]]);
    let sha_scriptpubkeys = sha256(&[&[0x22, 0x51, 0x20], output_key]);
    let sha_sequences = sha256(&[&[0, 0, 0, 0]]);
    let sha_outputs = sha256(&[&[0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x6a]]);
    tagged_hash(
        "TapSighash",
        &[
            &[0x00, hash_type], // sighash epoch, hash type
            &[0u8; 8],          // nVersion 0, nLockTime 0
            &sha_prevouts,
            &sha_amounts,
            &sha_scriptpubkeys,
            &sha_sequences,
            &sha_outputs,
            &[0x00],       // spend type: key path, no annex
            &[0, 0, 0, 0], // input index
        ],
    )
}

/// The x-only output key of a Taproot address on any network.
pub fn taproot_output_key(address: &str) -> Result<[u8; 32], String> {
    let address = address
        .parse::<Address<bitcoin::address::NetworkUnchecked>>()
        .map_err(|_| "not a Bitcoin address".to_string())?
        .assume_checked();
    let program: WitnessProgram = address.witness_program().ok_or("not a SegWit address")?;
    if program.version() != WitnessVersion::V1 {
        return Err("only Taproot (bc1p…/tb1p…) addresses are supported".to_string());
    }
    program.program().as_bytes().try_into().map_err(|_| "not a Taproot address".to_string())
}

/// The 64-byte signature and its hash type from a BIP-322 simple signature:
/// base64 of a witness stack holding one item of 64 bytes (SIGHASH_DEFAULT) or
/// 65 bytes (the last one being the hash type).
fn parse_witness(signature_b64: &str) -> Result<([u8; 64], u8), String> {
    let bytes = STANDARD.decode(signature_b64.trim()).map_err(|_| "the signature is not base64".to_string())?;
    let (item, hash_type) = match bytes.as_slice() {
        [0x01, 0x40, rest @ ..] if rest.len() == 64 => (rest, 0x00),
        [0x01, 0x41, rest @ ..] if rest.len() == 65 => (&rest[..64], rest[64]),
        _ => return Err("not a Taproot key-path signature".to_string()),
    };
    // An explicit SIGHASH_DEFAULT byte is not allowed (BIP-341), and BIP-322
    // only accepts DEFAULT or ALL.
    if bytes[1] == 0x41 && hash_type != 0x01 {
        return Err("unsupported signature hash type".to_string());
    }
    Ok((item.try_into().map_err(|_| "bad signature length".to_string())?, hash_type))
}

/// True when `signature_b64` is a valid BIP-322 simple signature of `message`
/// by the Taproot `address`.
pub fn verify(address: &str, message: &str, signature_b64: &str) -> Result<(), String> {
    let key = taproot_output_key(address)?;
    let (signature, hash_type) = parse_witness(signature_b64)?;
    let digest = sighash(&key, message.as_bytes(), hash_type);
    let secp = Secp256k1::verification_only();
    let public_key = XOnlyPublicKey::from_slice(&key).map_err(|_| "the address holds an invalid key".to_string())?;
    let signature = Signature::from_slice(&signature).map_err(|_| "malformed signature".to_string())?;
    secp.verify_schnorr(&signature, &Message::from_digest(digest), &public_key)
        .map_err(|_| "the signature does not match the address and message".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // BIP-322 test vectors.
    const ADDRESS: &str = "bc1ppv609nr0vr25u07u95waq5lucwfm6tde4nydujnu8npg4q75mr5sxq8lt3";
    const SIGNATURE: &str = "AUHd69PrJQEv+oKTfZ8l+WROBHuy9HKrbFCJu7U1iK2iiEy1vMU5EfMtjc+VSHM7aU0SDbak5IUZRVno2P5mjSafAQ==";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn message_hash_matches_the_specification() {
        assert_eq!(hex(&message_hash(b"")), "c90c269c4f8fcbe6880f72a721ddfbf1914268a794cbb21cfafee13770ae19f1");
        assert_eq!(hex(&message_hash(b"Hello World")), "f0eb03b1a75ac6d9847f55c624a99169b5dccba2a31f5b23bea77ba270de0a7a");
    }

    #[test]
    fn the_specification_taproot_signature_verifies() {
        assert_eq!(verify(ADDRESS, "Hello World", SIGNATURE), Ok(()));
    }

    #[test]
    fn another_message_address_or_signature_is_rejected() {
        assert!(verify(ADDRESS, "Hello World!", SIGNATURE).is_err());
        let other = "bc1pss0zhytly75awhm6x2hhvd5lnzv3vssgrf9axfheq8ldyzn88ges79fler";
        assert!(verify(other, "Hello World", SIGNATURE).is_err());
        let mut tampered = STANDARD.decode(SIGNATURE).unwrap();
        tampered[10] ^= 1;
        assert!(verify(ADDRESS, "Hello World", &STANDARD.encode(tampered)).is_err());
    }

    #[test]
    fn only_taproot_key_path_signatures_are_accepted() {
        let segwit_v0 = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";
        assert!(verify(segwit_v0, "Hello World", SIGNATURE).unwrap_err().contains("Taproot"));
        assert!(verify(ADDRESS, "Hello World", "not base64!").unwrap_err().contains("base64"));
        assert!(verify(ADDRESS, "Hello World", &STANDARD.encode([0x02, 0x40, 0x00])).is_err());
        // 65 bytes ending in an explicit SIGHASH_DEFAULT is invalid.
        let mut explicit_default = STANDARD.decode(SIGNATURE).unwrap();
        *explicit_default.last_mut().unwrap() = 0x00;
        assert!(verify(ADDRESS, "Hello World", &STANDARD.encode(explicit_default)).unwrap_err().contains("hash type"));
    }

    #[test]
    fn testnet_addresses_decode_to_their_output_key() {
        let key = taproot_output_key("tb1p0x9489n5mj2zklqdh0wzazehn23tc2ahdflgsm6ws896pvv35k8sdpd82j").unwrap();
        assert_eq!(key.len(), 32);
        assert!(taproot_output_key("nonsense").is_err());
    }
}
