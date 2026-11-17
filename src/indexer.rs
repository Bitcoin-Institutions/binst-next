// reused-from: binst-pilot-webapp@dda2d1a:src/search.rs (2026-11-16) — adapted: 2026-11-17 the user's own ord server (settings) as a backend; indexer half only (configuration, UniSat parsing, fetches) plus its tests; the search view is not carried over
use wasm_bindgen::JsCast;

// ── Indexer configuration ────────────────────────────────────────

/// Which indexer backend to use for inscription reads.
#[allow(dead_code)]
pub(crate) enum IndexerBackend {
    /// Local ord server (self-hosted or tunneled).
    OrdServer { base_url: &'static str },
    /// UniSat Open API (testnet4). Paths are the upstream ones under `base_url`.
    UniSat { base_url: &'static str },
}

/// Current active indexer — UniSat Open API testnet4, reached through the
/// same-origin proxy `/api/indexer` (Vercel function `api/indexer.js`), which
/// holds the optional API key server-side and caches responses at the edge.
/// Xverse dropped testnet4 in 2026: its hosts now 503 or redirect to signet.
pub(crate) const INDEXER: IndexerBackend = IndexerBackend::UniSat {
    base_url: "/api/indexer",
};

/// Current network label for display.
pub(crate) const NETWORK: &str = "testnet4";

// ── Indexer reads ────────────────────────────────────────────────

/// One inscription as the indexer reports it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IndexedInscription {
    pub id: String,
    /// Address currently holding the inscription.
    pub address: String,
    /// Output currently holding the inscription's sat: (txid, vout).
    pub output: Option<(String, u32)>,
    /// Parents the indexer validated, i.e. whose sat was spent in the reveal.
    /// A declared tag 3 without that spend does not appear here.
    pub parents: Vec<String>,
    /// Block that confirmed the inscription, when the indexer reports it.
    pub height: Option<u64>,
}

const INDEXER_PAGE_SIZE: u64 = 100;
const INDEXER_MAX_PAGES: u64 = 5;

/// UniSat wraps every JSON payload as `{code, msg, data}`; `code` 0 is success.
fn unisat_data(body: &serde_json::Value) -> Result<&serde_json::Value, String> {
    match body.get("code").and_then(|c| c.as_i64()) {
        Some(0) => body
            .get("data")
            .filter(|d| !d.is_null())
            .ok_or_else(|| "indexer returned no data".to_string()),
        _ => Err(format!(
            "indexer error: {}",
            body.get("msg").and_then(|m| m.as_str()).unwrap_or("unknown")
        )),
    }
}

fn parse_unisat_inscription(v: &serde_json::Value) -> Option<IndexedInscription> {
    let id = v.get("inscriptionId")?.as_str()?.to_string();
    let address = v.get("address").and_then(|a| a.as_str()).unwrap_or("").to_string();
    let output = v.get("utxo").and_then(|u| {
        let txid = u.get("txid")?.as_str()?;
        let vout = u32::try_from(u.get("vout")?.as_u64()?).ok()?;
        (txid.len() == 64).then(|| (txid.to_string(), vout))
    });
    // Only `parents` (validated). `parent`/`hasParent` also cover a tag 3 whose
    // parent sat was never spent, which proves nothing.
    let parents = v
        .get("parents")
        .and_then(|p| p.as_array())
        .map(|arr| arr.iter().filter_map(|p| p.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let height = v.get("height").and_then(|h| h.as_u64()).filter(|h| *h > 0);
    Some(IndexedInscription { id, address, output, parents, height })
}

/// Parse `GET /v1/indexer/inscription/info/{id}`.
pub(crate) fn parse_unisat_info(body: &serde_json::Value) -> Result<IndexedInscription, String> {
    parse_unisat_inscription(unisat_data(body)?)
        .ok_or_else(|| "indexer returned no inscriptionId".to_string())
}

/// Parse one page of `GET /v1/indexer/address/{addr}/inscription-data`.
/// Returns the page's inscriptions and the address total.
pub(crate) fn parse_unisat_address_page(
    body: &serde_json::Value,
) -> Result<(Vec<IndexedInscription>, u64), String> {
    let data = unisat_data(body)?;
    let items = data
        .get("inscription")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_unisat_inscription).collect())
        .unwrap_or_default();
    let total = data.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
    Ok((items, total))
}

/// GET a JSON indexer URL; non-2xx becomes an `Err` carrying the proxy's message.
async fn fetch_indexer_json(url: &str) -> Result<serde_json::Value, String> {
    let window = web_sys::window().ok_or("no window")?;
    let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("indexer fetch: {e:?}"))?;
    let resp: web_sys::Response = resp_val.unchecked_into();
    let text = wasm_bindgen_futures::JsFuture::from(resp.text().map_err(|_| "text()")?)
        .await
        .map_err(|e| format!("indexer text: {e:?}"))?
        .as_string()
        .unwrap_or_default();
    if !resp.ok() {
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_default();
        return Err(format!("indexer HTTP {} {}", resp.status(), msg).trim_end().to_string());
    }
    serde_json::from_str(&text).map_err(|e| format!("indexer JSON: {e}"))
}

