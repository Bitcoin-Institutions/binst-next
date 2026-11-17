//! Rendering: the state becomes HTML strings, one per page region, and one
//! delegated listener turns clicks back into messages.
//!
//! A region's DOM is replaced only when its HTML changed, so typing in a form
//! or scrolling a tree is not interrupted by unrelated updates.

pub mod common;
pub mod detail;
pub mod handoff;
pub mod icons;
pub mod members;
pub mod pages;
pub mod shell;
pub mod tree;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, Element, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};

use crate::model::{Layer, NodeRef, Step};
use crate::qr::QrFormat;
use crate::state::{AppState, DefineThen, Msg, ToastKind};
use crate::store::{self, dispatch};

fn document() -> Option<Document> {
    web_sys::window().and_then(|w| w.document())
}

/// Replace a region's HTML if it changed since the last render.
fn put(document: &Document, id: &str, html: &str) {
    let Some(region) = document.get_element_by_id(id) else { return };
    let mark = common::fingerprint(html);
    if region.get_attribute("data-h").as_deref() == Some(mark.as_str()) {
        return;
    }
    region.set_inner_html(html);
    region.set_attribute("data-h", &mark).ok();
}

fn sheet(state: &AppState) -> String {
    if let Some(h) = &state.handoff {
        return handoff::dialog(h);
    }
    if let Some(layer) = state.picker {
        let btc: Vec<shell::BtcCandidate> = crate::wallet::detect_all_btc_providers()
            .into_iter()
            .map(|p| shell::BtcCandidate { key: p.window_path.to_string(), name: p.display_name, icon: p.icon_url })
            .collect();
        let evm = crate::connect::evm_candidates();
        let walletconnect = web_sys::window()
            .and_then(|w| js_sys::Reflect::get(&w, &JsValue::from_str("__wc")).ok())
            .is_some_and(|wc| !wc.is_undefined() && !wc.is_null());
        return shell::picker(layer, &btc, &evm, walletconnect, state.import_error.as_deref());
    }
    if let Some(draft) = &state.define {
        return detail::define_form(state, draft);
    }
    state.selected.as_ref().map(|node| detail::detail(state, node)).unwrap_or_default()
}

/// Render every region from the current state.
pub fn render() {
    let Some(document) = document() else { return };
    let (topbar, ops, view, sheet, toasts, has_sheet, title) = store::with(|s| {
        let sheet = sheet(s);
        (shell::topbar(s), shell::ops(s), pages::view(s), sheet.clone(), shell::toasts(s), !sheet.is_empty(), page_title(s))
    });
    put(&document, "topbar", &topbar);
    put(&document, "ops", &ops);
    put(&document, "view", &view);
    put(&document, "sheet", &sheet);
    put(&document, "toasts", &toasts);
    if let Some(body) = document.body() {
        body.class_list().toggle_with_force("has-sheet", has_sheet).ok();
    }
    document.set_title(&title);
}

fn page_title(state: &AppState) -> String {
    use crate::router::Route;
    let page = match &state.route {
        Route::Home => return "BINST — Bitcoin Institutions".to_string(),
        Route::Institutions => "My institutions".to_string(),
        Route::Institution { id, .. } => {
            state.tree(id).map(|t| t.institution.name.clone()).unwrap_or_else(|| "Institution".to_string())
        }
        Route::Members { id } => {
            format!("Members · {}", state.tree(id).map(|t| t.institution.name.clone()).unwrap_or_else(|| "Institution".to_string()))
        }
        Route::Instance { .. } => "Instance".to_string(),
        Route::New => "Create an institution".to_string(),
        Route::Settings => "Settings".to_string(),
        Route::Lab(_) => "Lab".to_string(),
        Route::NotFound(_) => "Not found".to_string(),
    };
    format!("{page} · BINST")
}

// ── Events ───────────────────────────────────────────────────────

fn attr(el: &Element, name: &str) -> String {
    el.get_attribute(name).unwrap_or_default()
}

fn layer_of(el: &Element) -> Layer {
    if attr(el, "data-layer") == "evm" { Layer::Citrea } else { Layer::Bitcoin }
}

fn input_value(document: &Document, id: &str) -> String {
    let Some(el) = document.get_element_by_id(id) else { return String::new() };
    if let Some(input) = el.dyn_ref::<HtmlInputElement>() {
        input.value()
    } else if let Some(area) = el.dyn_ref::<HtmlTextAreaElement>() {
        area.value()
    } else {
        String::new()
    }
}

