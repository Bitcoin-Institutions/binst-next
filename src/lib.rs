//! BINST Next — a prototype of a redesigned Bitcoin Institutions workflow.
//!
//! Everything happens on a tree: an institution (an inscription on Bitcoin
//! testnet4), its process definitions (child inscriptions) and their
//! instances (contracts on Citrea testnet).
//!
//! Layout:
//! - `state` / `store` / `router` — typed state, its pure update, hash routes
//! - `load` / `actions` / `connect` — network reads, signing, wallet sessions
//! - `render` — HTML-string views and one delegated event listener
//! - `txbuilder`, `inscribe`, `citrea`, `wallet`, … — logic copied from
//!   binst-pilot-webapp (see PROVENANCE.md)
//!
//! Built with Trunk: `node scripts/dev-proxy.mjs` then `trunk serve`.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

mod actions;
mod bip322;
mod connect;
mod featured;
mod load;
mod members;
mod model;
mod nostr;
mod relay;
mod render;
mod router;
mod session;
mod state;
mod store;
mod tree_layout;

// Copied from binst-pilot-webapp. They keep functions the old views called
// that the new views do not use yet, hence the dead-code allowance.
#[allow(dead_code)]
mod auth;
#[allow(dead_code, deprecated)]
mod citrea;
#[allow(dead_code)]
mod fetch;
#[allow(dead_code)]
mod indexer;
#[allow(dead_code, deprecated)]
mod inscribe;
#[allow(dead_code)]
mod stack;
#[allow(dead_code)]
mod stack_plan;
#[allow(dead_code)]
mod storage;
#[allow(dead_code)]
mod txbuilder;
#[allow(dead_code, unused_variables)]
mod wallet;

use state::Msg;

/// Below this width the tree starts as an outline list.
const NARROW_PX: f64 = 720.0;

fn current_route() -> router::Route {
    let hash = web_sys::window().and_then(|w| w.location().hash().ok()).unwrap_or_default();
    router::parse(&hash)
}

/// Called by Trunk's generated JS glue after WASM loads.
#[wasm_bindgen(start)]
pub fn main() {
    let Some(window) = web_sys::window() else { return };

    wallet::setup_eip6963_listener();
    render::mount();
    connect::restore();

    let narrow = window.inner_width().ok().and_then(|w| w.as_f64()).is_some_and(|w| w < NARROW_PX);
    if narrow {
        store::dispatch(Msg::ListView(true));
    }

    let on_hash = Closure::wrap(Box::new(move || {
        store::dispatch(Msg::Navigate(current_route()));
        if let Some(window) = web_sys::window() {
            window.scroll_to_with_x_and_y(0.0, 0.0);
        }
    }) as Box<dyn Fn()>);
    window.add_event_listener_with_callback("hashchange", on_hash.as_ref().unchecked_ref()).ok();
    on_hash.forget();

    store::dispatch(Msg::Navigate(current_route()));
}