// ── The user's own ord server ────────────────────────────────────

/// Parse ord's `GET /inscription/<id>` JSON.
pub(crate) fn parse_ord_inscription(v: &serde_json::Value) -> Result<IndexedInscription, String> {
    let id = v.get("id").and_then(|i| i.as_str()).ok_or("ord returned no inscription id")?.to_string();
    let address = v.get("address").and_then(|a| a.as_str()).unwrap_or("").to_string();
    // satpoint is "<txid>:<vout>:<offset>".
    let output = v.get("satpoint").and_then(|s| s.as_str()).and_then(|sp| {
        let mut parts = sp.split(':');
        let txid = parts.next()?;
        let vout = parts.next()?.parse().ok()?;
        (txid.len() == 64).then(|| (txid.to_string(), vout))
    });
    // ord lists only parents whose sat was spent in the reveal.
    let parents = v
        .get("parents")
        .and_then(|p| p.as_array())
        .map(|arr| arr.iter().filter_map(|p| p.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let height = v.get("height").and_then(|h| h.as_u64()).filter(|h| *h > 0);
    Ok(IndexedInscription { id, address, output, parents, height })
}

/// Parse ord's `GET /address/<address>` JSON (needs `--index-addresses`).
pub(crate) fn parse_ord_address(v: &serde_json::Value) -> Vec<String> {
    v.get("inscriptions")
        .and_then(|i| i.as_array())
        .map(|arr| arr.iter().filter_map(|i| i.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// GET JSON from the user's ord server (ord answers JSON to this Accept header).
async fn fetch_ord_json(url: &str) -> Result<serde_json::Value, String> {
    let opts = web_sys::RequestInit::new();
    opts.set_method("GET");
    let req = web_sys::Request::new_with_str_and_init(url, &opts).map_err(|e| format!("ord request: {e:?}"))?;
    req.headers().set("Accept", "application/json").ok();
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&req))
        .await
        .map_err(|_| format!("Your ord server did not answer ({url}). Is it running, and reachable from this page?"))?
        .unchecked_into();
    let text = wasm_bindgen_futures::JsFuture::from(resp.text().map_err(|_| "text()")?)
        .await
        .map_err(|e| format!("ord text: {e:?}"))?
        .as_string()
        .unwrap_or_default();
    if !resp.ok() {
        let hint = if resp.status() == 404 && url.contains("/address/") {
            " (run ord with --index-addresses)"
        } else {
            ""
        };
        return Err(format!("ord HTTP {}{hint}: {}", resp.status(), text.chars().take(120).collect::<String>()));
    }
    serde_json::from_str(&text).map_err(|e| format!("ord JSON: {e}"))
}

/// Look up one inscription: holder, current sat output and validated parents.
pub(crate) async fn indexer_inscription(id: &str) -> Result<IndexedInscription, String> {
    if let Some(ord) = crate::settings::ord_base() {
        return parse_ord_inscription(&fetch_ord_json(&format!("{ord}/inscription/{id}")).await?);
    }
    let IndexerBackend::UniSat { base_url } = &INDEXER else {
        return Err("inscription lookup needs the UniSat backend".into());
    };
    let body = fetch_indexer_json(&format!("{base_url}/v1/indexer/inscription/info/{id}")).await?;
    parse_unisat_info(&body)
}

/// All inscriptions currently held by `address` (up to
/// `INDEXER_PAGE_SIZE * INDEXER_MAX_PAGES`).
pub(crate) async fn indexer_address_inscriptions(
    address: &str,
) -> Result<Vec<IndexedInscription>, String> {
    if let Some(ord) = crate::settings::ord_base() {
        let ids = parse_ord_address(&fetch_ord_json(&format!("{ord}/address/{address}")).await?);
        let mut all = Vec::new();
        for id in ids.iter().take((INDEXER_PAGE_SIZE * INDEXER_MAX_PAGES) as usize) {
            all.push(parse_ord_inscription(&fetch_ord_json(&format!("{ord}/inscription/{id}")).await?)?);
        }
        return Ok(all);
    }
    let IndexerBackend::UniSat { base_url } = &INDEXER else {
        return Err("address listing needs the UniSat backend".into());
    };
    let mut all = Vec::new();
    for page in 0..INDEXER_MAX_PAGES {
        let url = format!(
            "{base_url}/v1/indexer/address/{address}/inscription-data?cursor={}&size={INDEXER_PAGE_SIZE}",
            page * INDEXER_PAGE_SIZE
        );
        let (items, total) = parse_unisat_address_page(&fetch_indexer_json(&url).await?)?;
        let n = items.len();
        all.extend(items);
        if n == 0 || all.len() as u64 >= total {
            break;
        }
    }
    Ok(all)
}

/// URL of an inscription's raw content.
pub(crate) fn indexer_content_url(id: &str) -> String {
    if let Some(ord) = crate::settings::ord_base() {
        return format!("{ord}/content/{id}");
    }
    match &INDEXER {
        IndexerBackend::UniSat { base_url } => {
            format!("{base_url}/v1/indexer/inscription/content/{id}")
        }
        IndexerBackend::OrdServer { base_url } => format!("{base_url}/content/{id}"),
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const PARENT: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
    const CHILD: &str = "505eb39ed86053d5568a23bd128cd153142a21de02326e15354cb70059901e40i0";

    #[test]
    fn unisat_info_reads_holder_output_and_parents() {
        let body = serde_json::json!({
            "code": 0, "msg": "ok",
            "data": {
                "inscriptionId": CHILD,
                "address": "tb1p0x9489n5mj2zklqdh0wzazehn23tc2ahdflgsm6ws896pvv35k8sdpd82j",
                "hasParent": true,
                "parents": [PARENT],
                "utxo": {
                    "txid": "0280ede100000000000000000000000000000000000000000000000000007243",
                    "vout": 0, "satoshi": 546
                }
            }
        });
        let ins = parse_unisat_info(&body).unwrap();
        assert_eq!(ins.id, CHILD);
        assert_eq!(ins.parents, vec![PARENT.to_string()]);
        assert_eq!(ins.output.as_ref().map(|(_, v)| *v), Some(0));
        assert!(ins.address.starts_with("tb1p0x9489"));
    }

    #[test]
    fn unisat_has_parent_without_parents_is_not_a_parent() {
        // hasParent/parent come from the declared tag 3; only `parents` means
        // the parent sat was spent in the reveal.
        let body = serde_json::json!({
            "code": 0,
            "data": { "inscriptionId": CHILD, "hasParent": true, "parent": PARENT, "parents": null }
        });
        assert!(parse_unisat_info(&body).unwrap().parents.is_empty());
    }

    #[test]
    fn unisat_error_codes_are_errors() {
        let quota = serde_json::json!({
            "code": -2004, "data": null,
            "msg": "exceeded the request limit for unauthenticated requests"
        });
        assert!(parse_unisat_info(&quota).unwrap_err().contains("request limit"));
        let empty = serde_json::json!({ "code": 0, "msg": "ok", "data": null });
        assert!(parse_unisat_info(&empty).is_err());
    }

    #[test]
    fn unisat_bad_utxo_gives_no_output() {
        let body = serde_json::json!({
            "code": 0,
            "data": { "inscriptionId": CHILD, "utxo": { "txid": "short", "vout": 1 } }
        });
        assert_eq!(parse_unisat_info(&body).unwrap().output, None);
    }

    #[test]
    fn unisat_address_page_lists_ids_and_total() {
        let body = serde_json::json!({
            "code": 0,
            "data": {
                "cursor": 0, "total": 11,
                "inscription": [
                    { "inscriptionId": PARENT },
                    { "inscriptionId": CHILD, "parents": [PARENT] },
                    { "noId": true }
                ]
            }
        });
        let (items, total) = parse_unisat_address_page(&body).unwrap();
        assert_eq!(total, 11);
        assert_eq!(items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec![PARENT, CHILD]);
        assert_eq!(items[1].parents, vec![PARENT.to_string()]);
    }

    #[test]
    fn ord_inscription_and_address_parse() {
        // Shapes from ord 0.27.1 (regtest, --index-addresses).
        let child = serde_json::json!({
            "address": "bcrt1p9flm5zs8z2rnjplvly0mdtn3jqxuqentsqf7wfn2d8vzf3nea0lqsm64fj",
            "height": 153, "id": CHILD, "metaprotocol": "binst",
            "parents": [PARENT],
            "satpoint": "5863206b9078f3b8c5509a20e3d58373eb6883bd199e9cfa514bb98a5ff0c8d3:1:0",
            "value": 546
        });
        let ins = parse_ord_inscription(&child).unwrap();
        assert_eq!(ins.parents, vec![PARENT.to_string()]);
        assert_eq!(ins.output, Some(("5863206b9078f3b8c5509a20e3d58373eb6883bd199e9cfa514bb98a5ff0c8d3".to_string(), 1)));
        assert_eq!(ins.height, Some(153));
        let addr = serde_json::json!({ "outputs": ["ab:1"], "inscriptions": [PARENT, CHILD], "sat_balance": 1 });
        assert_eq!(parse_ord_address(&addr), vec![PARENT.to_string(), CHILD.to_string()]);
        assert!(parse_ord_inscription(&serde_json::json!({})).is_err());
    }

    #[test]
    fn content_url_goes_through_the_proxy() {
        assert_eq!(
            indexer_content_url(PARENT),
            format!("/api/indexer/v1/indexer/inscription/content/{PARENT}")
        );
    }
}