/// Read the define form's fields as they are on screen.
fn define_fields(document: &Document) -> (String, String, Vec<Step>) {
    let mut steps = Vec::new();
    if let (Ok(names), Ok(actions)) =
        (document.query_selector_all(".d-step-name"), document.query_selector_all(".d-step-action"))
    {
        for index in 0..names.length() {
            let name = names.item(index).and_then(|n| n.dyn_into::<HtmlInputElement>().ok()).map(|i| i.value()).unwrap_or_default();
            let action = actions
                .item(index)
                .and_then(|n| n.dyn_into::<HtmlSelectElement>().ok())
                .map(|s| s.value())
                .unwrap_or_else(|| "approval".to_string());
            steps.push(Step { name, action });
        }
    }
    (input_value(document, "d-name"), input_value(document, "d-desc"), steps)
}

/// The message for a clicked `[data-act]` element, if any.
fn message_for(document: &Document, el: &Element) -> Option<Msg> {
    let inst = attr(el, "data-inst");
    let id = attr(el, "data-id");
    Some(match attr(el, "data-act").as_str() {
        "connect" => Msg::OpenPicker(layer_of(el)),
        "close-picker" => Msg::ClosePicker,
        "pick-btc" => Msg::ConnectBtc { key: attr(el, "data-key"), name: attr(el, "data-name") },
        "pick-evm" => Msg::ConnectEvm { index: attr(el, "data-index").parse().ok()? },
        "pick-wc" => Msg::ConnectWalletConnect,
        "disconnect" => Msg::Disconnect(layer_of(el)),
        "select" => Msg::Select(Some(match attr(el, "data-kind").as_str() {
            "institution" => NodeRef::Institution(inst),
            "definition" => NodeRef::Definition { institution: inst, id },
            "instance" => NodeRef::Instance { institution: inst, address: id },
            _ => return None,
        })),
        "close-detail" => Msg::Select(None),
        "reload-tree" => Msg::ReloadTree(id),
        "reload-mine" => Msg::ReloadMine,
        "view-tree" => Msg::ListView(false),
        "view-list" => Msg::ListView(true),
        "zoom" => Msg::Zoom(attr(el, "data-delta").parse().ok()?),
        "dismiss-toast" => Msg::DismissToast(id.parse().ok()?),
        "copy" => {
            let text = attr(el, "data-text");
            // A link to this app is copied as a full URL.
            let text = if text.starts_with("#/") {
                let base = document.location().and_then(|l| l.href().ok()).unwrap_or_default();
                format!("{}{text}", base.split('#').next().unwrap_or(""))
            } else {
                text
            };
            if let Some(window) = web_sys::window() {
                let _ = window.navigator().clipboard().write_text(&text);
            }
            Msg::Toast(ToastKind::Info, "Copied to the clipboard.".to_string())
        }
        "create-review" => Msg::CreateReview {
            name: input_value(document, "f-name"),
            description: input_value(document, "f-desc"),
            website: input_value(document, "f-web"),
        },
        "create-back" => Msg::CreateBack,
        "create-submit" => Msg::CreateSubmit,
        "create-reset" => Msg::CreateReset,
        "define-open" => Msg::DefineOpen(inst),
        "define-close" => Msg::DefineClose,
        act @ ("define-add" | "define-remove" | "define-submit") => {
            let (name, description, steps) = define_fields(document);
            let then = match act {
                "define-add" => DefineThen::AddStep,
                "define-remove" => DefineThen::RemoveStep(attr(el, "data-index").parse().ok()?),
                _ => DefineThen::Submit,
            };
            Msg::DefineEdit { name, description, steps, then }
        }
        "reload-members" => Msg::ReloadMembers(id),
        "reload-announced" => Msg::ReloadAnnounced,
        "bind-nostr" => Msg::BindNostrKey(inst),
        "define-role" => Msg::DefineRole {
            institution: inst,
            name: input_value(document, "r-name"),
            description: input_value(document, "r-desc"),
        },
        "award-role" => {
            let slug = attr(el, "data-slug");
            let member = input_value(document, &format!("award-{slug}"));
            Msg::AwardRole { institution: inst, slug, member }
        }
        act @ ("revoke-role" | "reinstate-role") => Msg::SetRevoked {
            institution: inst,
            slug: attr(el, "data-slug"),
            member: attr(el, "data-member"),
            revoked: act == "revoke-role",
        },
        "import-account" => Msg::ImportAccount {
            descriptor: input_value(document, "acct-desc"),
            index: input_value(document, "acct-index").trim().parse().unwrap_or(0),
        },
        "handoff-cancel" => Msg::HandoffCancel,
        "handoff-format" => Msg::HandoffFormat(if attr(el, "data-format") == "bbqr" { QrFormat::Bbqr } else { QrFormat::Ur }),
        "handoff-camera" => Msg::HandoffCamera(attr(el, "data-on") == "1"),
        "handoff-paste" => Msg::HandoffSubmit(input_value(document, "handoff-in").into_bytes()),
        "handoff-download" => {
            let (step, b64) = store::with(|s| s.handoff.as_ref().map(|h| (h.step.clone(), h.psbt_b64.clone())))?;
            let bytes = bitcoin::base64::Engine::decode(&bitcoin::base64::engine::general_purpose::STANDARD, &b64).ok()?;
            let name = format!("binst-{step}.psbt");
            crate::handoff::download(&name, &bytes, "application/octet-stream");
            Msg::Toast(ToastKind::Info, format!("Saved {name}. Sign it, then load the signed file here."))
        }
        "save-settings" => Msg::SaveSettings(crate::settings::Settings {
            ord_url: input_value(document, "s-ord"),
            esplora_url: input_value(document, "s-esplora"),
            citrea_rpc: input_value(document, "s-citrea"),
        }),
        "forget-recovery" => {
            if attr(el, "data-revealed") != "1" {
                let ok = web_sys::window()
                    .and_then(|w| w.confirm_with_message("This reveal was never broadcast. Forgetting the key loses the commit output's sats unless you copied the descriptor. Forget it?").ok())
                    .unwrap_or(false);
                if !ok {
                    return None;
                }
            }
            Msg::ForgetRecovery(id)
        }
        "start-instance" => Msg::StartInstance { institution: inst, definition: id },
        "execute-step" => Msg::ExecuteStep { institution: inst, definition: attr(el, "data-def"), address: id },
        _ => return None,
    })
}

