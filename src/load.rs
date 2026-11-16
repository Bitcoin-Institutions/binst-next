//! Loading pipeline for institution trees.
//!
//! A tree fills in stage by stage, and every stage reports its own failure:
//!   1. indexer lookup of the institution  → holder address + body
//!   2. the holder's inscriptions          → children with a validated parent
//!   3. each definition's body
//!   4. Citrea `getTemplateInstances`      → instance addresses
//!   5. each instance's state

use binst_inscription::{parse_binst_body, BinstEntity};
use futures_util::future::join_all;

use crate::citrea;
use crate::fetch::fetch_text;
use crate::indexer::{indexer_address_inscriptions, indexer_content_url, indexer_inscription};
use crate::model::{Definition, Instance, Institution, Provenance, Source, Stage, Step, Tree, UserError};
use crate::state::{local_tree, Msg};
use crate::store::{self, dispatch};

/// Parentless inscriptions checked per wallet when listing "my institutions".
const MAX_CANDIDATES: usize = 40;
/// Instances read per definition.
const MAX_INSTANCES: usize = 24;
const CONTENT_CACHE_PREFIX: &str = "binst_next:v1:content:";

// ── Inscription bodies ───────────────────────────────────────────

/// Fetch an inscription body. Confirmed inscription content never changes,
/// so it is cached in localStorage to spare the indexer quota.
async fn inscription_body(id: &str) -> Result<String, String> {
    let key = format!("{CONTENT_CACHE_PREFIX}{id}");
    let storage = crate::storage::get_local_storage();
    if let Some(cached) = storage.as_ref().and_then(|s| s.get_item(&key).ok().flatten()) {
        return Ok(cached);
    }
    let body = fetch_text(&indexer_content_url(id)).await?;
    // Only cache what parses, so an error page is never remembered.
    if parse_binst_body(&body).is_ok() {
        if let Some(storage) = storage {
            storage.set_item(&key, &body).ok();
        }
    }
    Ok(body)
}

pub(crate) fn institution_from_body(id: &str, body: &str, holder: &str) -> Result<Institution, String> {
    match parse_binst_body(body) {
        Ok(BinstEntity::Institution(inst)) => Ok(Institution {
            id: id.to_string(),
            name: inst.name,
            description: inst.description.unwrap_or_default(),
            website: inst.website.unwrap_or_default(),
            admin: inst.admin,
            holder: holder.to_string(),
            pending: false, height: 0,
        }),
        Ok(BinstEntity::ProcessTemplate(_)) => {
            Err("this inscription is a process definition, not an institution".to_string())
        }
        Ok(_) => Err("this inscription is not an institution".to_string()),
        Err(_) => Err("this inscription is not a BINST entity".to_string()),
    }
}

/// The key-path Taproot address of an x-only key on testnet4. Wallets hold
/// what they inscribe at this address of the admin key.
pub(crate) fn admin_address(admin: &str) -> Option<String> {
    let key = admin.parse::<bitcoin::key::XOnlyPublicKey>().ok()?;
    let secp = bitcoin::secp256k1::Secp256k1::verification_only();
    Some(bitcoin::Address::p2tr(&secp, key, None, bitcoin::Network::Testnet4).to_string())
}

pub(crate) fn definition_from_body(id: &str, body: &str, institution: &str, provenance: Provenance) -> Option<Definition> {
    let Ok(BinstEntity::ProcessTemplate(template)) = parse_binst_body(body) else { return None };
    // The body names its institution; a child that names another one is ignored.
    if template.institution_id != institution {
        return None;
    }
    Some(Definition {
        id: id.to_string(),
        name: template.name,
        description: template.description.unwrap_or_default(),
        steps: template
            .steps
            .into_iter()
            .map(|s| Step { name: s.name, action: s.action_type.unwrap_or_else(|| "approval".to_string()) })
            .collect(),
        provenance,
        instances: Vec::new(),
        instances_stage: Stage::Loading,
    })
}

// ── Stage 1: the institution ─────────────────────────────────────

pub async fn load_tree(id: String) {
    let result = fetch_root(&id).await;
    dispatch(Msg::TreeLoaded(id, result));
}

