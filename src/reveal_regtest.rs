//! End-to-end check on regtest, with Bitcoin Core as the user's wallet.
//!
//! Ignored by default. It needs a regtest bitcoind with a funded descriptor
//! wallet named `miner`, and an ord server with `--index-addresses`:
//!
//! ```text
//! BINST_REGTEST_CLI="bitcoin-cli -datadir=/path/to/regtest" \
//! BINST_REGTEST_ORD=http://127.0.0.1:8099 \
//!   cargo test --lib reveal_regtest -- --ignored --nocapture
//! ```
//!
//! It creates an institution and a child definition the way the page does with
//! a "sign elsewhere" wallet: the page builds PSBTs and signs the reveal leaf
//! with the one-time key; Core's `walletprocesspsbt` signs every key-path input;
//! then ord must report the child with its validated parent.

use std::process::Command;

use binst_inscription::{BinstEntity, InstitutionBody, ProcessTemplateBody, StepDef};
use bitcoin::key::Keypair;
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use bitcoin::{Address, Amount, Network, OutPoint};
use serde_json::Value;

use crate::reveal::*;
use crate::txbuilder::{build_commit_reveal, psbt_to_base64, Utxo};

fn cli(args: &[&str]) -> String {
    let base = std::env::var("BINST_REGTEST_CLI").expect("set BINST_REGTEST_CLI");
    let mut parts = base.split_whitespace();
    let out = Command::new(parts.next().unwrap()).args(parts).args(args).output().expect("bitcoin-cli");
    assert!(out.status.success(), "bitcoin-cli {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn json(args: &[&str]) -> Value {
    serde_json::from_str(&cli(args)).unwrap()
}

fn ord(path: &str) -> Option<Value> {
    let base = std::env::var("BINST_REGTEST_ORD").expect("set BINST_REGTEST_ORD");
    let out = Command::new("curl").args(["-s", "-f", "-H", "Accept: application/json", &format!("{base}{path}")]).output().ok()?;
    out.status.success().then(|| serde_json::from_slice(&out.stdout).ok()).flatten()
}

fn mine(n: u32) {
    let to = cli(&["-rpcwallet=miner", "getnewaddress", "", "bech32m"]);
    cli(&["generatetoaddress", &n.to_string(), &to]);
    let height: u64 = cli(&["getblockcount"]).parse().unwrap();
    for _ in 0..60 {
        if ord("/blockheight").and_then(|v| v.as_u64()).is_some_and(|h| h >= height) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    panic!("ord did not reach height {height}");
}

/// Sign with Core's wallet the way a user would paste it into Bitcoin Core.
fn core_sign(wallet: &str, psbt: &bitcoin::psbt::Psbt) -> bitcoin::psbt::Psbt {
    let b64 = psbt_to_base64(psbt);
    let signed = json(&[&format!("-rpcwallet={wallet}"), "walletprocesspsbt", &b64, "true", "DEFAULT", "true", "false"]);
    let mut merged = merge_signed(psbt, signed["psbt"].as_str().unwrap().as_bytes()).expect("merge");
    finalize_key_path_inputs(&mut merged);
    verify_key_path_signatures(&merged).expect("Core's signatures verify");
    merged
}

fn broadcast(psbt: &bitcoin::psbt::Psbt) -> String {
    let tx = extract(psbt).expect("all inputs signed");
    let hex: String = bitcoin::consensus::serialize(&tx).iter().map(|b| format!("{b:02x}")).collect();
    cli(&["sendrawtransaction", &hex])
}

fn utxos(wallet: &str, address: &Address) -> Vec<Utxo> {
    let list = json(&[&format!("-rpcwallet={wallet}"), "listunspent", "0", "9999", &format!("[\"{address}\"]")]);
    list.as_array()
        .unwrap()
        .iter()
        .filter(|u| (u["amount"].as_f64().unwrap() * 1e8).round() as u64 > 546)
        .map(|u| Utxo {
            outpoint: OutPoint::new(u["txid"].as_str().unwrap().parse().unwrap(), u["vout"].as_u64().unwrap() as u32),
            amount: Amount::from_sat((u["amount"].as_f64().unwrap() * 1e8).round() as u64),
            script_pubkey: address.script_pubkey(),
        })
        .collect()
}

fn one_time_key(seed: u8) -> (SecretKey, bitcoin::key::XOnlyPublicKey) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let mut bytes = [seed; 32];
    bytes[..16].copy_from_slice(&nanos.to_le_bytes());
    let secret = SecretKey::from_slice(&bytes).unwrap();
    let (xonly, _) = Keypair::from_secret_key(&Secp256k1::new(), &secret).x_only_public_key();
    (secret, xonly)
}

#[test]
#[ignore]
fn reveal_regtest_core_signs_institution_and_child() {
    let wallet = format!("binst{}", std::process::id());
    cli(&["createwallet", &wallet]);
    let address_str = cli(&[&format!("-rpcwallet={wallet}"), "getnewaddress", "", "bech32m"]);
    let info = json(&[&format!("-rpcwallet={wallet}"), "getaddressinfo", &address_str]);
    let address: Address = address_str.parse::<Address<_>>().unwrap().assume_checked();
    let internal: bitcoin::key::XOnlyPublicKey = {
        // desc is tr([fp/86h/1h/0h/0/0]<xonly>)#cs
        let desc = info["desc"].as_str().unwrap();
        desc[desc.find(']').unwrap() + 1..desc.find(')').unwrap()].parse().unwrap()
    };
    let fingerprint = info["hdmasterfingerprint"].as_str().unwrap().parse().unwrap();
    let path = info["hdkeypath"].as_str().unwrap().replace('h', "'").parse().unwrap();
    let me = WalletKey { internal, origin: Some((fingerprint, path)), script_pubkey: address.script_pubkey() };
    println!("user wallet {wallet}, address {address}");

    cli(&["-rpcwallet=miner", "sendtoaddress", &address_str, "0.002"]);
    cli(&["-rpcwallet=miner", "sendtoaddress", &address_str, "0.002"]);
    mine(1);

    // ── Institution (root): Core signs the commit; the reveal is all ours. ──
    let (secret, reveal_key) = one_time_key(1);
    let institution = BinstEntity::Institution(InstitutionBody {
        v: 0,
        name: "Regtest institution".into(),
        admin: internal.to_string(),
        citrea_contract: None,
        membership_rune: None,
        description: Some("signed by Bitcoin Core".into()),
        website: None,
        vault: None,
    });
    let plan = build_commit_reveal(&reveal_key, &institution, &utxos(&wallet, &address)[..1], 2, &address, Network::Regtest, None, None).unwrap();
    let recovery = recovery_descriptor(&plan, &secret);
    let core_cs = json(&["getdescriptorinfo", &recovery[..recovery.find('#').unwrap()]]);
    assert_eq!(core_cs["checksum"].as_str().unwrap(), &recovery[recovery.find('#').unwrap() + 1..], "checksum agrees with Core");
    let commit_txid = broadcast(&core_sign(&wallet, &commit_psbt(&plan, &me).unwrap()));
    let mut reveal = reveal_psbt(&plan, &me).unwrap();
    sign_reveal_input(&mut reveal, &plan, &secret).unwrap();
    let reveal_txid = broadcast(&reveal);
    mine(1);
    let institution_id = format!("{reveal_txid}i0");
    println!("institution {institution_id} (commit {commit_txid})");
    let ins = ord(&format!("/inscription/{institution_id}")).expect("ord knows the institution");
    assert_eq!(ins["address"].as_str(), Some(address_str.as_str()));
    assert_eq!(ins["metaprotocol"].as_str(), Some("binst"));
    let body = Command::new("curl").args(["-s", &format!("{}/content/{institution_id}", std::env::var("BINST_REGTEST_ORD").unwrap())]).output().unwrap().stdout;
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["admin"].as_str(), Some(internal.to_string().as_str()));

    // ── Definition (child): Core signs the commit and the parent input. ──
    let satpoint = ins["satpoint"].as_str().unwrap();
    let mut parts = satpoint.split(':');
    let parent_utxo = Utxo {
        outpoint: OutPoint::new(parts.next().unwrap().parse().unwrap(), parts.next().unwrap().parse().unwrap()),
        amount: Amount::from_sat(ins["value"].as_u64().unwrap()),
        script_pubkey: address.script_pubkey(),
    };
    let (secret, reveal_key) = one_time_key(2);
    let definition = BinstEntity::ProcessTemplate(ProcessTemplateBody {
        v: 0,
        name: "Regtest definition".into(),
        institution_id: institution_id.clone(),
        description: None,
        citrea_contract: None,
        steps: vec![StepDef { name: "Approve".into(), action_type: Some("approval".into()) }],
    });
    let funding = utxos(&wallet, &address);
    let plan = build_commit_reveal(&reveal_key, &definition, &funding, 2, &address, Network::Regtest, Some(&institution_id), Some(parent_utxo)).unwrap();
    broadcast(&core_sign(&wallet, &commit_psbt(&plan, &me).unwrap()));
    let mut reveal = reveal_psbt(&plan, &me).unwrap();
    sign_reveal_input(&mut reveal, &plan, &secret).unwrap();
    assert_eq!(unsigned_inputs(&reveal), vec![0], "only the parent input is left for the wallet");
    let reveal = core_sign(&wallet, &reveal);
    let child_txid = broadcast(&reveal);
    mine(1);
    let child_id = format!("{child_txid}i0");
    let child = ord(&format!("/inscription/{child_id}")).expect("ord knows the child");
    println!("child {child_id}: parents {}", child["parents"]);
    assert_eq!(child["parents"], serde_json::json!([institution_id]));
    let parent_now = ord(&format!("/inscription/{institution_id}")).unwrap();
    assert_eq!(parent_now["address"].as_str(), Some(address_str.as_str()), "the parent sat came back to the holder");
    assert_eq!(parent_now["children"], serde_json::json!([child_id]));
}

