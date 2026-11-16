//! A minimal Nostr relay client: open a WebSocket, send one request, collect
//! the answer, close. No subscriptions are kept open.
//!
//! Relays are untrusted. Everything read here is verified by `nostr.rs`
//! before it is shown.

use js_sys::{Array, Function, Promise};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{MessageEvent, WebSocket};

use crate::nostr::Event;

const QUERY_TIMEOUT_MS: i32 = 6_000;
const PUBLISH_TIMEOUT_MS: i32 = 8_000;
/// Events kept per relay answer, so a hostile relay cannot flood the page.
const MAX_FRAMES: u32 = 500;

/// Relays to use: the defaults, or the list in `localStorage` under
/// `binst_next:v1:relays` (a JSON array of `ws://`/`wss://` URLs), which is
/// how a local relay is used for testing.
pub fn relays() -> Vec<String> {
    let stored = crate::storage::get_local_storage()
        .and_then(|s| s.get_item("binst_next:v1:relays").ok().flatten())
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .map(|list| list.into_iter().filter(|u| u.starts_with("wss://") || u.starts_with("ws://")).take(6).collect::<Vec<_>>())
        .filter(|list| !list.is_empty());
    stored.unwrap_or_else(|| crate::nostr::DEFAULT_RELAYS.iter().map(|r| r.to_string()).collect())
}

/// Send `message` to one relay and resolve with the text frames received up
/// to (and including) the first frame whose type is in `until`, or with what
/// arrived before the timeout, an error or the socket closing.
fn exchange(url: &str, message: String, until: &'static [&'static str], timeout_ms: i32) -> Promise {
    let url = url.to_string();
    Promise::new(&mut move |resolve: Function, _reject: Function| {
        let frames = Array::new();
        let Ok(socket) = WebSocket::new(&url) else {
            resolve.call1(&JsValue::NULL, &frames).ok();
            return;
        };

        // The handlers below are small and live as long as the page; a query
        // happens a few times per visit, so they are leaked rather than tracked.
        let finish = {
            let (socket, frames, resolve) = (socket.clone(), frames.clone(), resolve.clone());
            Closure::<dyn Fn()>::new(move || {
                socket.set_onmessage(None);
                socket.set_onerror(None);
                socket.set_onclose(None);
                socket.close().ok();
                // Resolving twice is harmless: a promise settles once.
                resolve.call1(&JsValue::NULL, &frames).ok();
            })
        };
        let finish_fn: Function = finish.as_ref().unchecked_ref::<Function>().clone();
        finish.forget();

        let on_open = {
            let (socket, message) = (socket.clone(), message.clone());
            Closure::<dyn Fn()>::new(move || {
                socket.send_with_str(&message).ok();
            })
        };
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();

        let on_message = {
            let (frames, finish_fn) = (frames.clone(), finish_fn.clone());
            Closure::<dyn Fn(MessageEvent)>::new(move |event: MessageEvent| {
                let Some(text) = event.data().as_string() else { return };
                let done = until.iter().any(|kind| text.starts_with(&format!("[\"{kind}\"")));
                frames.push(&JsValue::from_str(&text));
                if done || frames.length() >= MAX_FRAMES {
                    finish_fn.call0(&JsValue::NULL).ok();
                }
            })
        };
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();

        socket.set_onerror(Some(&finish_fn));
        socket.set_onclose(Some(&finish_fn));
        if let Some(window) = web_sys::window() {
            window.set_timeout_with_callback_and_timeout_and_arguments_0(&finish_fn, timeout_ms).ok();
        }
    })
}

async fn frames(url: &str, message: String, until: &'static [&'static str], timeout_ms: i32) -> Vec<serde_json::Value> {
    let Ok(result) = JsFuture::from(exchange(url, message, until, timeout_ms)).await else { return Vec::new() };
    Array::from(&result)
        .iter()
        .filter_map(|frame| frame.as_string())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .collect()
}

/// What the relays returned for one question.
pub struct Answer {
    pub events: Vec<Event>,
    pub asked: usize,
    /// Relays that completed the request (sent EOSE).
    pub answered: usize,
}

/// Ask every relay the same filters and merge the events by id.
pub async fn query(relays: &[String], filters: &[serde_json::Value]) -> Answer {
    let mut request = vec![serde_json::json!("REQ"), serde_json::json!("binst")];
    request.extend(filters.iter().cloned());
    let message = serde_json::Value::Array(request).to_string();
    let answers = futures_util::future::join_all(
        relays.iter().map(|url| frames(url, message.clone(), &["EOSE", "CLOSED"], QUERY_TIMEOUT_MS)),
    )
    .await;

    let mut events: Vec<Event> = Vec::new();
    let mut answered = 0;
    for answer in answers {
        if answer.iter().any(|frame| frame.get(0).and_then(|k| k.as_str()) == Some("EOSE")) {
            answered += 1;
        }
        for frame in answer {
            if frame.get(0).and_then(|k| k.as_str()) != Some("EVENT") {
                continue;
            }
            let Some(event) = frame.get(2).and_then(|e| serde_json::from_value::<Event>(e.clone()).ok()) else { continue };
            if !events.iter().any(|seen| seen.id == event.id) {
                events.push(event);
            }
        }
    }
    Answer { events, asked: relays.len(), answered }
}

/// Publish one event. Returns how many relays accepted it and the reasons
/// the others gave.
pub async fn publish(relays: &[String], event: &Event) -> (usize, Vec<String>) {
    let message = serde_json::json!(["EVENT", event]).to_string();
    let answers =
        futures_util::future::join_all(relays.iter().map(|url| frames(url, message.clone(), &["OK"], PUBLISH_TIMEOUT_MS))).await;
    let mut accepted = 0;
    let mut refusals = Vec::new();
    for (url, answer) in relays.iter().zip(answers) {
        let ok = answer.iter().find(|frame| frame.get(0).and_then(|k| k.as_str()) == Some("OK"));
        match ok {
            Some(frame) if frame.get(2).and_then(|b| b.as_bool()) == Some(true) => accepted += 1,
            Some(frame) => refusals.push(format!(
                "{url}: {}",
                frame.get(3).and_then(|m| m.as_str()).unwrap_or("refused").chars().take(120).collect::<String>()
            )),
            None => refusals.push(format!("{url}: no answer")),
        }
    }
    (accepted, refusals)
}
