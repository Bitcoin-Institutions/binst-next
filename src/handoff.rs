//! "Sign elsewhere": hand a PSBT to a signer outside the browser and wait.
//!
//! `request` opens the hand-off dialog (render/handoff.rs) and resolves when
//! the user hands back a signed PSBT or transaction that `state::update` has
//! checked against the one sent out. The dialog offers a file download, the
//! base64 to copy, a Bitcoin Core command, and an animated QR (BBQr or UR);
//! the signed result comes back by paste, file or camera.
//!
//! The pending promise's resolve/reject functions live in a `thread_local!`
//! next to the store's: JavaScript functions cannot sit in `AppState`, which
//! is plain data. Only one hand-off is open at a time.

use std::cell::RefCell;

use bitcoin::psbt::Psbt;
use js_sys::{Function, Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::qr::{Collector, Scan};
use crate::state::Msg;
use crate::store::dispatch;
use crate::txbuilder::psbt_to_base64;

thread_local! {
    static PENDING: RefCell<Option<(Function, Function)>> = const { RefCell::new(None) };
    static CAMERA: RefCell<Option<Camera>> = const { RefCell::new(None) };
}

/// Ask the user to sign `psbt` elsewhere; returns what they handed back.
pub async fn request(step: &str, label: &str, psbt: &Psbt) -> Result<Vec<u8>, String> {
    reject(); // a stale dialog from an earlier attempt
    let promise = Promise::new(&mut |resolve, reject| {
        PENDING.with(|p| *p.borrow_mut() = Some((resolve, reject)));
    });
    dispatch(Msg::HandoffOpen {
        step: step.to_string(),
        label: label.to_string(),
        psbt_b64: psbt_to_base64(psbt),
    });
    let value = JsFuture::from(promise).await.map_err(|_| format!("Signing the {step} was cancelled"))?;
    Ok(Uint8Array::new(&value).to_vec())
}

/// Called by the store once `update` accepted the signed bytes.
pub fn resolve(bytes: &[u8]) {
    if let Some((resolve, _)) = PENDING.with(|p| p.borrow_mut().take()) {
        resolve.call1(&JsValue::NULL, &Uint8Array::from(bytes)).ok();
    }
}

pub fn reject() {
    if let Some((_, reject)) = PENDING.with(|p| p.borrow_mut().take()) {
        reject.call0(&JsValue::NULL).ok();
    }
}

/// Offer `bytes` as a file download.
pub fn download(name: &str, bytes: &[u8], mime: &str) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let parts = js_sys::Array::of1(&Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options) else { return };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else { return };
    if let Ok(a) = document.create_element("a") {
        let a: web_sys::HtmlAnchorElement = a.unchecked_into();
        a.set_href(&url);
        a.set_download(name);
        a.click();
    }
    web_sys::Url::revoke_object_url(&url).ok();
}

/// Read a file the user picked and hand it back.
pub fn read_file(input: &web_sys::HtmlInputElement) {
    let Some(file) = input.files().and_then(|f| f.get(0)) else { return };
    wasm_bindgen_futures::spawn_local(async move {
        match JsFuture::from(file.array_buffer()).await {
            Ok(buf) => dispatch(Msg::HandoffSubmit(Uint8Array::new(&buf).to_vec())),
            Err(_) => dispatch(Msg::Toast(crate::state::ToastKind::Error, "Could not read that file".into())),
        }
    });
}

// ── Animated QR ──────────────────────────────────────────────────

/// Show the next frame of the animated QR in `#handoff-qr` (frames are all in
/// the DOM; only one is visible). Runs while the dialog's QR is on screen.
pub fn tick_qr() {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let Ok(frames) = document.query_selector_all("#handoff-qr .qr-frame") else { return };
    let n = frames.length();
    if n < 2 {
        return;
    }
    let mut current = 0;
    for i in 0..n {
        if let Some(el) = frames.item(i).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) {
            if !el.has_attribute("hidden") {
                current = i;
            }
            el.set_attribute("hidden", "").ok();
        }
    }
    let next = (current + 1) % n;
    if let Some(el) = frames.item(next).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) {
        el.remove_attribute("hidden").ok();
    }
    if let Some(counter) = document.get_element_by_id("handoff-qr-count") {
        counter.set_text_content(Some(&format!("frame {} of {n}", next + 1)));
    }
}

// ── Camera ───────────────────────────────────────────────────────

struct Camera {
    stream: web_sys::MediaStream,
    collector: Collector,
    _timer: Closure<dyn FnMut()>,
    timer_id: i32,
}

