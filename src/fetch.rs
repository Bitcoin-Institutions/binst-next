// reused-from: binst-pilot-webapp@dda2d1a:src/fetch.rs (2026-11-16) — adapted: none (verbatim)
//! HTTP fetch helpers for browser `window.fetch()`.

use wasm_bindgen::prelude::*;

/// Simple GET fetch that returns the response body as a string.
pub async fn fetch_text(url: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or("No window")?;
    let resp_value = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("fetch error: {:?}", e))?;

    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|_| "not a Response".to_string())?;

    let text_promise = resp
        .text()
        .map_err(|e| format!("text() error: {:?}", e))?;
    let text_value = wasm_bindgen_futures::JsFuture::from(text_promise)
        .await
        .map_err(|e| format!("await text error: {:?}", e))?;

    text_value
        .as_string()
        .ok_or_else(|| "body not string".to_string())
}

/// Fetch JSON from a URL via browser fetch and parse to serde_json::Value.
pub async fn fetch_json(url: &str) -> Result<serde_json::Value, String> {
    match fetch_text(url).await {
        Ok(txt) => serde_json::from_str(&txt).map_err(|e| format!("invalid json: {}", e)),
        Err(e) => Err(e),
    }
}

/// Fetch JSON with an Authorization: Bearer header (for Ordiscan).
pub async fn fetch_json_authed(url: &str, bearer: &str) -> Result<serde_json::Value, String> {
    match fetch_text_with_header(url, "Authorization", &format!("Bearer {}", bearer)).await {
        Ok(txt) => serde_json::from_str(&txt).map_err(|e| format!("invalid json: {}", e)),
        Err(e) => Err(e),
    }
}

/// Fetch JSON with an x-api-key header (for Xverse).
/// NOTE: The Xverse REST API is CORS-blocked from browser origins.
/// Kept for potential server-side or proxy use in the future.
#[allow(dead_code)]
pub async fn fetch_json_xverse(url: &str, api_key: &str) -> Result<serde_json::Value, String> {
    match fetch_text_with_header(url, "x-api-key", api_key).await {
        Ok(txt) => serde_json::from_str(&txt).map_err(|e| format!("invalid json: {}", e)),
        Err(e) => Err(e),
    }
}

/// Browser fetch with a custom header.
pub async fn fetch_text_with_header(
    url: &str,
    header_name: &str,
    header_value: &str,
) -> Result<String, String> {
    let window = web_sys::window().ok_or("no window")?;
    let opts = web_sys::RequestInit::new();
    opts.set_method("GET");
    opts.set_mode(web_sys::RequestMode::Cors);
    let headers = web_sys::Headers::new().map_err(|_| "headers")?;
    headers.set("Accept", "application/json").ok();
    headers.set(header_name, header_value).ok();
    opts.set_headers(&headers);
    let request =
        web_sys::Request::new_with_str_and_init(url, &opts).map_err(|_| "request")?;
    let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch: {:?}", e))?;
    let resp: web_sys::Response = resp_val.dyn_into().map_err(|_| "not a Response")?;
    let text = wasm_bindgen_futures::JsFuture::from(resp.text().map_err(|_| "text()")?)
        .await
        .map_err(|e| format!("text: {:?}", e))?;
    text.as_string().ok_or_else(|| "not a string".into())
}

/// Simple async sleep using setTimeout.
pub async fn sleep_ms(window: &web_sys::Window, ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
            .ok();
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
