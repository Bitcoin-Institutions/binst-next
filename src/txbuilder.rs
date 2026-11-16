// reused-from: binst-pilot-webapp@dda2d1a:src/txbuilder.rs (2026-11-16) — adapted: none (verbatim)
//! Transaction builder — construct commit/reveal inscription transactions
//! and export unsigned PSBTs for wallet-agnostic signing.
//!
//! Uses `rust-bitcoin` for all Bitcoin primitives.  No wallet dependency.

use bitcoin::blockdata::opcodes::all::*;
use bitcoin::blockdata::script::Builder as ScriptBuilder;
use bitcoin::key::{UntweakedPublicKey, XOnlyPublicKey};
use bitcoin::opcodes;
use bitcoin::psbt::Psbt;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::taproot::{LeafVersion, TaprootBuilder, TaprootSpendInfo};
use bitcoin::{
    transaction, Address, Amount, Network, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Witness,
};

use binst_inscription::BinstEntity;

// ── Constants ────────────────────────────────────────────────────

/// Dust limit for Taproot outputs (546 sats).
const DUST_LIMIT: u64 = 546;

/// Maximum single push in Tapscript (520 bytes).
const MAX_PUSH: usize = 520;

/// NUMS point — provably unspendable internal key (BIP-341 recommended).
/// H = lift_x(SHA256("BINST/nums"))  — we use the standard nothing-up-my-sleeve point.
const NUMS_POINT_HEX: &str =
    "50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";

// ── Types ────────────────────────────────────────────────────────

/// A spendable UTXO provided by the user (from wallet or manual entry).
#[derive(Debug, Clone)]
pub struct Utxo {
    pub outpoint: OutPoint,
    pub amount: Amount,
    pub script_pubkey: ScriptBuf,
}

/// Result of building a commit+reveal pair.
#[derive(Debug, Clone)]
pub struct InscriptionPlan {
    /// Unsigned commit transaction (funds the Taproot script-path output).
    pub commit_tx: Transaction,
    /// Unsigned reveal transaction (spends the commit via Tapscript witness).
    pub reveal_tx: Transaction,
    /// Taproot spend info needed to finalize the reveal witness.
    pub taproot_spend_info: TaprootSpendInfo,
    /// The inscription script that goes in the Tapscript leaf.
    pub inscription_script: ScriptBuf,
    /// Estimated total fee in sats (commit + reveal).
    pub estimated_fee: u64,
    /// The UTXOs selected for the commit inputs (needed to populate PSBT witness_utxo).
    pub commit_utxos: Vec<Utxo>,
    /// Optional parent UTXO, spent as the *first* input of the reveal tx and
    /// returned at full value in the first output, so the parent inscription
    /// keeps its sat offset (see `build_commit_reveal`, step 7).
    pub parent_utxo: Option<Utxo>,
}

impl InscriptionPlan {
    /// Reveal input index that spends the commit output (script path).
    pub fn commit_input_index(&self) -> usize {
        if self.parent_utxo.is_some() { 1 } else { 0 }
    }

    /// Reveal output index that receives the new inscription's sat.
    pub fn inscription_vout(&self) -> u32 {
        if self.parent_utxo.is_some() { 1 } else { 0 }
    }
}

// ── Inscription script builder ───────────────────────────────────

