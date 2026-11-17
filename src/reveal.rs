//! PSBTs any Taproot signer can sign, and the reveal signed in the browser.
//!
//! The commit output is locked to a one-time reveal key (leaf key and internal
//! key, as `ord wallet inscribe` does). The browser signs the reveal's
//! script-path input with that key, so the user's wallet only ever signs
//! key-path inputs: the commit's funding inputs and, for a child, the parent
//! inscription's UTXO. Bitcoin Core, Sparrow, Electrum and hardware signers can
//! all do that; none of them can sign an inscription envelope leaf.
//!
//! Everything here is pure (no browser), so it runs in `cargo test` and in the
//! regtest check.

use bitcoin::bip32::{DerivationPath, Fingerprint};
use bitcoin::key::{Keypair, TapTweak, XOnlyPublicKey};
use bitcoin::psbt::Psbt;
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
use bitcoin::taproot::{LeafVersion, TapLeafHash};
use bitcoin::{Network, PrivateKey, ScriptBuf, Transaction, TxOut, Witness};

use crate::txbuilder::InscriptionPlan;

/// The user's own key-path P2TR address, as far as the page knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct WalletKey {
    /// Untweaked x-only key of the address (BIP-86 internal key).
    pub internal: XOnlyPublicKey,
    /// Where the key comes from, when the user imported an account descriptor.
    /// Hardware and descriptor signers need it to find their key.
    pub origin: Option<(Fingerprint, DerivationPath)>,
    pub script_pubkey: ScriptBuf,
}

impl WalletKey {
    fn owns(&self, script: &ScriptBuf) -> bool {
        *script == self.script_pubkey
    }

    fn describe_input(&self, input: &mut bitcoin::psbt::Input) {
        input.tap_internal_key = Some(self.internal);
        if let Some((fp, path)) = &self.origin {
            input.tap_key_origins.insert(self.internal, (vec![], (*fp, path.clone())));
        }
    }

    fn describe_output(&self, output: &mut bitcoin::psbt::Output) {
        output.tap_internal_key = Some(self.internal);
        if let Some((fp, path)) = &self.origin {
            output.tap_key_origins.insert(self.internal, (vec![], (*fp, path.clone())));
        }
    }
}

/// Mark outputs that pay back to the user's address, so signers show them as
/// change instead of as a payment to a stranger.
fn describe_outputs(psbt: &mut Psbt, wallet: &WalletKey) {
    for (i, out) in psbt.unsigned_tx.output.clone().iter().enumerate() {
        if wallet.owns(&out.script_pubkey) {
            wallet.describe_output(&mut psbt.outputs[i]);
        }
    }
}

/// Unsigned commit PSBT: every input is a key-path spend from the user's address.
pub fn commit_psbt(plan: &InscriptionPlan, wallet: &WalletKey) -> Result<Psbt, String> {
    let mut psbt = Psbt::from_unsigned_tx(plan.commit_tx.clone())
        .map_err(|e| format!("Commit PSBT: {e}"))?;
    for (i, utxo) in plan.commit_utxos.iter().enumerate() {
        let input = &mut psbt.inputs[i];
        input.witness_utxo = Some(TxOut { value: utxo.amount, script_pubkey: utxo.script_pubkey.clone() });
        if wallet.owns(&utxo.script_pubkey) {
            wallet.describe_input(input);
        }
    }
    describe_outputs(&mut psbt, wallet);
    Ok(psbt)
}

