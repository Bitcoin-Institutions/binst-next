//! Reading an institution's members from Nostr relays, and the holder's
//! actions: binding a Nostr key, defining roles, awarding and revoking them.
//!
//! Signing never happens here. The Bitcoin wallet signs the binding message
//! and a NIP-07 signer extension (`window.nostr`) signs the events; this
//! module asks, checks what came back, and publishes.

use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::connect::friendly;
use crate::model::UserError;
use crate::nostr::{self, Event, Template, KIND_APP_DATA, KIND_BADGE_AWARD, KIND_BADGE_DEFINITION};
use crate::relay;
use crate::state::{MembersView, Msg};
use crate::store::{self, dispatch};

fn now() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

// ── Reading ──────────────────────────────────────────────────────

pub async fn load(institution: String) {
    let result = fetch(&institution).await;
    dispatch(Msg::MembersLoaded(institution, result));
}

async fn fetch(institution: &str) -> Result<MembersView, UserError> {
    let holder = store::with(|s| s.tree(institution).map(|t| t.institution.holder.clone()))
        .ok_or_else(|| UserError::new("Reading members", "the institution is not loaded"))?;
    let relays = relay::relays();

    // Anyone can publish an announcement; only one the holder signed counts.
    let announcements = relay::query(
        &relays,
        &[serde_json::json!({ "kinds": [KIND_APP_DATA], "#d": [nostr::announcement_d(institution)], "limit": 50 })],
    )
    .await;
    if announcements.answered == 0 {
        return Err(UserError::new(
            "Asking the Nostr relays",
            format!("none of the {} relays answered", announcements.asked),
        ));
    }
    let (binding, rejected) = nostr::best_binding(&announcements.events, institution, &holder);
    let Some(binding) = binding else {
        return Ok(MembersView { membership: None, rejected, asked: announcements.asked, answered: announcements.answered });
    };

    let published = relay::query(
        &relays,
        &[serde_json::json!({
            "kinds": [KIND_BADGE_DEFINITION, KIND_BADGE_AWARD, KIND_APP_DATA],
            "authors": [binding.issuer],
            "limit": 400,
        })],
    )
    .await;
    Ok(MembersView {
        membership: Some(nostr::membership(binding, institution, &published.events)),
        rejected,
        asked: announcements.asked,
        answered: announcements.answered,
    })
}

/// Institutions announced on the relays, for discovery without the indexer's
/// address listing.
pub async fn load_announced() {
    let answer = relay::query(
        &relay::relays(),
        &[serde_json::json!({ "kinds": [KIND_APP_DATA], "#t": [nostr::DISCOVERY_TAG], "limit": 200 })],
    )
    .await;
    let result = if answer.answered == 0 {
        Err(UserError::new("Asking the Nostr relays", format!("none of the {} relays answered", answer.asked)))
    } else {
        Ok(nostr::announced(&answer.events))
    };
    dispatch(Msg::AnnouncedLoaded(result));
}

// ── Signers ──────────────────────────────────────────────────────

fn js_error(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| Reflect::get(e, &JsValue::from_str("message")).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| "the extension returned an error".to_string())
}