async fn fetch_root(id: &str) -> Result<Tree, UserError> {
    let located = match indexer_inscription(id).await {
        Ok(located) => located,
        Err(why) => {
            // Broadcast from this browser and not indexed yet.
            if let Some(tree) = store::with(|s| local_tree(&s.registry, id)) {
                return Ok(tree);
            }
            // The bundled copy of the featured institution.
            if let Some(tree) = crate::featured::snapshot(id) {
                return Ok(tree);
            }
            return Err(UserError::new("Loading the institution", why));
        }
    };
    let body = inscription_body(id).await.map_err(|why| UserError::new("Reading the institution's inscription", why))?;
    let mut institution = institution_from_body(id, &body, &located.address)
        .map_err(|why| UserError::new("Reading the institution's inscription", why))?;
    institution.height = located.height.unwrap_or(0);
    Ok(Tree {
        institution,
        definitions: Vec::new(),
        definitions_stage: Stage::Loading,
        source: Source::Live,
        notes: Vec::new(),
    })
}

// ── Stages 2–3: process definitions ──────────────────────────────

pub async fn load_definitions(id: String) {
    let Some((holder, admin)) =
        store::with(|s| s.tree(&id).map(|t| (t.institution.holder.clone(), t.institution.admin.clone())))
    else {
        return;
    };
    let result = fetch_definitions(&id, &holder, &admin).await;
    dispatch(Msg::DefinitionsLoaded(id, result));
}

/// The indexer has no children endpoint, so children are looked for where
/// they usually sit: with the institution's holder, at the admin key's own
/// address (the institution may have moved since), and in the connected wallet.
async fn fetch_definitions(id: &str, holder: &str, admin: &str) -> Result<Vec<Definition>, String> {
    let mut listed = indexer_address_inscriptions(holder).await?;
    let mut searched = vec![holder.to_string()];
    for address in [admin_address(admin).unwrap_or_default(), crate::session::l1_address()] {
        if address.is_empty() || searched.contains(&address) {
            continue;
        }
        if let Ok(more) = indexer_address_inscriptions(&address).await {
            listed.extend(more);
        }
        searched.push(address);
    }

    // A validated parent makes a verified child. A parentless inscription may
    // still name this institution in its body: that is only a declaration.
    let mut wanted: Vec<(String, Provenance)> = Vec::new();
    for item in listed {
        if item.id == id || wanted.iter().any(|(seen, _)| *seen == item.id) {
            continue;
        }
        if item.parents.iter().any(|p| p == id) {
            wanted.push((item.id, Provenance::Verified));
        } else if item.parents.is_empty() && wanted.len() < MAX_CANDIDATES {
            wanted.push((item.id, Provenance::Declared));
        }
    }
    let bodies = join_all(wanted.iter().map(|(child, _)| inscription_body(child))).await;
    let mut definitions: Vec<Definition> = wanted
        .iter()
        .zip(bodies)
        .filter_map(|((child, provenance), body)| definition_from_body(child, &body.ok()?, id, *provenance))
        .collect();
    definitions.sort_by_key(|d| (d.provenance != Provenance::Verified, d.name.to_lowercase()));
    Ok(definitions)
}

// ── Stages 4–5: instances on Citrea ──────────────────────────────

pub async fn load_instances(institution: String, definition: String) {
    let result = fetch_instances(&definition).await;
    dispatch(Msg::InstancesLoaded { institution, definition, result });
}

async fn fetch_instances(definition: &str) -> Result<Vec<Instance>, String> {
    let factory = crate::storage::get_factory_address().ok_or("no factory address for this network")?;
    let mut addresses = citrea::fetch_template_instances(&factory, definition).await?;
    addresses.truncate(MAX_INSTANCES);
    let loaded = join_all(addresses.iter().map(|address| fetch_instance(address))).await;
    Ok(loaded.into_iter().flatten().collect())
}

async fn fetch_instance(address: &str) -> Option<Instance> {
    let total_steps = citrea::get_total_steps(address).await.ok()?;
    let current_step = citrea::get_current_step_index(address).await.ok()?;
    let completed = citrea::is_process_completed(address).await.unwrap_or(false);
    let creator = citrea::get_instance_creator(address).await.unwrap_or_default();
    Some(Instance { address: address.to_lowercase(), creator, current_step, total_steps, completed })
}

// ── "My institutions" ────────────────────────────────────────────