fn acted_element(event: &web_sys::Event) -> Option<Element> {
    let target = event.target()?.dyn_into::<Element>().ok()?;
    let el = target.closest("[data-act]").ok()??;
    // A disabled control does nothing.
    (!el.has_attribute("disabled")).then_some(el)
}

/// Attach the delegated listeners. Call once at boot.
pub fn mount() {
    let Some(document) = document() else { return };

    let doc = document.clone();
    let on_click = Closure::wrap(Box::new(move |event: web_sys::Event| {
        if let Some(msg) = acted_element(&event).and_then(|el| message_for(&doc, &el)) {
            dispatch(msg);
        }
    }) as Box<dyn Fn(web_sys::Event)>);
    document.add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref()).ok();
    on_click.forget();

    let doc = document.clone();
    let on_key = Closure::wrap(Box::new(move |event: web_sys::KeyboardEvent| {
        match event.key().as_str() {
            "Escape" => {
                let msg = store::with(|s| {
                    if s.picker.is_some() {
                        Some(Msg::ClosePicker)
                    } else if s.define.as_ref().is_some_and(|d| !d.busy) {
                        Some(Msg::DefineClose)
                    } else if s.selected.is_some() {
                        Some(Msg::Select(None))
                    } else {
                        None
                    }
                });
                if let Some(msg) = msg {
                    dispatch(msg);
                }
            }
            // Tree nodes are SVG groups: give them a button's keyboard behaviour.
            "Enter" | " " => {
                let Some(target) = event.target().and_then(|t| t.dyn_into::<Element>().ok()) else { return };
                if target.tag_name().eq_ignore_ascii_case("g") && target.has_attribute("data-act") {
                    event.prevent_default();
                    if let Some(msg) = message_for(&doc, &target) {
                        dispatch(msg);
                    }
                }
            }
            _ => {}
        }
    }) as Box<dyn Fn(web_sys::KeyboardEvent)>);
    document.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref()).ok();
    on_key.forget();

    // File inputs (the hand-off dialog's "Load a .psbt file").
    let on_change = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let Some(input) = event.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) else { return };
        if input.get_attribute("data-change").as_deref() == Some("handoff-file") {
            crate::handoff::read_file(&input);
            input.set_value("");
        }
    }) as Box<dyn Fn(web_sys::Event)>);
    document.add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref()).ok();
    on_change.forget();

    // Animated QR: advance the visible frame (a no-op when none is shown).
    let tick = Closure::wrap(Box::new(crate::handoff::tick_qr) as Box<dyn Fn()>);
    if let Some(window) = web_sys::window() {
        window
            .set_interval_with_callback_and_timeout_and_arguments_0(tick.as_ref().unchecked_ref(), 400)
            .ok();
    }
    tick.forget();
}