/// Unsigned reveal PSBT. The commit input carries its leaf, control block,
/// internal key and merkle root (BIP-371); the parent input, when present, is
/// described as one of the user's key-path inputs.
pub fn reveal_psbt(plan: &InscriptionPlan, wallet: &WalletKey) -> Result<Psbt, String> {
    let mut psbt = Psbt::from_unsigned_tx(plan.reveal_tx.clone())
        .map_err(|e| format!("Reveal PSBT: {e}"))?;
    let commit_idx = plan.commit_input_index();
    let leaf = (plan.inscription_script.clone(), LeafVersion::TapScript);
    let control_block = plan
        .taproot_spend_info
        .control_block(&leaf)
        .ok_or("Cannot compute the reveal's control block")?;
    let commit_input = &mut psbt.inputs[commit_idx];
    commit_input.witness_utxo = Some(plan.commit_tx.output[0].clone());
    commit_input.tap_scripts.insert(control_block, leaf);
    commit_input.tap_internal_key = Some(plan.taproot_spend_info.internal_key());
    commit_input.tap_merkle_root = plan.taproot_spend_info.merkle_root();

    if let Some(parent) = &plan.parent_utxo {
        let idx = plan
            .reveal_tx
            .input
            .iter()
            .position(|i| i.previous_output == parent.outpoint)
            .ok_or("The reveal does not spend the parent UTXO")?;
        let input = &mut psbt.inputs[idx];
        input.witness_utxo = Some(TxOut { value: parent.amount, script_pubkey: parent.script_pubkey.clone() });
        if wallet.owns(&parent.script_pubkey) {
            wallet.describe_input(input);
        }
    }
    describe_outputs(&mut psbt, wallet);
    Ok(psbt)
}

/// Sign the reveal's script-path input with the one-time reveal key and
/// finalize that input. SIGHASH_DEFAULT commits to every input's prevout,
/// amount and script and to all outputs, not to other inputs' witnesses, so the
/// wallet can sign the parent input before or after this.
pub fn sign_reveal_input(psbt: &mut Psbt, plan: &InscriptionPlan, reveal_secret: &SecretKey) -> Result<(), String> {
    let secp = Secp256k1::new();
    let keypair = Keypair::from_secret_key(&secp, reveal_secret);
    let (xonly, _) = keypair.x_only_public_key();
    if xonly != plan.taproot_spend_info.internal_key() {
        return Err("The reveal key does not match this inscription".into());
    }
    let prevouts: Vec<TxOut> = psbt
        .inputs
        .iter()
        .map(|i| i.witness_utxo.clone().ok_or("A reveal input has no witness_utxo"))
        .collect::<Result<_, _>>()?;
    let commit_idx = plan.commit_input_index();
    let leaf_hash = TapLeafHash::from_script(&plan.inscription_script, LeafVersion::TapScript);
    let sighash = SighashCache::new(&psbt.unsigned_tx)
        .taproot_script_spend_signature_hash(commit_idx, &Prevouts::All(&prevouts), leaf_hash, TapSighashType::Default)
        .map_err(|e| format!("Reveal sighash: {e}"))?;
    let msg = Message::from_digest(bitcoin::hashes::Hash::to_byte_array(sighash));
    let sig = secp.sign_schnorr_no_aux_rand(&msg, &keypair);
    let control_block = plan
        .taproot_spend_info
        .control_block(&(plan.inscription_script.clone(), LeafVersion::TapScript))
        .ok_or("Cannot compute the reveal's control block")?;
    let input = &mut psbt.inputs[commit_idx];
    input.final_script_witness = Some(Witness::from_slice(&[
        sig.as_ref().to_vec(),
        plan.inscription_script.to_bytes(),
        control_block.serialize(),
    ]));
    // A finalized input keeps only its UTXO (BIP-174 finalizer role).
    input.tap_scripts.clear();
    input.tap_internal_key = None;
    input.tap_merkle_root = None;
    input.tap_key_origins.clear();
    Ok(())
}

/// `rawtr(<WIF>)#checksum` for the commit output's tweaked key. Importing it
/// into Bitcoin Core or Sparrow lets the user spend the commit output by key
/// path if the reveal never confirms.
pub fn recovery_descriptor(plan: &InscriptionPlan, reveal_secret: &SecretKey) -> String {
    let secp = Secp256k1::new();
    let tweaked = Keypair::from_secret_key(&secp, reveal_secret).tap_tweak(&secp, plan.taproot_spend_info.merkle_root());
    let wif = PrivateKey::new(tweaked.to_keypair().secret_key(), Network::Testnet4).to_wif();
    with_checksum(&format!("rawtr({wif})"))
}

