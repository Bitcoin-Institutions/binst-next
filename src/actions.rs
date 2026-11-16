//! Things that sign or broadcast: inscribing on testnet4, transactions on
//! Citrea testnet, and following them until they settle.

use binst_decoder::vault::wasm::derive_admin_vault_address;
use binst_inscription::{BinstEntity, InstitutionBody, ProcessTemplateBody, StepDef};

use crate::citrea;
use crate::connect::friendly;
use crate::fetch::{fetch_json, fetch_text, sleep_ms};
use crate::inscribe::{fetch_inscription_sat_utxo, inscribe_serverless};
use crate::model::Step;
use crate::state::{LocalDefinition, LocalInstitution, Msg, OpStage};
use crate::store::dispatch;
use crate::wallet::get_connected_pubkey;

const MEMPOOL_BASE: &str = "https://mempool.space/testnet4/api";
/// Used when the fee estimate cannot be fetched.
const FALLBACK_FEE_RATE: u64 = 2;

pub async fn fetch_fee_rate() {
    let rate = fetch_json(&format!("{MEMPOOL_BASE}/v1/fees/recommended"))
        .await
        .ok()
        .and_then(|v| v.get("halfHourFee").and_then(|f| f.as_u64()))
        .unwrap_or(FALLBACK_FEE_RATE);
    dispatch(Msg::FeeRate(rate));
}

// ── Bitcoin: institution ─────────────────────────────────────────

pub async fn inscribe_institution(name: String, description: String, website: String, fee_rate: u64) {
    let admin = get_connected_pubkey();
    if admin.starts_with('<') {
        dispatch(Msg::CreateFailed("Connect a Bitcoin wallet first.".to_string()));
        return;
    }
    // The vault address is a declaration in the body; the sat itself stays at
    // the wallet address so indexers can list it there.
    let vault = derive_admin_vault_address(&admin, true).ok().filter(|s| !s.is_empty());
    let entity = BinstEntity::Institution(InstitutionBody {
        v: 0,
        name: name.clone(),
        admin: admin.clone(),
        citrea_contract: None,
        membership_rune: None,
        description: (!description.is_empty()).then(|| description.clone()),
        website: (!website.is_empty()).then(|| website.clone()),
        vault,
    });
    dispatch(Msg::CreateProgress("Waiting for your wallet: sign the commit, then the reveal.".to_string()));
    match inscribe_serverless(&entity, fee_rate, None, None).await {
        Ok(result) => dispatch(Msg::CreateDone(LocalInstitution {
            id: result.inscription_id,
            reveal_txid: result.reveal_txid,
            name,
            description,
            website,
            admin,
            holder: crate::session::l1_address(),
        })),
        Err(why) => dispatch(Msg::CreateFailed(friendly(&why))),
    }
}

// ── Bitcoin: process definition ──────────────────────────────────

pub async fn inscribe_definition(institution: String, name: String, description: String, steps: Vec<Step>, fee_rate: u64) {
    // Provenance needs the institution's sat spent in the child's reveal, so
    // without that UTXO nothing is inscribed.
    let parent_utxo = match fetch_inscription_sat_utxo(&institution).await {
        Ok(utxo) => utxo,
        Err(why) => {
            dispatch(Msg::DefineFailed(format!("Could not locate the institution's UTXO: {why}")));
            return;
        }
    };
    let entity = BinstEntity::ProcessTemplate(ProcessTemplateBody {
        v: 0,
        name: name.clone(),
        institution_id: institution.clone(),
        description: (!description.is_empty()).then(|| description.clone()),
        citrea_contract: None,
        steps: steps
            .iter()
            .map(|s| StepDef { name: s.name.clone(), action_type: Some(s.action.clone()) })
            .collect(),
    });
    dispatch(Msg::DefineProgress("Waiting for your wallet: sign the commit, then the reveal.".to_string()));
    match inscribe_serverless(&entity, fee_rate, Some(&institution), Some(parent_utxo)).await {
        Ok(result) => dispatch(Msg::DefineDone(LocalDefinition {
            id: result.inscription_id,
            reveal_txid: result.reveal_txid,
            institution_id: institution,
            name,
            description,
            steps,
        })),
        Err(why) => dispatch(Msg::DefineFailed(friendly(&why))),
    }
}

/// Follow a Bitcoin transaction until it has one confirmation.
pub async fn poll_confirmation(op: u32, txid: String) {
    let Some(window) = web_sys::window() else { return };
    let mut misses = 0;
    loop {
        sleep_ms(&window, 15_000).await;
        // mempool.space answers unknown txids with plain text, not JSON.
        let Ok(text) = fetch_text(&format!("{MEMPOOL_BASE}/tx/{txid}/status")).await else { continue };
        let Ok(status) = serde_json::from_str::<serde_json::Value>(&text) else {
            misses += 1;
            if misses >= 8 {
                dispatch(Msg::OpUpdate { id: op, stage: OpStage::Failed("the transaction is not in the mempool".to_string()) });
                return;
            }
            continue;
        };
        if status.get("confirmed").and_then(|c| c.as_bool()) == Some(true) {
            let height = status.get("block_height").and_then(|h| h.as_u64()).unwrap_or(0) as u32;
            dispatch(Msg::OpUpdate { id: op, stage: OpStage::Confirmed(height) });
            return;
        }
    }
}

// ── Citrea: instances ────────────────────────────────────────────

pub async fn start_instance(institution: String, definition: String, steps: Vec<Step>) {
    let result = async {
        let factory = crate::storage::get_factory_address().ok_or("no factory address for this network")?;
        let names: Vec<String> = steps.iter().map(|s| s.name.clone()).collect();
        let actions: Vec<String> = steps.iter().map(|s| s.action.clone()).collect();
        let (address, tx) = citrea::create_instance(&factory, &definition, &names, &actions).await?;
        crate::storage::save_instance_creation_tx(&address, &tx);
        Ok::<_, String>((address.to_lowercase(), tx))
    }
    .await
    .map_err(|why| friendly(&why));
    dispatch(Msg::InstanceStarted { institution, definition, result });
}

pub async fn execute_step(institution: String, definition: String, address: String, step: u32) {
    let result = async {
        let tx = citrea::execute_step(&address, step).await?;
        crate::storage::save_instance_step_tx(&address, step, &tx);
        // The step counts once the chain reports it; wait for that before
        // reloading, or the tree would still show the old step.
        if let Some(window) = web_sys::window() {
            for _ in 0..20 {
                sleep_ms(&window, 1_500).await;
                let advanced = citrea::get_current_step_index(&address).await.is_ok_and(|current| current > step);
                if advanced || citrea::is_process_completed(&address).await.unwrap_or(false) {
                    break;
                }
            }
        }
        Ok::<_, String>(tx)
    }
    .await
    .map_err(|why| friendly(&why));
    dispatch(Msg::StepExecuted { institution, definition, address, result });
}