pub async fn load_mine() {
    let (wallet, mut ids) = store::with(|s| {
        (
            s.btc.as_ref().map(|b| b.address.clone()),
            s.registry.institutions.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
        )
    });
    if let Some(address) = wallet {
        let listed = match indexer_address_inscriptions(&address).await {
            Ok(listed) => listed,
            Err(why) => {
                dispatch(Msg::MineLoaded(Err(UserError::new("Listing your wallet's inscriptions", why))));
                return;
            }
        };
        // Institutions have no parent. Definitions are skipped here; they
        // appear under their institution's tree.
        let candidates: Vec<String> =
            listed.into_iter().filter(|i| i.parents.is_empty()).map(|i| i.id).take(MAX_CANDIDATES).collect();
        let bodies = join_all(candidates.iter().map(|id| inscription_body(id))).await;
        for (id, body) in candidates.iter().zip(bodies) {
            let is_institution = matches!(body.map(|b| parse_binst_body(&b)), Ok(Ok(BinstEntity::Institution(_))));
            if is_institution && !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    dispatch(Msg::MineLoaded(Ok(ids)));
}

// ── Instance deep links ──────────────────────────────────────────

/// Find which institution an instance belongs to: instance → template
/// inscription (read from the contract) → validated parent.
pub async fn resolve_instance(address: String) {
    let result = async {
        let template = citrea::get_template_inscription_id(&address)
            .await
            .map_err(|why| UserError::new("Reading the instance on Citrea", why))?;
        if !crate::model::is_inscription_id(&template) {
            return Err(UserError::new("Reading the instance on Citrea", "it does not name a template inscription"));
        }
        let located = indexer_inscription(&template)
            .await
            .map_err(|why| UserError::new("Looking up the instance's process definition", why))?;
        let institution = located
            .parents
            .into_iter()
            .next()
            .ok_or_else(|| UserError::new("Looking up the instance's process definition", "it has no validated parent institution"))?;
        Ok((institution, template))
    }
    .await;
    dispatch(Msg::InstanceResolved { address, result });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
    const DEF: &str = "505eb39ed86053d5568a23bd128cd153142a21de02326e15354cb70059901e40i0";
    const ADMIN: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    fn institution_body() -> String {
        format!(r#"{{"type":"institution","v":0,"name":"Acme <b>","admin":"{ADMIN}","description":"Research"}}"#)
    }

    fn template_body(institution: &str) -> String {
        format!(
            r#"{{"type":"process_template","v":0,"name":"KYC","institution_id":"{institution}","steps":[{{"name":"Review","action_type":"approval"}},{{"name":"Sign"}}]}}"#
        )
    }

    #[test]
    fn institution_body_maps_to_the_model() {
        let inst = institution_from_body(ID, &institution_body(), "tb1pholder").unwrap();
        assert_eq!(inst.name, "Acme <b>");
        assert_eq!(inst.description, "Research");
        assert_eq!(inst.website, "");
        assert_eq!(inst.holder, "tb1pholder");
        assert!(!inst.pending);
    }

    #[test]
    fn a_definition_is_not_an_institution() {
        let err = institution_from_body(DEF, &template_body(ID), "").unwrap_err();
        assert!(err.contains("process definition"));
        assert!(institution_from_body(ID, "<html>", "").unwrap_err().contains("not a BINST entity"));
    }

    #[test]
    fn definition_body_maps_steps_and_defaults_the_action() {
        let def = definition_from_body(DEF, &template_body(ID), ID, Provenance::Verified).unwrap();
        assert_eq!(def.name, "KYC");
        assert_eq!(def.steps.len(), 2);
        assert_eq!(def.steps[1].action, "approval");
        assert_eq!(def.provenance, Provenance::Verified);
        assert_eq!(def.instances_stage, Stage::Loading);
    }

    #[test]
    fn a_child_naming_another_institution_is_ignored() {
        assert!(definition_from_body(DEF, &template_body(DEF), ID, Provenance::Declared).is_none());
        assert!(definition_from_body(DEF, &institution_body(), ID, Provenance::Declared).is_none());
    }

    #[test]
    fn the_admin_key_maps_to_its_key_path_address() {
        // The featured institution's admin key and the address that holds it.
        assert_eq!(
            admin_address("dbb3ac49b27c84b7c65918fc998a30d24b184e87a79bec87c280d54ec4c3587f").as_deref(),
            Some("tb1p0x9489n5mj2zklqdh0wzazehn23tc2ahdflgsm6ws896pvv35k8sdpd82j")
        );
        assert!(admin_address("not a key").is_none());
    }
}