/// Append the BIP-380 descriptor checksum.
pub fn with_checksum(desc: &str) -> String {
    format!("{desc}#{}", descriptor_checksum(desc).unwrap_or_default())
}

/// BIP-380 descriptor checksum.
pub fn descriptor_checksum(desc: &str) -> Option<String> {
    const INPUT: &str = "0123456789()[],'/*abcdefgh@:$%{}IJKLMNOPQRSTUVWXYZ&+-.;<=>?!^_|~ijklmnopqrstuvwxyzABCDEFGH`#\"\\ ";
    const CHECKSUM: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    fn polymod(c: u64, val: u64) -> u64 {
        let c0 = c >> 35;
        let mut c = ((c & 0x7_ffff_ffff) << 5) ^ val;
        for (bit, k) in [0xf5dee51989u64, 0xa9fdca3312, 0x1bab10e32d, 0x3706b1677a, 0x644d626ffd].iter().enumerate() {
            if c0 & (1 << bit) != 0 {
                c ^= k;
            }
        }
        c
    }
    let (mut c, mut cls, mut count) = (1u64, 0u64, 0);
    for ch in desc.chars() {
        let pos = INPUT.find(ch)? as u64;
        c = polymod(c, pos & 31);
        cls = cls * 3 + (pos >> 5);
        count += 1;
        if count == 3 {
            c = polymod(c, cls);
            cls = 0;
            count = 0;
        }
    }
    if count > 0 {
        c = polymod(c, cls);
    }
    for _ in 0..8 {
        c = polymod(c, 0);
    }
    c ^= 1;
    Some((0..8).map(|j| CHECKSUM[((c >> (5 * (7 - j))) & 31) as usize] as char).collect())
}

// ── Signed PSBTs coming back ─────────────────────────────────────

/// Parse what a signer handed back: a PSBT (raw bytes, base64 or hex) or a
/// signed transaction (raw bytes or hex). Returns `original` with the new
/// signatures merged in, after checking that the transaction is the very one
/// that was sent (a signer that adds, drops or changes an input or output is
/// refused) and that every new signature commits to the whole transaction
/// (SIGHASH_DEFAULT or SIGHASH_ALL only).
pub fn merge_signed(original: &Psbt, returned: &[u8]) -> Result<Psbt, String> {
    let decoded = decode_returned(returned)?;
    let mut merged = original.clone();
    match decoded {
        Returned::Psbt(signed) => {
            if signed.unsigned_tx != original.unsigned_tx {
                return Err("The signer changed the transaction (inputs, outputs or amounts); refusing it".into());
            }
            for (i, (ours, theirs)) in merged.inputs.iter_mut().zip(signed.inputs).enumerate() {
                if has_witness(ours) {
                    continue;
                }
                if let Some(w) = theirs.final_script_witness.filter(|w| !w.is_empty()) {
                    check_witness_sighash(i, &w)?;
                    ours.final_script_witness = Some(w);
                }
                if let Some(sig) = theirs.tap_key_sig {
                    check_sighash(i, sig.sighash_type)?;
                    ours.tap_key_sig = Some(sig);
                }
                for (key, sig) in theirs.tap_script_sigs {
                    check_sighash(i, sig.sighash_type)?;
                    ours.tap_script_sigs.insert(key, sig);
                }
            }
        }
        Returned::Tx(tx) => {
            let mut bare = tx.clone();
            bare.input.iter_mut().for_each(|i| i.witness = Witness::default());
            if bare != original.unsigned_tx {
                return Err("The signed transaction is not the one that was sent; refusing it".into());
            }
            for (i, (ours, theirs)) in merged.inputs.iter_mut().zip(tx.input).enumerate() {
                if !has_witness(ours) && !theirs.witness.is_empty() {
                    check_witness_sighash(i, &theirs.witness)?;
                    ours.final_script_witness = Some(theirs.witness);
                }
            }
        }
    }
    Ok(merged)
}