async fn call(object: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, String> {
    let function: Function = Reflect::get(object, &JsValue::from_str(method))
        .ok()
        .and_then(|f| f.dyn_into().ok())
        .ok_or_else(|| format!("{method} is not available"))?;
    let result = match args {
        [] => function.call0(object),
        [a] => function.call1(object, a),
        [a, b] => function.call2(object, a, b),
        _ => return Err("too many arguments".to_string()),
    }
    .map_err(|e| js_error(&e))?;
    match result.dyn_into::<Promise>() {
        Ok(promise) => JsFuture::from(promise).await.map_err(|e| js_error(&e)),
        Err(value) => Ok(value),
    }
}

/// The NIP-07 signer extension, if the visitor has one.
fn nostr_signer() -> Result<JsValue, String> {
    web_sys::window()
        .and_then(|w| Reflect::get(&w, &JsValue::from_str("nostr")).ok())
        .filter(|n| !n.is_undefined() && !n.is_null())
        .ok_or_else(|| "No Nostr signer found. Install a NIP-07 extension (for example nos2x or Alby) and reload.".to_string())
}

async fn signer_pubkey() -> Result<String, String> {
    let key = call(&nostr_signer()?, "getPublicKey", &[]).await?.as_string().unwrap_or_default();
    nostr::parse_pubkey(&key).map_err(|_| "The Nostr signer returned an invalid public key.".to_string())
}

/// Have the NIP-07 signer sign a template, and check what it returned.
async fn sign(template: &Template, expected_author: &str) -> Result<Event, String> {
    let unsigned = serde_json::to_string(template).map_err(|e| e.to_string())?;
    let unsigned = js_sys::JSON::parse(&unsigned).map_err(|e| js_error(&e))?;
    let signed = call(&nostr_signer()?, "signEvent", &[unsigned]).await?;
    let json = js_sys::JSON::stringify(&signed).map_err(|e| js_error(&e))?.as_string().unwrap_or_default();
    let event: Event = serde_json::from_str(&json).map_err(|_| "The Nostr signer returned a malformed event.".to_string())?;
    if event.pubkey != expected_author {
        return Err("Your Nostr signer is using a different key from the one bound to this institution.".to_string());
    }
    if event.kind != template.kind || event.tags != template.tags || event.content != template.content {
        return Err("The Nostr signer changed the event before signing it.".to_string());
    }
    event.verify().map_err(|why| format!("The signed event does not verify: {why}."))?;
    Ok(event)
}

/// Ask the connected Bitcoin wallet for a BIP-322 simple signature.
async fn wallet_bip322(address: &str, message: &str) -> Result<String, String> {
    let key = crate::session::l1_provider_key();
    let provider = crate::wallet::resolve_btc_provider(&key).ok_or("The Bitcoin wallet is no longer available. Reconnect it.")?;
    let message_js = JsValue::from_str(message);
    if crate::wallet::is_sats_connect_provider(&provider) {
        let params = js_sys::Object::new();
        for (k, v) in [("address", address), ("message", message), ("protocol", "BIP322")] {
            Reflect::set(&params, &JsValue::from_str(k), &JsValue::from_str(v)).ok();
        }
        let response = call(&provider, "request", &[JsValue::from_str("signMessage"), params.into()]).await?;
        let result = Reflect::get(&response, &JsValue::from_str("result")).unwrap_or(response);
        return Reflect::get(&result, &JsValue::from_str("signature"))
            .ok()
            .and_then(|s| s.as_string())
            .ok_or_else(|| "The wallet did not return a signature.".to_string());
    }
    call(&provider, "signMessage", &[message_js, JsValue::from_str("bip322-simple")])
        .await?
        .as_string()
        .ok_or_else(|| "The wallet did not return a signature.".to_string())
}

async fn publish(event: &Event) -> Result<usize, String> {
    let (accepted, refusals) = relay::publish(&relay::relays(), event).await;
    if accepted == 0 {
        return Err(format!("No relay accepted the event. {}", refusals.join("; ")));
    }
    Ok(accepted)
}

// ── The holder's actions ─────────────────────────────────────────

/// Bind a Nostr key to the institution: the Bitcoin wallet signs a message
/// naming the key, and the key publishes it as the announcement.
pub async fn bind(institution: String, name: String, holder: String) {
    let result = async {
        let issuer = signer_pubkey().await?;
        let issued = now();
        let message = nostr::binding_message(&institution, &issuer, issued);
        dispatch(Msg::MembersProgress("Waiting for your Bitcoin wallet to sign the binding message…".to_string()));
        let signature = wallet_bip322(&holder, &message).await?;
        // Fail here, with a reason, instead of publishing a binding nobody can verify.
        crate::bip322::verify(&holder, &message, &signature)
            .map_err(|why| format!("The wallet's signature is not a BIP-322 Taproot signature this app can verify ({why})."))?;
        dispatch(Msg::MembersProgress("Waiting for your Nostr signer to sign the announcement…".to_string()));
        let template = nostr::announcement_template(&institution, &name, &holder, &signature, issued);
        let event = sign(&template, &issuer).await?;
        dispatch(Msg::MembersProgress("Publishing to the relays…".to_string()));
        publish(&event).await
    }
    .await;
    finish(institution, result, "Nostr key bound to the institution");
}

pub async fn publish_role(institution: String, issuer: String, slug: String, name: String, description: String) {
    let template = nostr::role_template(&institution, &slug, &name, &description, now());
    let result = sign_and_publish(&template, &issuer).await;
    finish(institution, result, "Role published");
}

pub async fn publish_award(institution: String, issuer: String, slug: String, member: String) {
    let template = nostr::award_template(&institution, &issuer, &slug, &member, now());
    let result = sign_and_publish(&template, &issuer).await;
    finish(institution, result, "Role awarded");
}

pub async fn publish_revocations(institution: String, issuer: String, revoked: Vec<(String, String)>) {
    let template = nostr::revocations_template(&institution, &revoked, now());
    let result = sign_and_publish(&template, &issuer).await;
    finish(institution, result, "Revocation list published");
}

async fn sign_and_publish(template: &Template, issuer: &str) -> Result<usize, String> {
    let event = sign(template, issuer).await?;
    dispatch(Msg::MembersProgress("Publishing to the relays…".to_string()));
    publish(&event).await
}

fn finish(institution: String, result: Result<usize, String>, done: &str) {
    match result {
        Ok(accepted) => dispatch(Msg::MembersDone {
            institution,
            text: format!("{done}. Accepted by {accepted} relay{}.", if accepted == 1 { "" } else { "s" }),
        }),
        Err(why) => dispatch(Msg::MembersFailed(friendly(&why))),
    }
}