#[test]
#[ignore]
fn reveal_regtest_recovery_descriptor_spends_a_stranded_commit() {
    let wallet = format!("binstrec{}", std::process::id());
    cli(&["createwallet", &wallet]);
    let address: Address = cli(&[&format!("-rpcwallet={wallet}"), "getnewaddress", "", "bech32m"]).parse::<Address<_>>().unwrap().assume_checked();
    let info = json(&[&format!("-rpcwallet={wallet}"), "getaddressinfo", &address.to_string()]);
    let desc = info["desc"].as_str().unwrap();
    let internal = desc[desc.find(']').unwrap() + 1..desc.find(')').unwrap()].parse().unwrap();
    let me = WalletKey { internal, origin: None, script_pubkey: address.script_pubkey() };
    cli(&["-rpcwallet=miner", "sendtoaddress", &address.to_string(), "0.001"]);
    mine(1);
    let (secret, reveal_key) = one_time_key(3);
    let entity = BinstEntity::Institution(InstitutionBody {
        v: 0, name: "Stranded".into(), admin: reveal_key.to_string(), citrea_contract: None,
        membership_rune: None, description: None, website: None, vault: None,
    });
    let plan = build_commit_reveal(&reveal_key, &entity, &utxos(&wallet, &address), 2, &address, Network::Regtest, None, None).unwrap();
    let commit_txid = broadcast(&core_sign(&wallet, &commit_psbt(&plan, &me).unwrap()));
    mine(1);
    // The reveal never happens. Import the recovery descriptor into a fresh wallet.
    let rescue = format!("{wallet}rescue");
    cli(&["-named", "createwallet", &format!("wallet_name={rescue}"), "blank=true"]);
    let req = serde_json::json!([{ "desc": recovery_descriptor(&plan, &secret), "timestamp": 0 }]).to_string();
    let res = json(&[&format!("-rpcwallet={rescue}"), "importdescriptors", &req]);
    assert_eq!(res[0]["success"], serde_json::json!(true), "{res}");
    let unspent = json(&[&format!("-rpcwallet={rescue}"), "listunspent"]);
    assert_eq!(unspent[0]["txid"].as_str(), Some(commit_txid.as_str()));
    let to = cli(&["-rpcwallet=miner", "getnewaddress", "", "bech32m"]);
    let sent = json(&[&format!("-rpcwallet={rescue}"), "sendall", &format!("[\"{to}\"]"), "null", "unset", "2"]);
    assert_eq!(sent["complete"], serde_json::json!(true));
    println!("recovered commit {commit_txid} in {}", sent["txid"]);
}