fn decode_returned(returned: &[u8]) -> Result<Returned, String> {
    fn psbt(bytes: &[u8]) -> Result<Returned, String> {
        Psbt::deserialize(bytes).map(Returned::Psbt).map_err(|e| format!("Not a valid PSBT: {e}"))
    }
    if returned.starts_with(b"psbt\xff") {
        return psbt(returned);
    }
    // Text forms: drop all whitespace (wrapped base64 from files and QR apps).
    let text: String = std::str::from_utf8(returned)
        .map(|t| t.chars().filter(|c| !c.is_whitespace()).collect())
        .unwrap_or_default();
    if !text.is_empty() {
        if let Ok(bytes) = hex_decode(&text) {
            if bytes.starts_with(b"psbt\xff") {
                return psbt(&bytes);
            }
            if let Ok(tx) = bitcoin::consensus::deserialize::<Transaction>(&bytes) {
                return Ok(Returned::Tx(tx));
            }
        }
        if let Ok(bytes) = bitcoin::base64::Engine::decode(&bitcoin::base64::engine::general_purpose::STANDARD, &text) {
            if bytes.starts_with(b"psbt\xff") {
                return psbt(&bytes);
            }
        }
    }
    // A binary signed transaction (Coldcard's .txn file, BBQr type T).
    if let Ok(tx) = bitcoin::consensus::deserialize::<Transaction>(returned) {
        return Ok(Returned::Tx(tx));
    }
    if text.is_empty() {
        Err("Nothing to read: paste or load the signed PSBT".into())
    } else {
        Err("Not a PSBT (base64, hex or file) or a signed transaction (hex or file)".into())
    }
}

fn has_witness(input: &bitcoin::psbt::Input) -> bool {
    input.final_script_witness.as_ref().is_some_and(|w| !w.is_empty())
}

/// Only signatures that commit to every input and output are taken: an
/// ANYONECANPAY or NONE/SINGLE signature on the parent input would let anyone
/// reuse it in another transaction and take the parent's sat.
fn check_sighash(input: usize, ty: TapSighashType) -> Result<(), String> {
    match ty {
        TapSighashType::Default | TapSighashType::All => Ok(()),
        other => Err(format!(
            "Input {input} was signed with {other}, which does not commit to the whole transaction. Sign again with the default sighash type."
        )),
    }
}

/// The sighash of a finalized Taproot witness: a 64-byte signature is
/// DEFAULT; a 65-byte one carries its type in the last byte.
fn check_witness_sighash(input: usize, witness: &Witness) -> Result<(), String> {
    let Some(sig) = witness.nth(0) else { return Ok(()) };
    match sig.len() {
        64 => Ok(()),
        65 => check_sighash(
            input,
            TapSighashType::from_consensus_u8(sig[64]).map_err(|_| format!("Input {input} has an unknown sighash type"))?,
        ),
        _ => Err(format!("Input {input} does not carry a Taproot signature")),
    }
}

enum Returned {
    Psbt(Psbt),
    Tx(Transaction),
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ())).collect()
}

/// Move key-path signatures into `final_script_witness` (wallets that leave
/// the PSBT unfinalized, like UniSat with `autoFinalized: false`).
pub fn finalize_key_path_inputs(psbt: &mut Psbt) {
    for input in psbt.inputs.iter_mut() {
        if has_witness(input) {
            continue;
        }
        if let Some(sig) = input.tap_key_sig {
            input.final_script_witness = Some(Witness::from_slice(&[sig.to_vec()]));
        }
    }
}

/// Inputs still missing a witness, by index.
pub fn unsigned_inputs(psbt: &Psbt) -> Vec<usize> {
    psbt.inputs
        .iter()
        .enumerate()
        .filter(|(_, i)| !has_witness(i))
        .map(|(n, _)| n)
        .collect()
}