/// Start the camera into `#handoff-video` and scan frames until a full PSBT
/// is in (QR decoding runs in WASM with `rqrr`, no browser barcode API).
pub fn start_camera() {
    wasm_bindgen_futures::spawn_local(async {
        if let Err(why) = open_camera().await {
            dispatch(Msg::HandoffCamera(false));
            dispatch(Msg::Toast(crate::state::ToastKind::Error, why));
        }
    });
}

async fn open_camera() -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let devices = window.navigator().media_devices().map_err(|_| "This browser gives no camera access")?;
    let constraints = web_sys::MediaStreamConstraints::new();
    let video = js_sys::Object::new();
    js_sys::Reflect::set(&video, &"facingMode".into(), &"environment".into()).ok();
    constraints.set_video(&video);
    let stream: web_sys::MediaStream = JsFuture::from(
        devices.get_user_media_with_constraints(&constraints).map_err(|_| "Camera not available")?,
    )
    .await
    .map_err(|_| "Camera permission was refused")?
    .unchecked_into();
    let element: web_sys::HtmlVideoElement = window
        .document()
        .and_then(|d| d.get_element_by_id("handoff-video"))
        .ok_or("The scanner closed")?
        .unchecked_into();
    element.set_src_object(Some(&stream));
    element.set_muted(true);
    let _ = element.play();

    let timer = Closure::<dyn FnMut()>::new(scan_frame);
    let timer_id = window
        .set_interval_with_callback_and_timeout_and_arguments_0(timer.as_ref().unchecked_ref(), 180)
        .map_err(|_| "Could not start the scanner")?;
    stop_camera();
    CAMERA.with(|c| *c.borrow_mut() = Some(Camera { stream, collector: Collector::default(), _timer: timer, timer_id }));
    set_status("Point the camera at the signer's QR code");
    Ok(())
}

pub fn stop_camera() {
    if let Some(camera) = CAMERA.with(|c| c.borrow_mut().take()) {
        if let Some(window) = web_sys::window() {
            window.clear_interval_with_handle(camera.timer_id);
        }
        for track in camera.stream.get_tracks().iter() {
            track.unchecked_into::<web_sys::MediaStreamTrack>().stop();
        }
    }
}

fn set_status(text: &str) {
    if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("handoff-scan-status")) {
        el.set_text_content(Some(text));
    }
}

fn scan_frame() {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let Some(video) = document.get_element_by_id("handoff-video").and_then(|e| e.dyn_into::<web_sys::HtmlVideoElement>().ok()) else {
        stop_camera();
        return;
    };
    let (w, h) = (video.video_width(), video.video_height());
    if w == 0 || h == 0 {
        return;
    }
    // Downscale large frames: rqrr is quick on ~640px images.
    let scale = (640.0 / w.max(h) as f64).min(1.0);
    let (cw, ch) = (((w as f64) * scale) as u32, ((h as f64) * scale) as u32);
    let Ok(canvas) = document.create_element("canvas") else { return };
    let canvas: web_sys::HtmlCanvasElement = canvas.unchecked_into();
    canvas.set_width(cw);
    canvas.set_height(ch);
    let Some(ctx) = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
    else {
        return;
    };
    if ctx.draw_image_with_html_video_element_and_dw_and_dh(&video, 0.0, 0.0, cw as f64, ch as f64).is_err() {
        return;
    }
    let Ok(image) = ctx.get_image_data(0.0, 0.0, cw as f64, ch as f64) else { return };
    let rgba = image.data();
    let luma = |x: usize, y: usize| {
        let i = (y * cw as usize + x) * 4;
        ((rgba[i] as u32 * 299 + rgba[i + 1] as u32 * 587 + rgba[i + 2] as u32 * 114) / 1000) as u8
    };
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(cw as usize, ch as usize, luma);
    for grid in prepared.detect_grids() {
        let Ok((_, text)) = grid.decode() else { continue };
        let result = CAMERA.with(|c| c.borrow_mut().as_mut().map(|cam| cam.collector.push(&text)));
        match result {
            Some(Scan::Done(bytes)) => {
                stop_camera();
                dispatch(Msg::HandoffCamera(false));
                dispatch(Msg::HandoffSubmit(bytes));
                return;
            }
            Some(Scan::Partial { have, of }) => set_status(&format!("Scanned {have} of {of} parts…")),
            Some(Scan::Ignored(why)) => set_status(&why),
            None => return,
        }
    }
}