/// Build the Tapscript inscription envelope for a BINST entity.
///
/// Layout (Ordinals standard):
/// ```text
/// <admin_pubkey> OP_CHECKSIG
/// OP_FALSE OP_IF
///   OP_PUSH "ord"
///   OP_PUSH 3  OP_PUSH <parent_txid_bytes>   ← only present when parent is set
///   OP_PUSH 1  OP_PUSH <content_type>
///   OP_PUSH 7  OP_PUSH "binst"
///   OP_0
///   OP_PUSH <body_chunk_1>
///   OP_PUSH <body_chunk_2>
///   ...
/// OP_ENDIF
/// ```
///
/// `parent_inscription_id` — optional Ordinals inscription ID of the parent
/// entity (e.g. the institution that owns this process template). When set, the
/// Ordinals parent tag (tag 3) is embedded in the envelope, creating an
/// on-chain cryptographic link that cannot be spoofed.
///
/// The parent ID format is `<txid>i<vout>` (e.g. `abc123...i0`). The txid
/// bytes are reversed (little-endian) before being pushed, as per the Ordinals
/// spec.
pub fn build_inscription_script(
    admin_pubkey: &XOnlyPublicKey,
    entity: &BinstEntity,
    parent_inscription_id: Option<&str>,
) -> Result<ScriptBuf, String> {
    // An institution's `admin` is the anchor for control and L2 authorization,
    // so it must be a real x-only key, not a placeholder.
    if let BinstEntity::Institution(inst) = entity {
        inst.admin.parse::<XOnlyPublicKey>().map_err(|_| {
            format!(
                "Institution admin must be a 64-hex x-only public key, got {:?}. Connect a BTC wallet first.",
                inst.admin
            )
        })?;
    }

    let body = serde_json::to_vec(entity)
        .map_err(|e| format!("JSON serialize error: {e}"))?;

    let mut b = ScriptBuilder::new()
        .push_x_only_key(admin_pubkey)
        .push_opcode(OP_CHECKSIG)
        .push_opcode(opcodes::OP_FALSE)
        .push_opcode(OP_IF);

    // "ord" magic
    b = b.push_slice(b"ord");

    // Tag 3: parent (optional) — txid bytes reversed (little-endian), stripped
    // of the trailing `i<vout>` suffix before encoding.
    if let Some(parent_id) = parent_inscription_id {
        let txid_hex = parent_id.split('i').next().unwrap_or(parent_id);
        if txid_hex.len() == 64 {
            let mut txid_bytes = (0..32)
                .map(|i| u8::from_str_radix(&txid_hex[i * 2..i * 2 + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .map_err(|e| format!("Invalid parent txid hex: {e}"))?;
            txid_bytes.reverse(); // Ordinals spec: little-endian
            b = b.push_int(3);
            b = push_bytes_checked(b, &txid_bytes)?;
        }
    }

    // Tag 1: content type
    b = b.push_int(1)
        .push_slice(b"application/json");

    // Tag 7: metaprotocol
    b = b.push_int(7)
        .push_slice(b"binst");

    // Body separator
    b = b.push_opcode(opcodes::OP_0);

    // Body in ≤520-byte chunks
    for chunk in body.chunks(MAX_PUSH) {
        b = push_bytes_checked(b, chunk)?;
    }

    b = b.push_opcode(OP_ENDIF);

    Ok(b.into_script())
}

/// Push a byte slice, handling the PushBytes length limit.
fn push_bytes_checked(
    builder: ScriptBuilder,
    data: &[u8],
) -> Result<ScriptBuilder, String> {
    let push: &bitcoin::script::PushBytes = data
        .try_into()
        .map_err(|_| format!("Push data too large: {} bytes", data.len()))?;
    Ok(builder.push_slice(push))
}

// ── Commit + Reveal builder ──────────────────────────────────────

/// Parse the NUMS (nothing-up-my-sleeve) internal key.
fn nums_internal_key() -> Result<UntweakedPublicKey, String> {
    NUMS_POINT_HEX
        .parse::<XOnlyPublicKey>()
        .map(|k| UntweakedPublicKey::from(k))
        .map_err(|e| format!("Bad NUMS point: {e}"))
}

/// Build a commit + reveal transaction pair for a single inscription.
///
/// - `admin_pubkey`: the x-only key that will sign the reveal (Tapscript path).
/// - `entity`: the BINST entity to inscribe.
/// - `utxos`: available UTXOs to fund the commit.
/// - `fee_rate_sat_vb`: fee rate in sat/vB.
/// - `change_address`: where leftover sats go.
/// - `network`: Bitcoin network (for address derivation).
/// - `parent_inscription_id`: optional Ordinals parent ID (`<txid>i0`); embeds
///   tag 3 in the envelope to create a cryptographic on-chain parent-child link.
/// - `parent_utxo`: the UTXO holding the parent inscription's sat. **Required
///   for the Ordinals provenance rule** — the parent sat must be spent as an
///   input of the reveal tx. It is spent as input 0 and returned at full value
///   in output 0 so it survives; the child then lands in output 1. Without this,
///   tag 3 alone is not sufficient for the indexer to recognise the parent-child
///   link.
pub fn build_commit_reveal(
    admin_pubkey: &XOnlyPublicKey,
    entity: &BinstEntity,
    utxos: &[Utxo],
    fee_rate_sat_vb: u64,
    change_address: &Address,
    _network: Network,
    parent_inscription_id: Option<&str>,
    parent_utxo: Option<Utxo>,
) -> Result<InscriptionPlan, String> {
    let secp = Secp256k1::verification_only();

    // 1. Build inscription Tapscript.
    let inscription_script = build_inscription_script(admin_pubkey, entity, parent_inscription_id)?;

    // 2. Build Taproot tree with one leaf.
    let internal_key = nums_internal_key()?;
    let taproot = TaprootBuilder::new()
        .add_leaf(0, inscription_script.clone())
        .map_err(|e| format!("Taproot leaf error: {e:?}"))?
        .finalize(&secp, internal_key)
        .map_err(|_| "Taproot finalize failed".to_string())?;

    let commit_output_script = ScriptBuf::new_p2tr(
        &secp,
        internal_key,
        taproot.merkle_root(),
    );

    // 3. Fee estimates from the signed size of each transaction. The witnesses
    //    are placeholders of the real sizes: a 64-byte Schnorr signature
    //    (SIGHASH_DEFAULT) for key-path inputs, and signature + script + control
    //    block for the commit's script-path input. Funding UTXOs are P2TR
    //    key-path at the wallet address.
    let control_block = taproot
        .control_block(&(inscription_script.clone(), LeafVersion::TapScript))
        .ok_or("Could not compute control block")?;
    let key_path_witness = Witness::from_slice(&[[0u8; 64]]);
    let script_path_witness = Witness::from_slice(&[
        &[0u8; 64][..],
        inscription_script.as_bytes(),
        &control_block.serialize(),
    ]);
    let mut reveal_witnesses = vec![script_path_witness];
    if parent_utxo.is_some() {
        reveal_witnesses.push(key_path_witness.clone());
    }
    // One output per input: the inscription, plus the parent when present.
    let reveal_fee = signed_vsize(&reveal_witnesses, reveal_witnesses.len()) * fee_rate_sat_vb;

    // 4. The commit output must fund: reveal_fee + dust for the inscription output.
    let commit_output_value = reveal_fee + DUST_LIMIT;

    // 5. Coin selection (simple: take UTXOs until we have enough). The commit
    //    fee grows with each input, so it is recomputed as inputs are added;
    //    it assumes a change output (commit output + change).
    let commit_fee_for = |n_inputs: usize| {
        signed_vsize(&vec![key_path_witness.clone(); n_inputs.max(1)], 2) * fee_rate_sat_vb
    };
    let mut selected: Vec<Utxo> = Vec::new();
    let mut total_in = Amount::ZERO;
    let mut commit_fee = commit_fee_for(1);
    for utxo in utxos {
        selected.push(utxo.clone());
        total_in += utxo.amount;
        commit_fee = commit_fee_for(selected.len());
        if total_in.to_sat() >= commit_output_value + commit_fee {
            break;
        }
    }
    let needed = commit_output_value + commit_fee;
    if total_in.to_sat() < needed {
        return Err(format!(
            "Insufficient funds: need {} sats, have {} sats",
            needed,
            total_in.to_sat(),
        ));
    }

    // 6. Build commit transaction.
    let commit_inputs: Vec<TxIn> = selected
        .iter()
        .map(|u| TxIn {
            previous_output: u.outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        })
        .collect();

    let mut commit_outputs = vec![TxOut {
        value: Amount::from_sat(commit_output_value),
        script_pubkey: commit_output_script,
    }];

    let change = total_in.to_sat() - commit_output_value - commit_fee;
    if change > DUST_LIMIT {
        commit_outputs.push(TxOut {
            value: Amount::from_sat(change),
            script_pubkey: change_address.script_pubkey(),
        });
    }

    let commit_tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: commit_inputs,
        output: commit_outputs,
    };

    // 7. Build reveal transaction.
    //    Ordinal theory assigns sats to outputs first-in-first-out, and the fee
    //    is the tail of the last input. So the input order decides where the
    //    parent's sat ends up:
    //
    //    Without a parent:  input 0 = commit (Tapscript path), output 0 = new inscription.
    //    With a parent:     input 0 = parent UTXO, output 0 = parent returned at full value
    //                       (its sat keeps the same offset);
    //                       input 1 = commit, output 1 = new inscription (546 sats),
    //                       and the reveal fee comes out of the commit's tail.
    //
    //    Placing the parent after the commit pushes its sat past the outputs and
    //    into the fee as soon as the fee exceeds 546 sats; the Ordinals provenance
    //    rule only needs the parent to be spent, not its position, so the parent goes
    //    first. ord still assigns the child ID `<reveal_txid>i0` (it counts envelopes).
    let commit_txid = commit_tx.compute_txid();

    let commit_input = TxIn {
        previous_output: OutPoint::new(commit_txid, 0),
        script_sig: ScriptBuf::new(),
        sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
        witness: Witness::default(), // filled at signing time
    };

    // The new inscription's sat (dust) goes to the wallet's key-path P2TR address
    // (change_address) so the wallet owns and can spend it as a parent UTXO.
    //
    // We do NOT send it to the Tapscript commit address (taproot.output_key()).
    // That would lock the sat at a one-time script the wallet cannot key-path-sign,
    // making it impossible for the wallet to spend it when creating a child
    // inscription (process template) whose reveal tx must spend the parent sat.
    let inscription_output = TxOut {
        value: Amount::from_sat(DUST_LIMIT),
        script_pubkey: change_address.script_pubkey(),
    };

    let (reveal_inputs, reveal_outputs) = match parent_utxo {
        None => (vec![commit_input], vec![inscription_output]),
        Some(ref p_utxo) => {
            let parent_input = TxIn {
                previous_output: p_utxo.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::default(),
            };
            // Same value as the parent UTXO, so every parent sat maps to output 0.
            let parent_output = TxOut {
                value: p_utxo.amount,
                script_pubkey: change_address.script_pubkey(),
            };
            (
                vec![parent_input, commit_input],
                vec![parent_output, inscription_output],
            )
        }
    };

    let reveal_tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: reveal_inputs,
        output: reveal_outputs,
    };

    Ok(InscriptionPlan {
        commit_tx,
        reveal_tx,
        taproot_spend_info: taproot,
        inscription_script,
        estimated_fee: commit_fee + reveal_fee,
        commit_utxos: selected,
        parent_utxo,
    })
}

/// Virtual size of a transaction with one input per witness (each witness a
/// placeholder of the final size) and `n_outputs` P2TR outputs.
fn signed_vsize(witnesses: &[Witness], n_outputs: usize) -> u64 {
    let p2tr = ScriptBuf::from_bytes([&[0x51, 0x20][..], &[0u8; 32][..]].concat());
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: witnesses
            .iter()
            .map(|w| TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: w.clone(),
            })
            .collect(),
        output: vec![TxOut { value: Amount::ZERO, script_pubkey: p2tr }; n_outputs],
    };
    tx.vsize() as u64
}