/// Check every key-path signature (one-element witness) against the BIP-341
/// sighash and the output key of the coin it spends, so a signature from the
/// wrong key fails here, with a clear message, instead of at broadcast.
pub fn verify_key_path_signatures(psbt: &Psbt) -> Result<(), String> {
    let prevouts: Vec<TxOut> = psbt
        .inputs
        .iter()
        .map(|i| i.witness_utxo.clone().ok_or("An input has no witness_utxo"))
        .collect::<Result<_, _>>()?;
    let secp = Secp256k1::verification_only();
    let mut cache = SighashCache::new(&psbt.unsigned_tx);
    for (i, input) in psbt.inputs.iter().enumerate() {
        let Some(witness) = input.final_script_witness.as_ref().filter(|w| w.len() == 1) else { continue };
        let script = &prevouts[i].script_pubkey;
        if !script.is_p2tr() {
            continue;
        }
        let output_key = XOnlyPublicKey::from_slice(&script.as_bytes()[2..34]).map_err(|e| e.to_string())?;
        let sig = bitcoin::taproot::Signature::from_slice(&witness[0])
            .map_err(|_| format!("Input {i}: not a Taproot signature"))?;
        check_sighash(i, sig.sighash_type)?;
        let sighash = cache
            .taproot_key_spend_signature_hash(i, &Prevouts::All(&prevouts), sig.sighash_type)
            .map_err(|e| format!("Input {i}: {e}"))?;
        let msg = Message::from_digest(bitcoin::hashes::Hash::to_byte_array(sighash));
        secp.verify_schnorr(&sig.signature, &msg, &output_key)
            .map_err(|_| format!("Input {i}: the signature does not match this address's key. Was it signed by the wallet that holds it?"))?;
    }
    Ok(())
}

