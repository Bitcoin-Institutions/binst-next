//! The one mutable store.
//!
//! The workspace convention is "no global mutable state". This crate keeps a
//! single documented exception: the application state lives in one
//! `thread_local!`, is changed only through `dispatch`, and every change goes
//! through the pure `state::update`. Nothing else in the new code holds state.

use std::cell::RefCell;

use wasm_bindgen::JsValue;

use crate::state::{update, AppState, Effect, Msg};

thread_local! {
    static STATE: RefCell<AppState> = RefCell::new(AppState::new(crate::featured::FEATURED_ID));
}

/// Read the state.
pub fn with<R>(f: impl FnOnce(&AppState) -> R) -> R {
    STATE.with(|s| f(&s.borrow()))
}

/// Apply a message, re-render, then run the effects it asked for.
pub fn dispatch(msg: Msg) {
    let effects = STATE.with(|s| update(&mut s.borrow_mut(), msg));
    crate::render::render();
    for effect in effects {
        run(effect);
    }
}

fn spawn(future: impl std::future::Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(future);
}

fn run(effect: Effect) {
    match effect {
        Effect::ReplaceHash(hash) => {
            if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
                history.replace_state_with_url(&JsValue::NULL, "", Some(&hash)).ok();
            }
        }
        Effect::LoadTree(id) => spawn(crate::load::load_tree(id)),
        Effect::LoadDefinitions(id) => spawn(crate::load::load_definitions(id)),
        Effect::LoadInstances { institution, definition } => {
            spawn(crate::load::load_instances(institution, definition))
        }
        Effect::LoadMine => spawn(crate::load::load_mine()),
        Effect::ResolveInstance(address) => spawn(crate::load::resolve_instance(address)),
        Effect::ConnectBtc { key, name } => spawn(crate::connect::connect_btc(key, name)),
        Effect::ConnectEvm { index } => spawn(crate::connect::connect_evm(index)),
        Effect::ConnectWalletConnect => spawn(crate::connect::connect_walletconnect()),
        Effect::ForgetEvmProvider => crate::wallet::clear_connected_l2_provider(),
        Effect::SaveSessions => crate::connect::save_sessions(),
        Effect::SaveRegistry => crate::connect::save_registry(),
        Effect::FetchFeeRate => spawn(crate::actions::fetch_fee_rate()),
        Effect::InscribeInstitution { name, description, website, fee_rate } => {
            spawn(crate::actions::inscribe_institution(name, description, website, fee_rate))
        }
        Effect::InscribeDefinition { institution, name, description, steps, fee_rate } => {
            spawn(crate::actions::inscribe_definition(institution, name, description, steps, fee_rate))
        }
        Effect::StartInstance { institution, definition, steps } => {
            spawn(crate::actions::start_instance(institution, definition, steps))
        }
        Effect::ExecuteStep { institution, definition, address, step } => {
            spawn(crate::actions::execute_step(institution, definition, address, step))
        }
        Effect::PollConfirmation { op, txid } => spawn(crate::actions::poll_confirmation(op, txid)),
        Effect::LoadMembers(id) => spawn(crate::members::load(id)),
        Effect::LoadAnnounced => spawn(crate::members::load_announced()),
        Effect::BindNostrKey { institution, name, holder } => spawn(crate::members::bind(institution, name, holder)),
        Effect::PublishRole { institution, issuer, slug, name, description } => {
            spawn(crate::members::publish_role(institution, issuer, slug, name, description))
        }
        Effect::PublishAward { institution, issuer, slug, member } => {
            spawn(crate::members::publish_award(institution, issuer, slug, member))
        }
        Effect::PublishRevocations { institution, issuer, revoked } => {
            spawn(crate::members::publish_revocations(institution, issuer, revoked))
        }
        Effect::ExpireToast(id) => spawn(async move {
            if let Some(window) = web_sys::window() {
                crate::fetch::sleep_ms(&window, 7_000).await;
            }
            dispatch(Msg::DismissToast(id));
        }),
    }
}