// ── PSBT export ──────────────────────────────────────────────────

/// Convert the commit transaction into an unsigned PSBT for external signing.
pub fn commit_to_psbt(plan: &InscriptionPlan) -> Result<Psbt, String> {
    let mut psbt = Psbt::from_unsigned_tx(plan.commit_tx.clone())
        .map_err(|e| format!("PSBT creation failed: {e}"))?;
    // witness_utxo is required for Taproot signing — without it wallets
    // show "UNKNOWN SCRIPT" and refuse to sign.
    for (i, utxo) in plan.commit_utxos.iter().enumerate() {
        if let Some(input) = psbt.inputs.get_mut(i) {
            input.witness_utxo = Some(TxOut {
                value: utxo.amount,
                script_pubkey: utxo.script_pubkey.clone(),
            });
        }
    }
    Ok(psbt)
}

/// Serialize a PSBT to base64 (standard interchange format).
pub fn psbt_to_base64(psbt: &Psbt) -> String {
    let buf = psbt.serialize();
    use bitcoin::base64::{Engine, engine::general_purpose::STANDARD};
    STANDARD.encode(&buf)
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::Txid;
    use binst_inscription::InstitutionBody;
    use std::str::FromStr;

    fn test_pubkey() -> XOnlyPublicKey {
        XOnlyPublicKey::from_str(
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .unwrap()
    }

    fn test_entity() -> BinstEntity {
        BinstEntity::Institution(InstitutionBody {
            v: 0,
            name: "TestInst".into(),
            admin: "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into(),
            citrea_contract: None,
            membership_rune: None,
            description: Some("A test institution".into()),
            website: None,
            vault: None,
        })
    }

    fn test_utxo(sats: u64) -> Utxo {
        Utxo {
            outpoint: OutPoint::new(
                Txid::from_str(
                    "0000000000000000000000000000000000000000000000000000000000000001",
                )
                .unwrap(),
                0,
            ),
            amount: Amount::from_sat(sats),
            script_pubkey: ScriptBuf::new_p2tr_tweaked(
                bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(test_pubkey()),
            ),
        }
    }

    // ── inscription script ───────────────────────────────

    #[test]
    fn commit_vsize_matches_measured_regtest_commit() {
        // 1 P2TR key-path input, commit + change outputs: 154 vB on regtest
        // (testmempoolaccept, 2026-10-27).
        let w = Witness::from_slice(&[[0u8; 64]]);
        assert_eq!(signed_vsize(&[w], 2), 154);
    }

    #[test]
    fn reveal_fee_equals_signed_reveal_vsize_times_rate() {
        let pk = test_pubkey();
        let addr = Address::p2tr_tweaked(
            bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            Network::Testnet,
        );
        let parent = Utxo {
            outpoint: OutPoint::new(
                Txid::from_str("abcdef0000000000000000000000000000000000000000000000000000000001").unwrap(),
                0,
            ),
            amount: Amount::from_sat(DUST_LIMIT),
            script_pubkey: addr.script_pubkey(),
        };
        let parent_id = "abcdef0000000000000000000000000000000000000000000000000000000001i0";
        for with_parent in [false, true] {
            let fee_rate = 7;
            let plan = build_commit_reveal(
                &pk, &test_entity(), &[test_utxo(200_000)], fee_rate, &addr, Network::Testnet,
                with_parent.then_some(parent_id), with_parent.then_some(parent.clone()),
            )
            .unwrap();
            // Put real-size witnesses on the actual reveal and measure it.
            let cb = plan
                .taproot_spend_info
                .control_block(&(plan.inscription_script.clone(), LeafVersion::TapScript))
                .unwrap()
                .serialize();
            let mut reveal = plan.reveal_tx.clone();
            for (i, input) in reveal.input.iter_mut().enumerate() {
                input.witness = if i == plan.commit_input_index() {
                    Witness::from_slice(&[&[0u8; 64][..], plan.inscription_script.as_bytes(), &cb])
                } else {
                    Witness::from_slice(&[[0u8; 64]])
                };
            }
            let paid = plan.commit_tx.output[0].value.to_sat() - DUST_LIMIT;
            assert_eq!(paid, reveal.vsize() as u64 * fee_rate, "with_parent={with_parent}");
        }
    }

    #[test]
    fn inscription_script_rejects_placeholder_admin() {
        let mut entity = test_entity();
        if let BinstEntity::Institution(ref mut inst) = entity {
            inst.admin = "<connect wallet>".into();
        }
        let err = build_inscription_script(&test_pubkey(), &entity, None).unwrap_err();
        assert!(err.contains("x-only public key"), "{err}");
    }

    #[test]
    fn inscription_script_contains_envelope() {
        let script = build_inscription_script(&test_pubkey(), &test_entity(), None).unwrap();
        let bytes = script.as_bytes();
        // Must contain OP_FALSE(0x00) OP_IF(0x63) somewhere
        let has_envelope = bytes.windows(2).any(|w| w == [0x00, 0x63]);
        assert!(has_envelope, "script must contain OP_FALSE OP_IF");
        // Must contain OP_ENDIF(0x68)
        assert!(bytes.contains(&0x68), "script must contain OP_ENDIF");
        // Must contain "ord" magic
        assert!(
            bytes.windows(3).any(|w| w == b"ord"),
            "script must contain 'ord' magic"
        );
        // Must contain "binst" metaprotocol
        assert!(
            bytes.windows(5).any(|w| w == b"binst"),
            "script must contain 'binst' metaprotocol"
        );
    }

    #[test]
    fn inscription_script_contains_json_body() {
        let script = build_inscription_script(&test_pubkey(), &test_entity(), None).unwrap();
        let bytes = script.as_bytes();
        // The JSON body must be somewhere in the script
        assert!(
            bytes.windows(8).any(|w| w == b"TestInst"),
            "script must contain entity name in JSON body"
        );
    }

    #[test]
    fn inscription_script_embeds_parent_tag() {
        // A well-known 32-byte txid (all zeros except last byte = 1) as hex
        let parent_id = "0000000000000000000000000000000000000000000000000000000000000001i0";
        let script =
            build_inscription_script(&test_pubkey(), &test_entity(), Some(parent_id)).unwrap();
        let bytes = script.as_bytes();
        // Tag 3 must appear as a push of the byte 0x03 immediately after "ord"
        // Find "ord" bytes and check the byte two positions after for tag 3 marker
        let ord_pos = bytes
            .windows(3)
            .position(|w| w == b"ord")
            .expect("must contain 'ord' magic");
        // After ord: push(3 bytes="ord") then push_int(3) = 0x53 (OP_3) or 0x01 0x03
        // push_int(3) in script builder emits OP_3 (0x53)
        let after_ord = &bytes[ord_pos + 3..];
        assert!(
            after_ord.iter().take(4).any(|&b| b == 3 || b == 0x53),
            "tag 3 (parent) must appear immediately after 'ord' magic; bytes after ord: {:?}",
            &after_ord[..after_ord.len().min(8)]
        );
    }

    // ── commit + reveal ──────────────────────────────────

    #[test]
    fn build_commit_reveal_basic() {
        let pk = test_pubkey();
        let addr = Address::p2tr_tweaked(
            bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            Network::Testnet,
        );
        let plan = build_commit_reveal(
            &pk,
            &test_entity(),
            &[test_utxo(100_000)],
            2,
            &addr,
            Network::Testnet,
            None,
            None,
        )
        .unwrap();

        // Commit has at least 1 input and 1 output
        assert!(!plan.commit_tx.input.is_empty());
        assert!(!plan.commit_tx.output.is_empty());

        // Reveal spends commit output 0
        assert_eq!(plan.reveal_tx.input.len(), 1);
        assert_eq!(
            plan.reveal_tx.input[0].previous_output.txid,
            plan.commit_tx.compute_txid()
        );
        assert_eq!(plan.reveal_tx.input[0].previous_output.vout, 0);

        // Reveal has exactly 1 output (the inscription)
        assert_eq!(plan.reveal_tx.output.len(), 1);
        assert_eq!(plan.reveal_tx.output[0].value.to_sat(), DUST_LIMIT);
        // Inscription sat must go to the wallet address, not the Tapscript key.
        assert_eq!(
            plan.reveal_tx.output[0].script_pubkey,
            addr.script_pubkey(),
            "inscription sat must go to wallet key-path address"
        );

        // Fee estimate is positive
        assert!(plan.estimated_fee > 0);
    }

    #[test]
    fn build_commit_reveal_insufficient_funds() {
        let pk = test_pubkey();
        let addr = Address::p2tr_tweaked(
            bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            Network::Testnet,
        );
        let result = build_commit_reveal(
            &pk,
            &test_entity(),
            &[test_utxo(100)], // only 100 sats — not enough
            2,
            &addr,
            Network::Testnet,
            None,
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Insufficient funds"));
    }

    #[test]
    fn build_commit_reveal_includes_change() {
        let pk = test_pubkey();
        let addr = Address::p2tr_tweaked(
            bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            Network::Testnet,
        );
        let plan = build_commit_reveal(
            &pk,
            &test_entity(),
            &[test_utxo(1_000_000)], // plenty of sats
            1,
            &addr,
            Network::Testnet,
            None,
            None,
        )
        .unwrap();

        // With 1M sats input and low fee, should have a change output
        assert_eq!(plan.commit_tx.output.len(), 2, "should have commit + change outputs");
    }

    // ── PSBT ─────────────────────────────────────────────

    #[test]
    fn psbt_round_trip() {
        let pk = test_pubkey();
        let addr = Address::p2tr_tweaked(
            bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            Network::Testnet,
        );
        let plan = build_commit_reveal(
            &pk,
            &test_entity(),
            &[test_utxo(100_000)],
            2,
            &addr,
            Network::Testnet,
            None,
            None,
        )
        .unwrap();

        let psbt = commit_to_psbt(&plan).unwrap();
        let b64 = psbt_to_base64(&psbt);
        assert!(!b64.is_empty(), "base64 PSBT should not be empty");
        // Base64 PSBTs start with "cHNidP8" (the base64 of "psbt\xff")
        assert!(b64.starts_with("cHNidP8"), "should be valid PSBT base64 prefix");
    }

    /// Ordinal theory: the sat at `offset` (counted across all inputs in order)
    /// lands in the output that covers that position, first-in-first-out.
    /// `None` means it falls past the outputs, into the fee.
    fn sat_destination(inputs: &[u64], outputs: &[u64], offset: u64) -> Option<(usize, u64)> {
        assert!(offset < inputs.iter().sum::<u64>(), "offset outside inputs");
        let mut start = 0;
        for (vout, &value) in outputs.iter().enumerate() {
            if offset < start + value {
                return Some((vout, offset - start));
            }
            start += value;
        }
        None
    }

    #[test]
    fn reveal_with_parent_keeps_parent_sat_and_places_child_in_output_1() {
        let pk = test_pubkey();
        let addr = Address::p2tr_tweaked(
            bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            Network::Testnet,
        );
        let parent_id = "abcdef0000000000000000000000000000000000000000000000000000000001i0";

        for parent_value in [DUST_LIMIT, 10_000] {
            for fee_rate in 1..=50 {
                let parent = Utxo {
                    outpoint: OutPoint::new(
                        Txid::from_str(
                            "abcdef0000000000000000000000000000000000000000000000000000000001",
                        )
                        .unwrap(),
                        0,
                    ),
                    amount: Amount::from_sat(parent_value),
                    script_pubkey: addr.script_pubkey(),
                };
                let plan = build_commit_reveal(
                    &pk,
                    &test_entity(),
                    &[test_utxo(1_000_000)],
                    fee_rate,
                    &addr,
                    Network::Testnet,
                    Some(parent_id),
                    Some(parent),
                )
                .unwrap();

                // Input values in the order the reveal actually spends them.
                let commit_outpoint = OutPoint::new(plan.commit_tx.compute_txid(), 0);
                let commit_value = plan.commit_tx.output[0].value.to_sat();
                let inputs: Vec<u64> = plan
                    .reveal_tx
                    .input
                    .iter()
                    .map(|i| if i.previous_output == commit_outpoint { commit_value } else { parent_value })
                    .collect();
                let start_of = |op: OutPoint| -> u64 {
                    let idx = plan.reveal_tx.input.iter().position(|i| i.previous_output == op).unwrap();
                    inputs[..idx].iter().sum()
                };
                let parent_start = start_of(plan.parent_utxo.as_ref().unwrap().outpoint);
                let commit_start = start_of(commit_outpoint);
                let outputs: Vec<u64> =
                    plan.reveal_tx.output.iter().map(|o| o.value.to_sat()).collect();

                // Every parent sat (inscription at offset 0 or deeper) stays in output 0.
                for parent_offset in [0, parent_value / 2, parent_value - 1] {
                    assert_eq!(
                        sat_destination(&inputs, &outputs, parent_start + parent_offset),
                        Some((0, parent_offset)),
                        "parent sat at offset {parent_offset} must stay in output 0 \
                         (parent {parent_value} sats, {fee_rate} sat/vB)"
                    );
                }
                // The child is inscribed on the first sat of the commit input.
                assert_eq!(
                    sat_destination(&inputs, &outputs, commit_start),
                    Some((1, 0)),
                    "child must land on the first sat of output 1 ({fee_rate} sat/vB)"
                );
                // The fee is paid by the commit's tail, not by the parent.
                let fee = inputs.iter().sum::<u64>() - outputs.iter().sum::<u64>();
                assert_eq!(fee, commit_value - DUST_LIMIT);
            }
        }
    }

    #[test]
    fn reveal_without_parent_keeps_single_input_layout() {
        let pk = test_pubkey();
        let addr = Address::p2tr_tweaked(
            bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            Network::Testnet,
        );
        let plan = build_commit_reveal(
            &pk, &test_entity(), &[test_utxo(200_000)], 2, &addr, Network::Testnet, None, None,
        )
        .unwrap();
        assert_eq!(plan.reveal_tx.input.len(), 1);
        assert_eq!(plan.reveal_tx.input[0].previous_output.txid, plan.commit_tx.compute_txid());
        assert_eq!(plan.commit_input_index(), 0);
        assert_eq!(plan.inscription_vout(), 0);
    }

    #[test]
    fn build_commit_reveal_with_parent_utxo() {
        let pk = test_pubkey();
        let addr = Address::p2tr_tweaked(
            bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            Network::Testnet,
        );
        // A second UTXO that represents the parent inscription sat.
        let parent = Utxo {
            outpoint: OutPoint::new(
                Txid::from_str(
                    "abcdef0000000000000000000000000000000000000000000000000000000001",
                )
                .unwrap(),
                0,
            ),
            // Larger than dust, so a hardcoded 546 in the parent output fails the test.
            amount: Amount::from_sat(10_000),
            script_pubkey: ScriptBuf::new_p2tr_tweaked(
                bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(pk),
            ),
        };
        let parent_id = "abcdef0000000000000000000000000000000000000000000000000000000001i0";

        let plan = build_commit_reveal(
            &pk,
            &test_entity(),
            &[test_utxo(200_000)],
            2,
            &addr,
            Network::Testnet,
            Some(parent_id),
            Some(parent.clone()),
        )
        .unwrap();

        // Reveal must have 2 inputs: parent UTXO first, then the commit output.
        assert_eq!(plan.reveal_tx.input.len(), 2, "reveal must have parent + commit inputs");
        assert_eq!(
            plan.reveal_tx.input[0].previous_output,
            parent.outpoint,
            "input 0 must spend the parent UTXO"
        );
        assert_eq!(
            plan.reveal_tx.input[1].previous_output.txid,
            plan.commit_tx.compute_txid(),
            "input 1 must spend commit output"
        );
        assert_eq!(plan.commit_input_index(), 1);
        assert_eq!(plan.inscription_vout(), 1);

        // Reveal must have 2 outputs: parent sat returned, then the new inscription.
        assert_eq!(plan.reveal_tx.output.len(), 2, "reveal must have parent + inscription outputs");
        assert_eq!(
            plan.reveal_tx.output[0].value,
            parent.amount,
            "output 0 returns the parent UTXO at full value"
        );
        assert_eq!(
            plan.reveal_tx.output[1].value.to_sat(),
            DUST_LIMIT,
            "output 1 is the inscription sat"
        );
        // The inscription output must go to the wallet's key-path address
        // (change_address), NOT to the unique Tapscript commit address. The wallet
        // must be able to sign it as a parent UTXO for a future child inscription.
        assert_eq!(
            plan.reveal_tx.output[1].script_pubkey,
            addr.script_pubkey(),
            "inscription sat must go to the wallet address, not the Tapscript key"
        );

        // Fee estimate must be larger than the no-parent case.
        let plan_no_parent = build_commit_reveal(
            &pk,
            &test_entity(),
            &[test_utxo(200_000)],
            2,
            &addr,
            Network::Testnet,
            None,
            None,
        )
        .unwrap();
        assert!(
            plan.estimated_fee > plan_no_parent.estimated_fee,
            "fee with parent UTXO must be higher"
        );

        // parent_utxo field must be populated on the plan.
        assert!(plan.parent_utxo.is_some());
    }
}