/// The finished transaction, with every witness in place.
pub fn extract(psbt: &Psbt) -> Result<Transaction, String> {
    let missing = unsigned_inputs(psbt);
    if !missing.is_empty() {
        return Err(format!("Input(s) {missing:?} are not signed yet"));
    }
    let mut tx = psbt.unsigned_tx.clone();
    for (txin, input) in tx.input.iter_mut().zip(&psbt.inputs) {
        txin.witness = input.final_script_witness.clone().unwrap_or_default();
    }
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::txbuilder::{build_commit_reveal, Utxo};
    use binst_inscription::{BinstEntity, InstitutionBody};
    use bitcoin::key::TweakedPublicKey;
    use bitcoin::{Address, Amount, OutPoint};

    fn secret(byte: u8) -> SecretKey {
        SecretKey::from_slice(&[byte; 32]).unwrap()
    }

    fn wallet() -> (WalletKey, Address) {
        let secp = Secp256k1::new();
        let (internal, _) = Keypair::from_secret_key(&secp, &secret(7)).x_only_public_key();
        let address = Address::p2tr(&secp, internal, None, bitcoin::KnownHrp::Testnets);
        let origin = (Fingerprint::from([1, 2, 3, 4]), "m/86'/1'/0'/0/0".parse().unwrap());
        (WalletKey { internal, origin: Some(origin), script_pubkey: address.script_pubkey() }, address)
    }

    fn entity(admin: &XOnlyPublicKey) -> BinstEntity {
        BinstEntity::Institution(InstitutionBody {
            v: 0,
            name: "Regtest".into(),
            admin: admin.to_string(),
            citrea_contract: None,
            membership_rune: None,
            description: None,
            website: None,
            vault: None,
        })
    }

    fn plan(parent: bool) -> (InscriptionPlan, SecretKey, WalletKey) {
        let (wallet, address) = wallet();
        let reveal_secret = secret(9);
        let secp = Secp256k1::new();
        let (reveal_key, _) = Keypair::from_secret_key(&secp, &reveal_secret).x_only_public_key();
        let utxo = Utxo {
            outpoint: OutPoint::new("11".repeat(32).parse().unwrap(), 0),
            amount: Amount::from_sat(100_000),
            script_pubkey: address.script_pubkey(),
        };
        let parent_utxo = parent.then(|| Utxo {
            outpoint: OutPoint::new("22".repeat(32).parse().unwrap(), 1),
            amount: Amount::from_sat(546),
            script_pubkey: address.script_pubkey(),
        });
        let parent_id = parent.then(|| format!("{}i0", "33".repeat(32)));
        let plan = build_commit_reveal(
            &reveal_key,
            &entity(&wallet.internal),
            &[utxo],
            2,
            &address,
            Network::Testnet4,
            parent_id.as_deref(),
            parent_utxo,
        )
        .unwrap();
        (plan, reveal_secret, wallet)
    }

    #[test]
    fn reveal_key_is_leaf_and_internal_key() {
        let (plan, secret, _) = plan(false);
        let secp = Secp256k1::new();
        let (xonly, _) = Keypair::from_secret_key(&secp, &secret).x_only_public_key();
        assert_eq!(plan.taproot_spend_info.internal_key(), xonly);
        assert_eq!(&plan.inscription_script.as_bytes()[1..33], &xonly.serialize());
    }

    #[test]
    fn signed_reveal_verifies_against_the_script_path_sighash() {
        for parent in [false, true] {
            let (plan, secret, wallet) = plan(parent);
            let mut psbt = reveal_psbt(&plan, &wallet).unwrap();
            sign_reveal_input(&mut psbt, &plan, &secret).unwrap();
            let idx = plan.commit_input_index();
            let witness = psbt.inputs[idx].final_script_witness.clone().unwrap();
            assert_eq!(witness.len(), 3);
            let prevouts: Vec<TxOut> = psbt.inputs.iter().map(|i| i.witness_utxo.clone().unwrap()).collect();
            let leaf_hash = TapLeafHash::from_script(&plan.inscription_script, LeafVersion::TapScript);
            let sighash = SighashCache::new(&psbt.unsigned_tx)
                .taproot_script_spend_signature_hash(idx, &Prevouts::All(&prevouts), leaf_hash, TapSighashType::Default)
                .unwrap();
            let sig = bitcoin::secp256k1::schnorr::Signature::from_slice(&witness[0]).unwrap();
            let msg = Message::from_digest(bitcoin::hashes::Hash::to_byte_array(sighash));
            Secp256k1::verification_only()
                .verify_schnorr(&sig, &msg, &plan.taproot_spend_info.internal_key())
                .unwrap();
            // Only the wallet's inputs are left to sign.
            let expected: Vec<usize> = if parent { vec![0] } else { vec![] };
            assert_eq!(unsigned_inputs(&psbt), expected);
        }
    }

    #[test]
    fn wrong_reveal_key_is_refused() {
        let (plan, _, wallet) = plan(false);
        let mut psbt = reveal_psbt(&plan, &wallet).unwrap();
        assert!(sign_reveal_input(&mut psbt, &plan, &secret(3)).is_err());
    }

    #[test]
    fn wallet_inputs_and_change_carry_bip371_origins() {
        let (plan, _, wallet) = plan(true);
        let commit = commit_psbt(&plan, &wallet).unwrap();
        assert_eq!(commit.inputs[0].tap_internal_key, Some(wallet.internal));
        assert!(commit.inputs[0].tap_key_origins.contains_key(&wallet.internal));
        // Commit output 0 is the inscription script; output 1 is change.
        assert!(commit.outputs[0].tap_internal_key.is_none());
        assert_eq!(commit.outputs[1].tap_internal_key, Some(wallet.internal));
        let reveal = reveal_psbt(&plan, &wallet).unwrap();
        assert_eq!(reveal.inputs[0].tap_internal_key, Some(wallet.internal));
        assert!(reveal.inputs[1].tap_key_origins.is_empty(), "no fake origin on the script-path input");
        assert_eq!(reveal.inputs[1].tap_merkle_root, plan.taproot_spend_info.merkle_root());
    }

    #[test]
    fn extension_wallet_gets_the_key_but_no_origins() {
        let (plan, _, mut wallet) = plan(false);
        wallet.origin = None;
        let commit = commit_psbt(&plan, &wallet).unwrap();
        assert_eq!(commit.inputs[0].tap_internal_key, Some(wallet.internal));
        assert!(commit.inputs[0].tap_key_origins.is_empty());
        assert!(commit.inputs[0].witness_utxo.is_some());
    }

    #[test]
    fn recovery_descriptor_controls_the_commit_output() {
        let (plan, secret, _) = plan(false);
        let desc = recovery_descriptor(&plan, &secret);
        assert!(desc.starts_with("rawtr(c") || desc.starts_with("rawtr("));
        let wif = &desc["rawtr(".len()..desc.find(')').unwrap()];
        let key = PrivateKey::from_wif(wif).unwrap();
        let secp = Secp256k1::new();
        let (xonly, _) = key.inner.x_only_public_key(&secp);
        let output = ScriptBuf::new_p2tr_tweaked(TweakedPublicKey::dangerous_assume_tweaked(xonly));
        assert_eq!(output, plan.commit_tx.output[0].script_pubkey);
    }

    #[test]
    fn descriptor_checksum_matches_bip380_examples() {
        // Test vectors from BIP-380.
        assert_eq!(descriptor_checksum("raw(deadbeef)").as_deref(), Some("89f8spxm"));
        assert_eq!(
            descriptor_checksum("pkh([d34db33f/44'/0'/0']xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL/1/*)")
                .as_deref(),
            Some("ml40v0wf")
        );
    }

    #[test]
    fn merge_accepts_signed_psbt_and_refuses_a_changed_one() {
        let (plan, secret, wallet) = plan(true);
        let mut ours = reveal_psbt(&plan, &wallet).unwrap();
        sign_reveal_input(&mut ours, &plan, &secret).unwrap();

        // A signer returns the PSBT with a key-path signature on the parent input.
        let mut theirs = ours.clone();
        theirs.inputs[1].final_script_witness = None; // signers may drop finalized data
        theirs.inputs[0].tap_key_sig = Some(bitcoin::taproot::Signature {
            signature: bitcoin::secp256k1::schnorr::Signature::from_slice(&[1u8; 64]).unwrap(),
            sighash_type: TapSighashType::Default,
        });
        let b64 = crate::txbuilder::psbt_to_base64(&theirs);
        let mut merged = merge_signed(&ours, b64.as_bytes()).unwrap();
        finalize_key_path_inputs(&mut merged);
        assert!(unsigned_inputs(&merged).is_empty());
        assert!(merged.inputs[1].final_script_witness.is_some(), "our reveal signature survives");

        // Raw bytes and hex work too.
        assert!(merge_signed(&ours, &theirs.serialize()).is_ok());

        // A changed output is refused.
        let mut tampered = theirs.clone();
        tampered.unsigned_tx.output[0].value = Amount::from_sat(1);
        let err = merge_signed(&ours, crate::txbuilder::psbt_to_base64(&tampered).as_bytes()).unwrap_err();
        assert!(err.contains("changed"));
    }

    #[test]
    fn anyonecanpay_and_none_signatures_are_refused() {
        let (plan, secret, wallet) = plan(true);
        let mut ours = reveal_psbt(&plan, &wallet).unwrap();
        sign_reveal_input(&mut ours, &plan, &secret).unwrap();
        for ty in [TapSighashType::AllPlusAnyoneCanPay, TapSighashType::None, TapSighashType::SinglePlusAnyoneCanPay] {
            let mut theirs = ours.clone();
            theirs.inputs[0].tap_key_sig = Some(bitcoin::taproot::Signature {
                signature: bitcoin::secp256k1::schnorr::Signature::from_slice(&[1u8; 64]).unwrap(),
                sighash_type: ty,
            });
            let err = merge_signed(&ours, &theirs.serialize()).unwrap_err();
            assert!(err.contains("does not commit"), "{ty}: {err}");
            // Same through a finalized raw transaction (65-byte signature).
            let mut tx = ours.unsigned_tx.clone();
            let mut sig = vec![1u8; 64];
            sig.push(ty as u8);
            tx.input[0].witness = Witness::from_slice(&[sig]);
            assert!(merge_signed(&ours, &bitcoin::consensus::serialize(&tx)).is_err(), "{ty} raw");
        }
    }

    #[test]
    fn key_path_signatures_are_verified_against_the_coin() {
        let secp = Secp256k1::new();
        let (plan, secret, wallet) = plan(true);
        let mut psbt = reveal_psbt(&plan, &wallet).unwrap();
        sign_reveal_input(&mut psbt, &plan, &secret).unwrap();
        // Sign the parent input with the wallet's tweaked key (BIP-86), as a wallet would.
        let prevouts: Vec<TxOut> = psbt.inputs.iter().map(|i| i.witness_utxo.clone().unwrap()).collect();
        let sighash = SighashCache::new(&psbt.unsigned_tx)
            .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), TapSighashType::Default)
            .unwrap();
        let msg = Message::from_digest(bitcoin::hashes::Hash::to_byte_array(sighash));
        let tweaked = Keypair::from_secret_key(&secp, &secret_for_wallet()).tap_tweak(&secp, None);
        let good = secp.sign_schnorr_no_aux_rand(&msg, &tweaked.to_keypair());
        psbt.inputs[0].final_script_witness = Some(Witness::from_slice(&[good.as_ref().to_vec()]));
        verify_key_path_signatures(&psbt).unwrap();
        // The untweaked key's signature is caught before broadcast.
        let bad = secp.sign_schnorr_no_aux_rand(&msg, &Keypair::from_secret_key(&secp, &secret_for_wallet()));
        psbt.inputs[0].final_script_witness = Some(Witness::from_slice(&[bad.as_ref().to_vec()]));
        assert!(verify_key_path_signatures(&psbt).unwrap_err().contains("does not match"));
    }

    fn secret_for_wallet() -> SecretKey {
        secret(7)
    }

    #[test]
    fn wrapped_base64_binary_tx_and_empty_witness() {
        let (plan, secret, wallet) = plan(true);
        let mut ours = reveal_psbt(&plan, &wallet).unwrap();
        sign_reveal_input(&mut ours, &plan, &secret).unwrap();
        let mut theirs = ours.clone();
        theirs.inputs[0].final_script_witness = Some(Witness::default()); // empty, with a key sig
        theirs.inputs[0].tap_key_sig = Some(bitcoin::taproot::Signature {
            signature: bitcoin::secp256k1::schnorr::Signature::from_slice(&[1u8; 64]).unwrap(),
            sighash_type: TapSighashType::Default,
        });
        let b64 = crate::txbuilder::psbt_to_base64(&theirs);
        let wrapped: String = b64.as_bytes().chunks(60).map(|c| format!("{}\n", std::str::from_utf8(c).unwrap())).collect();
        let mut merged = merge_signed(&ours, wrapped.as_bytes()).unwrap();
        finalize_key_path_inputs(&mut merged);
        assert!(unsigned_inputs(&merged).is_empty());
        let mut tx = ours.unsigned_tx.clone();
        tx.input[0].witness = Witness::from_slice(&[[5u8; 64]]);
        let merged = merge_signed(&ours, &bitcoin::consensus::serialize(&tx)).unwrap();
        assert!(unsigned_inputs(&merged).is_empty());
    }

    #[test]
    fn merge_accepts_a_signed_raw_transaction() {
        let (plan, secret, wallet) = plan(true);
        let mut ours = reveal_psbt(&plan, &wallet).unwrap();
        sign_reveal_input(&mut ours, &plan, &secret).unwrap();
        let mut tx = ours.unsigned_tx.clone();
        tx.input[0].witness = Witness::from_slice(&[[5u8; 64]]);
        let hex: String = bitcoin::consensus::serialize(&tx).iter().map(|b| format!("{b:02x}")).collect();
        let merged = merge_signed(&ours, hex.as_bytes()).unwrap();
        assert!(unsigned_inputs(&merged).is_empty());
        assert_eq!(extract(&merged).unwrap().input[0].witness.len(), 1);
    }
}
