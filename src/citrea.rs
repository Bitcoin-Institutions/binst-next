// reused-from: binst-pilot-webapp@dda2d1a:src/citrea.rs (2026-11-16) — adapted: 2026-11-17 reads use the Citrea RPC from settings; L2 address read from the session store instead of a hidden input
//! Citrea L2 (EVM) contract interaction layer.
//!
//! All `eth_sendTransaction` / `eth_call` logic lives here, keeping
//! `execute.rs` as pure UI wiring.
//!
//! ## Architecture: Thin L2, Fat L1
//!
//! Bitcoin L1 is the source of truth for identity + definitions (inscriptions).
//! L2 only holds execution state via two contracts:
//!
//! - **BINSTProcessFactory** — thin deployer, creates instances from L1 data
//! - **BINSTProcess** — self-contained instance, carries its own step defs +
//!   `templateInscriptionId` anchor back to Bitcoin L1
//!
//! No Institution or ProcessTemplate contracts on L2 — those live on Bitcoin.
//!
//! ## ABI selectors (from compiled artifacts)
//!
//! | Contract            | Method                                        | Selector     |
//! |---------------------|-----------------------------------------------|--------------|
//! | BINSTProcessFactory | `createInstance(string,string[],string[])`     | `0x6f794b70` |
//! | BINSTProcessFactory | `getInstanceCount()`                          | `0xae34325c` |
//! | BINSTProcessFactory | `getAllInstances()`                           | `0x52c228e3` |
//! | BINSTProcessFactory | `getUserInstances(address)`                   | `0xfceaae17` |
//! | BINSTProcessFactory | `getTemplateInstances(string)`                | `0xb43bed00` |
//! | BINSTProcess        | `executeStep(uint8,string)`                   | `0xf16e3a23` |
//! | BINSTProcess        | `currentStepIndex()`                          | `0x334f45ec` |
//! | BINSTProcess        | `completed()`                                 | `0x9d9a7fe9` |
//! | BINSTProcess        | `totalSteps()`                                | `0x6931b3ae` |
//! | BINSTProcess        | `creator()`                                   | `0x02d05d3f` |
//! | BINSTProcess        | `templateInscriptionId()`                     | `0x0270a0b3` |
//! | BINSTProcess        | `getStepCount()`                              | `0x6fd63351` |
//! | BINSTProcess        | `getStep(uint256)`                            | `0x7874888a` |
//! | BINSTProcess        | `getStepState(uint256)`                       | `0xac5b070a` |
//! | BINSTProcess        | `isCompleted()`                               | `0xfa391c64` |
//!
//! ## Provider access
//!
//! Reads `window.ethereum` (MetaMask / Brave Wallet injection).
//! The connected L2 wallet address is read from the `#wallet-l2-address`
//! DOM element, which is written by `wallet_picker.rs` on connect.

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use js_sys::{Array, Object, Reflect};

// ── Internal helpers ──────────────────────────────────────────────

/// Hardhat account #0 — pre-funded with 10 000 ETH on every Hardhat node.
/// Used as the automatic burner wallet for local development.
const HARDHAT_ACCOUNT_0: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

/// Get the EVM provider the user actually connected through.
/// Uses the stored EIP-6963 provider from `wallet.rs`, falling back
/// to `window.ethereum` only if no explicit connection was made.
fn get_provider() -> Result<JsValue, String> {
    crate::wallet::get_connected_l2_provider()
        .ok_or_else(|| "No EVM provider found — connect MetaMask or Brave Wallet".to_string())
}

/// Read the connected L2 wallet address from the session store.
/// On Hardhat local, auto-returns account #0 if no wallet is connected.
fn get_l2_address() -> Result<String, String> {
    let addr = crate::session::l2_address();
    if !addr.is_empty() {
        return Ok(addr);
    }
    // On Hardhat local, use the built-in burner account — no wallet needed
    if matches!(crate::storage::get_l2_network(), crate::storage::L2Network::HardhatLocal) {
        return Ok(HARDHAT_ACCOUNT_0.to_string());
    }
    Err("L2 wallet not connected — connect MetaMask or Brave Wallet first".to_string())
}

/// Fail unless the wallet is on `expected_hex` (an `eth_chainId` value).
async fn ensure_wallet_chain(provider: &JsValue, expected_hex: &str, name: &str) -> Result<(), String> {
    let request_obj = Object::new();
    Reflect::set(&request_obj, &JsValue::from_str("method"), &JsValue::from_str("eth_chainId"))
        .map_err(|e| format!("js: {e:?}"))?;

    let request_fn: js_sys::Function = Reflect::get(provider, &JsValue::from_str("request"))
        .map_err(|_| "provider.request not found".to_string())?
        .dyn_into()
        .map_err(|_| "provider.request is not a function".to_string())?;
    let promise: js_sys::Promise = request_fn
        .call1(provider, &request_obj)
        .map_err(|e| format!("eth_chainId call failed: {e:?}"))?
        .dyn_into()
        .map_err(|_| "eth_chainId did not return a Promise".to_string())?;
    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("eth_chainId failed: {e:?}"))?;

    let actual = result.as_string().unwrap_or_default();
    if chain_ids_match(&actual, expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "Wallet is on chain {} but BINST expects {name} ({expected_hex}). \
             Switch networks in your wallet and try again.",
            if actual.is_empty() { "unknown" } else { actual.as_str() }
        ))
    }
}

/// Compare two `eth_chainId` hex strings (case and leading zeros ignored).
fn chain_ids_match(a: &str, b: &str) -> bool {
    let parse = |s: &str| u64::from_str_radix(s.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok();
    matches!((parse(a), parse(b)), (Some(x), Some(y)) if x == y)
}

/// Send a state-changing EVM transaction and return the tx hash.
///
/// On Hardhat local: sends directly via HTTP RPC (no wallet popup).
/// On other networks: sends via the connected wallet provider (MetaMask).
async fn send_transaction(from: &str, to: &str, calldata: &str) -> Result<String, String> {
    let l2_net = crate::storage::get_l2_network();

    // Hardhat local: direct HTTP — no wallet needed, no popups
    if matches!(l2_net, crate::storage::L2Network::HardhatLocal) {
        return send_transaction_via_http(l2_net.rpc_url(), from, to, calldata).await;
    }

    // Other networks: use connected wallet provider
    let provider = get_provider()?;

    // The chain is only checked at connect time; the user can switch networks
    // in the wallet afterwards. Re-check right before every send.
    ensure_wallet_chain(&provider, l2_net.chain_id_hex(), l2_net.chain_name()).await?;

    let tx_params = Object::new();
    Reflect::set(&tx_params, &JsValue::from_str("from"), &JsValue::from_str(from))
        .map_err(|e| format!("js: {e:?}"))?;
    Reflect::set(&tx_params, &JsValue::from_str("to"), &JsValue::from_str(to))
        .map_err(|e| format!("js: {e:?}"))?;
    Reflect::set(&tx_params, &JsValue::from_str("data"), &JsValue::from_str(calldata))
        .map_err(|e| format!("js: {e:?}"))?;
    Reflect::set(&tx_params, &JsValue::from_str("value"), &JsValue::from_str("0x0"))
        .map_err(|e| format!("js: {e:?}"))?;

    let params = Array::new();
    params.push(&tx_params);

    let request_obj = Object::new();
    Reflect::set(&request_obj, &JsValue::from_str("method"),
        &JsValue::from_str("eth_sendTransaction"))
        .map_err(|e| format!("js: {e:?}"))?;
    Reflect::set(&request_obj, &JsValue::from_str("params"), &params)
        .map_err(|e| format!("js: {e:?}"))?;

    let request_fn = Reflect::get(&provider, &JsValue::from_str("request"))
        .map_err(|_| "provider.request not found".to_string())?;
    let request_fn: js_sys::Function = request_fn
        .dyn_into()
        .map_err(|_| "provider.request is not a function".to_string())?;

    let promise = request_fn
        .call1(&provider, &request_obj)
        .map_err(|e| format!("provider.request call failed: {e:?}"))?;
    let promise: js_sys::Promise = promise
        .dyn_into()
        .map_err(|_| "provider.request did not return a Promise".to_string())?;

    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| {
            let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
            format!("Wallet rejected transaction: {msg}")
        })?;

    Ok(result.as_string().unwrap_or_else(|| format!("{result:?}")))
}

/// Fetch a transaction receipt via `eth_getTransactionReceipt`.
///
/// Always uses direct HTTP to the L2 RPC endpoint — receipts are
/// read-only data and don't need a wallet signature.
///
/// Returns the full receipt as a `JsValue` (Object), or an error.
async fn get_transaction_receipt(tx_hash: &str) -> Result<JsValue, String> {
    let l2_net = crate::storage::get_l2_network();
    get_receipt_via_http(&crate::settings::citrea_rpc(l2_net), tx_hash).await
}

/// Parse the `InstanceCreated` event from a tx receipt to extract the instance address.
///
/// Event: `InstanceCreated(address indexed instance, address indexed creator, string templateInscriptionId, uint256 stepCount)`
/// Topic0: `0xa3de24f67beb38e771bb8aca2ec9e03485fedd4f4c7379d840ecc7cdd7f0700f`
/// Topic1: instance address (indexed)
/// Topic2: creator address (indexed)
fn parse_instance_address_from_receipt(receipt: &JsValue) -> Result<String, String> {
    let logs = Reflect::get(receipt, &JsValue::from_str("logs"))
        .map_err(|_| "receipt.logs not found".to_string())?;
    let logs: &Array = logs.dyn_ref::<Array>()
        .ok_or("receipt.logs is not an array")?;

    let event_topic = "0xa3de24f67beb38e771bb8aca2ec9e03485fedd4f4c7379d840ecc7cdd7f0700f";

    for i in 0..logs.length() {
        let log = logs.get(i);
        let topics = match Reflect::get(&log, &JsValue::from_str("topics")).ok() {
            Some(t) => t,
            None => continue,
        };
        let topics: &Array = match topics.dyn_ref::<Array>() {
            Some(a) => a,
            None => continue,
        };

        if topics.length() < 2 {
            continue;
        }

        let topic0 = topics.get(0).as_string().unwrap_or_default();
        if topic0.to_lowercase() != event_topic {
            continue;
        }

        // topic1 = indexed instance address (32-byte word, address is last 20 bytes)
        let topic1 = topics.get(1).as_string().unwrap_or_default();
        let hex = topic1.trim_start_matches("0x");
        if hex.len() >= 40 {
            let addr = &hex[hex.len() - 40..];
            let instance_address = format!("0x{addr}");
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[Citrea] InstanceCreated event: instance={instance_address}"
            )));
            return Ok(instance_address);
        }
    }

    Err("InstanceCreated event not found in receipt logs".to_string())
}

/// Send a read-only EVM call (`eth_call`) and return the raw hex result.
///
/// Always uses direct HTTP to the L2 RPC endpoint so that reads work
/// without any wallet connection.  Both Citrea testnet and Hardhat local
/// expose a public/localhost RPC that accepts `eth_call` over HTTP.
async fn eth_call(to: &str, calldata: &str) -> Result<String, String> {
    let l2_net = crate::storage::get_l2_network();
    eth_call_via_http(&crate::settings::citrea_rpc(l2_net), to, calldata).await
}

/// Direct HTTP JSON-RPC `eth_call` — used for read-only queries when the
/// target is a localhost RPC (Hardhat) so we don't depend on MetaMask's
/// active chain.
async fn eth_call_via_http(rpc_url: &str, to: &str, calldata: &str) -> Result<String, String> {
    let body = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_call","params":[{{"to":"{}","data":"{}"}},"latest"],"id":1}}"#,
        to, calldata
    );

    let window = web_sys::window().ok_or("no window")?;
    let mut opts = web_sys::RequestInit::new();
    opts.method("POST");
    opts.body(Some(&JsValue::from_str(&body)));

    let headers = web_sys::Headers::new().map_err(|e| format!("Headers::new: {e:?}"))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("set header: {e:?}"))?;
    opts.headers(&headers);

    let request = web_sys::Request::new_with_str_and_init(rpc_url, &opts)
        .map_err(|e| format!("Request::new: {e:?}"))?;

    let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| {
            let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
            format!("fetch to {rpc_url} failed: {msg}")
        })?;

    let resp: web_sys::Response = resp_val.dyn_into()
        .map_err(|_| "fetch did not return a Response".to_string())?;

    if !resp.ok() {
        return Err(format!("RPC HTTP {}", resp.status()));
    }

    let json_val = wasm_bindgen_futures::JsFuture::from(
        resp.json().map_err(|e| format!("resp.json(): {e:?}"))?,
    )
    .await
    .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))?;

    // Parse { "result": "0x..." }
    let result = Reflect::get(&json_val, &JsValue::from_str("result"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();

    if result.is_empty() {
        // Check for error
        let err = Reflect::get(&json_val, &JsValue::from_str("error"))
            .ok()
            .and_then(|v| {
                Reflect::get(&v, &JsValue::from_str("message"))
                    .ok()
                    .and_then(|m| m.as_string())
            })
            .unwrap_or_else(|| "empty result from RPC".to_string());
        return Err(err);
    }

    Ok(result)
}

// ── Generic Citrea JSON-RPC call (non-eth_call) ───────────────────

/// Send an arbitrary JSON-RPC call to the L2 node and return the raw JsValue result.
/// Used for Citrea-specific endpoints like `citrea_getLastCommittedL2Height`.
async fn citrea_rpc_call(method: &str, params: &str) -> Result<JsValue, String> {
    let l2_net = crate::storage::get_l2_network();
    let rpc_url = crate::settings::citrea_rpc(l2_net);
    let rpc_url = rpc_url.as_str();

    let body = format!(
        r#"{{"jsonrpc":"2.0","method":"{}","params":[{}],"id":1}}"#,
        method, params
    );

    let window = web_sys::window().ok_or("no window")?;
    let mut opts = web_sys::RequestInit::new();
    opts.method("POST");
    opts.body(Some(&JsValue::from_str(&body)));

    let headers = web_sys::Headers::new().map_err(|e| format!("Headers::new: {e:?}"))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("set header: {e:?}"))?;
    opts.headers(&headers);

    let request = web_sys::Request::new_with_str_and_init(rpc_url, &opts)
        .map_err(|e| format!("Request::new: {e:?}"))?;

    let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch failed: {}", e.as_string().unwrap_or_default()))?;

    let resp: web_sys::Response = resp_val.dyn_into()
        .map_err(|_| "fetch did not return a Response".to_string())?;

    if !resp.ok() {
        return Err(format!("RPC HTTP {}", resp.status()));
    }

    let json_val = wasm_bindgen_futures::JsFuture::from(
        resp.json().map_err(|e| format!("resp.json(): {e:?}"))?,
    )
    .await
    .map_err(|e| e.as_string().unwrap_or_default())?;

    let result = Reflect::get(&json_val, &JsValue::from_str("result"))
        .map_err(|_| "no result field".to_string())?;

    if result.is_null() || result.is_undefined() {
        return Err("null result".to_string());
    }

    Ok(result)
}

// ── Citrea finality queries ───────────────────────────────────────

/// Finality status of an L2 block with respect to Bitcoin settlement.
#[derive(Debug, Clone)]
pub struct L2FinalityStatus {
    /// Last L2 block that has been committed to Bitcoin (sequencer commitment).
    pub last_committed: u64,
    /// Commitment index of the last committed batch.
    pub last_committed_index: u64,
    /// Last L2 block that has been proven on Bitcoin (ZK proof).
    pub last_proven: u64,
    /// Commitment index of the last proven batch.
    pub last_proven_index: u64,
}

/// Query `citrea_getLastCommittedL2Height` and `citrea_getLastProvenL2Height`
/// to get the current finality boundaries.
///
/// Works on Citrea testnet/mainnet only (Hardhat local doesn't have these).
pub async fn get_finality_status() -> Result<L2FinalityStatus, String> {
    let committed = citrea_rpc_call("citrea_getLastCommittedL2Height", "").await?;
    let proven = citrea_rpc_call("citrea_getLastProvenL2Height", "").await?;

    let committed_height = Reflect::get(&committed, &JsValue::from_str("height"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u64;
    let committed_index = Reflect::get(&committed, &JsValue::from_str("commitment_index"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u64;

    let proven_height = Reflect::get(&proven, &JsValue::from_str("height"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u64;
    let proven_index = Reflect::get(&proven, &JsValue::from_str("commitment_index"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u64;

    Ok(L2FinalityStatus {
        last_committed: committed_height,
        last_committed_index: committed_index,
        last_proven: proven_height,
        last_proven_index: proven_index,
    })
}

/// Get the L2 block number from a transaction receipt.
pub async fn get_tx_block_number(tx_hash: &str) -> Result<u64, String> {
    let receipt = get_transaction_receipt(tx_hash).await?;

    let block_hex = Reflect::get(&receipt, &JsValue::from_str("blockNumber"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();

    let hex = block_hex.trim_start_matches("0x");
    if hex.is_empty() {
        return Err("no blockNumber in receipt".to_string());
    }
    u64::from_str_radix(hex, 16).map_err(|e| format!("parse blockNumber: {e}"))
}

/// Classify an L2 block height into a finality tier.
///
/// Returns one of: "Soft Confirmation", "Committed", "Proven", "Proven Finalized"
pub fn classify_finality(block_height: u64, status: &L2FinalityStatus) -> &'static str {
    if block_height <= status.last_proven {
        "Proven"
    } else if block_height <= status.last_committed {
        "Committed"
    } else {
        "Soft Confirmation"
    }
}

/// Recover the creation tx hash and block number for an instance by querying
/// the factory's `InstanceCreated` event logs.
///
/// The Citrea testnet RPC limits `eth_getLogs` to 1000 blocks per query, so we
/// scan backwards from the current block in 1000-block chunks (up to 50 chunks =
/// 50,000 blocks ≈ a few hours of soft confirmations).
///
/// Returns `(tx_hash, block_number)` on success.
pub async fn recover_instance_creation(
    factory_address: &str,
    instance_address: &str,
) -> Result<(String, u64), String> {
    // InstanceCreated topic0
    let topic0 = "0xa3de24f67beb38e771bb8aca2ec9e03485fedd4f4c7379d840ecc7cdd7f0700f";
    // topic1 = instance address, left-padded to 32 bytes
    let addr_clean = instance_address.trim_start_matches("0x").to_lowercase();
    let topic1 = format!("0x000000000000000000000000{addr_clean}");

    // Get current block number
    let current_block = get_current_block_number().await?;

    // Scan backwards in 1000-block chunks (Citrea RPC limit)
    let chunk = 999u64;
    let max_chunks = 50u64;
    for i in 0..max_chunks {
        let to_block = current_block.saturating_sub(i * (chunk + 1));
        let from_block = to_block.saturating_sub(chunk);
        if to_block == 0 {
            break;
        }

        let params = format!(
            r#"{{"fromBlock":"0x{:x}","toBlock":"0x{:x}","address":"{}","topics":["{}","{}"]}}"#,
            from_block, to_block, factory_address, topic0, topic1,
        );
        let body = format!(
            r#"{{"jsonrpc":"2.0","method":"eth_getLogs","params":[{}],"id":1}}"#,
            params,
        );

        let l2_net = crate::storage::get_l2_network();
        let rpc_url = crate::settings::citrea_rpc(l2_net);
    let rpc_url = rpc_url.as_str();

        let window = web_sys::window().ok_or("no window")?;
        let mut opts = web_sys::RequestInit::new();
        opts.method("POST");
        opts.body(Some(&JsValue::from_str(&body)));

        let headers = web_sys::Headers::new().map_err(|e| format!("{e:?}"))?;
        headers.set("Content-Type", "application/json").map_err(|e| format!("{e:?}"))?;
        opts.headers(&headers);

        let request = web_sys::Request::new_with_str_and_init(rpc_url, &opts)
            .map_err(|e| format!("{e:?}"))?;

        let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
            .await
            .map_err(|e| e.as_string().unwrap_or_default())?;

        let resp: web_sys::Response = resp_val.dyn_into().map_err(|_| "not a Response")?;
        if !resp.ok() {
            // RPC error (e.g. range too large) — skip chunk
            continue;
        }

        let json_val = wasm_bindgen_futures::JsFuture::from(
            resp.json().map_err(|e| format!("{e:?}"))?,
        )
        .await
        .map_err(|e| e.as_string().unwrap_or_default())?;

        let result = Reflect::get(&json_val, &JsValue::from_str("result")).ok();

        if let Some(arr) = result.and_then(|v| js_sys::Array::try_from(v).ok()) {
            if arr.length() > 0 {
                let log = arr.get(0);
                let tx_hash = Reflect::get(&log, &JsValue::from_str("transactionHash"))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                let block_hex = Reflect::get(&log, &JsValue::from_str("blockNumber"))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                let block = u64::from_str_radix(
                    block_hex.trim_start_matches("0x"), 16,
                ).unwrap_or(0);

                if !tx_hash.is_empty() && block > 0 {
                    return Ok((tx_hash, block));
                }
            }
        }
    }

    Err("InstanceCreated event not found in recent blocks".into())
}

/// Get the current L2 block number.
async fn get_current_block_number() -> Result<u64, String> {
    let l2_net = crate::storage::get_l2_network();
    let rpc_url = crate::settings::citrea_rpc(l2_net);
    let rpc_url = rpc_url.as_str();
    let body = r#"{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}"#;

    let window = web_sys::window().ok_or("no window")?;
    let mut opts = web_sys::RequestInit::new();
    opts.method("POST");
    opts.body(Some(&JsValue::from_str(body)));

    let headers = web_sys::Headers::new().map_err(|e| format!("{e:?}"))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("{e:?}"))?;
    opts.headers(&headers);

    let request = web_sys::Request::new_with_str_and_init(rpc_url, &opts)
        .map_err(|e| format!("{e:?}"))?;

    let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| e.as_string().unwrap_or_default())?;

    let resp: web_sys::Response = resp_val.dyn_into().map_err(|_| "not a Response")?;
    let json_val = wasm_bindgen_futures::JsFuture::from(
        resp.json().map_err(|e| format!("{e:?}"))?,
    )
    .await
    .map_err(|e| e.as_string().unwrap_or_default())?;

    let hex = Reflect::get(&json_val, &JsValue::from_str("result"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();

    u64::from_str_radix(hex.trim_start_matches("0x"), 16)
        .map_err(|e| format!("parse blockNumber: {e}"))
}

/// Recover step execution tx hashes from the instance contract's `StepExecuted` events.
///
/// Scans backwards from the current block in 1000-block chunks (Citrea RPC limit).
/// Returns a Vec of `(step_index, tx_hash, block_number)` sorted by step index.
pub async fn recover_step_executions(
    instance_address: &str,
) -> Result<Vec<(u32, String, u64)>, String> {
    // StepExecuted(uint256 indexed stepIndex, address indexed actor, uint8 status, string data)
    let topic0 = "0x5b89aea554e20d06eec989a91e330f07430146585616a53676b168e42a331b82";

    let current_block = get_current_block_number().await?;
    let l2_net = crate::storage::get_l2_network();
    let rpc_url = crate::settings::citrea_rpc(l2_net);
    let rpc_url = rpc_url.as_str();

    let mut results: Vec<(u32, String, u64)> = Vec::new();
    let chunk = 999u64;
    let max_chunks = 50u64;

    for i in 0..max_chunks {
        let to_block = current_block.saturating_sub(i * (chunk + 1));
        let from_block = to_block.saturating_sub(chunk);
        if to_block == 0 {
            break;
        }

        let body = format!(
            r#"{{"jsonrpc":"2.0","method":"eth_getLogs","params":[{{"fromBlock":"0x{:x}","toBlock":"0x{:x}","address":"{}","topics":["{}"]}}],"id":1}}"#,
            from_block, to_block, instance_address, topic0,
        );

        let window = web_sys::window().ok_or("no window")?;
        let mut opts = web_sys::RequestInit::new();
        opts.method("POST");
        opts.body(Some(&JsValue::from_str(&body)));

        let headers = web_sys::Headers::new().map_err(|e| format!("{e:?}"))?;
        headers.set("Content-Type", "application/json").map_err(|e| format!("{e:?}"))?;
        opts.headers(&headers);

        let request = web_sys::Request::new_with_str_and_init(rpc_url, &opts)
            .map_err(|e| format!("{e:?}"))?;

        let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
            .await
            .map_err(|e| e.as_string().unwrap_or_default())?;

        let resp: web_sys::Response = resp_val.dyn_into().map_err(|_| "not a Response")?;
        if !resp.ok() {
            continue;
        }

        let json_val = wasm_bindgen_futures::JsFuture::from(
            resp.json().map_err(|e| format!("{e:?}"))?,
        )
        .await
        .map_err(|e| e.as_string().unwrap_or_default())?;

        let result = Reflect::get(&json_val, &JsValue::from_str("result")).ok();

        if let Some(arr) = result.and_then(|v| js_sys::Array::try_from(v).ok()) {
            for j in 0..arr.length() {
                let log = arr.get(j);

                let tx_hash = Reflect::get(&log, &JsValue::from_str("transactionHash"))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();

                let block_hex = Reflect::get(&log, &JsValue::from_str("blockNumber"))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                let block = u64::from_str_radix(
                    block_hex.trim_start_matches("0x"), 16,
                ).unwrap_or(0);

                // topic1 = stepIndex (uint256, indexed)
                let topics = Reflect::get(&log, &JsValue::from_str("topics"))
                    .ok()
                    .and_then(|v| js_sys::Array::try_from(v).ok());
                let step_index = topics
                    .and_then(|t| {
                        if t.length() > 1 {
                            t.get(1).as_string()
                        } else {
                            None
                        }
                    })
                    .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0);

                if !tx_hash.is_empty() && block > 0 {
                    results.push((step_index, tx_hash, block));
                }
            }

            // If we found results, we can stop scanning — all step events
            // are likely in the same block range as creation
            if !results.is_empty() {
                break;
            }
        }
    }

    results.sort_by_key(|(idx, _, _)| *idx);
    Ok(results)
}

/// Direct HTTP JSON-RPC `eth_sendTransaction` — used on Hardhat local so
/// state-changing calls go through without any wallet popup.  Hardhat's
/// built-in accounts accept unsigned `eth_sendTransaction` with just a
/// `from` field (the node signs internally).
async fn send_transaction_via_http(
    rpc_url: &str,
    from: &str,
    to: &str,
    calldata: &str,
) -> Result<String, String> {
    let body = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_sendTransaction","params":[{{"from":"{}","to":"{}","data":"{}","value":"0x0"}}],"id":1}}"#,
        from, to, calldata
    );

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[Citrea] send_transaction_via_http → {rpc_url} from={from}"
    )));

    let window = web_sys::window().ok_or("no window")?;
    let mut opts = web_sys::RequestInit::new();
    opts.method("POST");
    opts.body(Some(&JsValue::from_str(&body)));

    let headers = web_sys::Headers::new().map_err(|e| format!("Headers::new: {e:?}"))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("set header: {e:?}"))?;
    opts.headers(&headers);

    let request = web_sys::Request::new_with_str_and_init(rpc_url, &opts)
        .map_err(|e| format!("Request::new: {e:?}"))?;

    let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| {
            let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
            format!("fetch to {rpc_url} failed: {msg}")
        })?;

    let resp: web_sys::Response = resp_val.dyn_into()
        .map_err(|_| "fetch did not return a Response".to_string())?;

    if !resp.ok() {
        return Err(format!("RPC HTTP {}", resp.status()));
    }

    let json_val = wasm_bindgen_futures::JsFuture::from(
        resp.json().map_err(|e| format!("resp.json(): {e:?}"))?,
    )
    .await
    .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))?;

    // Parse { "result": "0x<tx_hash>" }
    let result = Reflect::get(&json_val, &JsValue::from_str("result"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();

    if result.is_empty() {
        let err = Reflect::get(&json_val, &JsValue::from_str("error"))
            .ok()
            .and_then(|v| {
                Reflect::get(&v, &JsValue::from_str("message"))
                    .ok()
                    .and_then(|m| m.as_string())
            })
            .unwrap_or_else(|| "empty tx hash from RPC".to_string());
        return Err(err);
    }

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[Citrea] send_transaction_via_http → tx_hash={result}"
    )));

    Ok(result)
}

/// Direct HTTP JSON-RPC `eth_getTransactionReceipt` — used on Hardhat
/// local so receipt polling doesn't trigger MetaMask popups.
///
/// Returns the receipt `result` object as a `JsValue` — the same shape
/// that `parse_instance_address_from_receipt` expects (JS object with
/// `.logs[].topics[]` arrays).
async fn get_receipt_via_http(rpc_url: &str, tx_hash: &str) -> Result<JsValue, String> {
    let body = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getTransactionReceipt","params":["{}"],"id":1}}"#,
        tx_hash
    );

    let window = web_sys::window().ok_or("no window")?;
    let mut opts = web_sys::RequestInit::new();
    opts.method("POST");
    opts.body(Some(&JsValue::from_str(&body)));

    let headers = web_sys::Headers::new().map_err(|e| format!("Headers::new: {e:?}"))?;
    headers.set("Content-Type", "application/json").map_err(|e| format!("set header: {e:?}"))?;
    opts.headers(&headers);

    let request = web_sys::Request::new_with_str_and_init(rpc_url, &opts)
        .map_err(|e| format!("Request::new: {e:?}"))?;

    let resp_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| {
            let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
            format!("fetch to {rpc_url} failed: {msg}")
        })?;

    let resp: web_sys::Response = resp_val.dyn_into()
        .map_err(|_| "fetch did not return a Response".to_string())?;

    if !resp.ok() {
        return Err(format!("RPC HTTP {}", resp.status()));
    }

    let json_val = wasm_bindgen_futures::JsFuture::from(
        resp.json().map_err(|e| format!("resp.json(): {e:?}"))?,
    )
    .await
    .map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}")))?;

    // Extract the "result" field — it's already a JS object with the
    // receipt structure ({ logs: [{ topics: [...], ... }], ... }).
    // `resp.json()` parsed the entire JSON response into a JS object tree,
    // so `result.logs` is a real JS Array, not a string.
    let result = Reflect::get(&json_val, &JsValue::from_str("result"))
        .map_err(|_| "receipt response missing 'result' field".to_string())?;

    if result.is_null() || result.is_undefined() {
        // Check for RPC-level error
        let err = Reflect::get(&json_val, &JsValue::from_str("error"))
            .ok()
            .and_then(|v| {
                Reflect::get(&v, &JsValue::from_str("message"))
                    .ok()
                    .and_then(|m| m.as_string())
            })
            .unwrap_or_else(|| "Receipt not found (tx may still be pending)".to_string());
        return Err(err);
    }

    Ok(result)
}

// ── Public API ────────────────────────────────────────────────────

// ── Factory: createInstance ───────────────────────────────────────

/// Call `BINSTProcessFactory.createInstance(string, string[], string[])`.
///
/// Deploys a new `BINSTProcess` contract linked to an L1 template inscription.
/// The step names and action types are passed from the L1 inscription body.
///
/// Returns `(tx_hash, instance_address)`.  The instance address is parsed
/// from the `InstanceCreated` event in the tx receipt.  If receipt parsing
/// fails (e.g. node hasn't mined yet), the instance address will be empty
/// and the caller should fall back to `getTemplateInstances()`.
///
/// Selector: `0x6f794b70`
pub async fn create_instance(
    factory_address: &str,
    template_inscription_id: &str,
    step_names: &[String],
    step_action_types: &[String],
) -> Result<(String, String), String> {
    let from = get_l2_address()?;

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[Citrea] BINSTProcessFactory({factory_address}).createInstance(\
         template={}, steps={}) from {from}",
        &template_inscription_id[..16.min(template_inscription_id.len())],
        step_names.len(),
    )));

    let calldata = encode_create_instance(template_inscription_id, step_names, step_action_types)?;
    let tx_hash = send_transaction(&from, factory_address, &calldata).await?;

    // Wait briefly then fetch receipt to extract the deployed instance address.
    // Hardhat mines instantly; Citrea testnet soft-confirms in ~1-2s.
    // 8 retries × 2s = 16s max — generous enough for any network hiccup.
    let instance_address = match poll_receipt_for_instance(&tx_hash, 8, 2_000).await {
        Ok(addr) => addr,
        Err(e) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "[Citrea] Could not parse instance address from receipt: {e}"
            )));
            String::new()
        }
    };

    Ok((tx_hash, instance_address))
}

/// Poll `eth_getTransactionReceipt` up to `max_retries` times with
/// `delay_ms` between attempts, then parse the `InstanceCreated` event
/// to extract the new instance contract address.
async fn poll_receipt_for_instance(
    tx_hash: &str,
    max_retries: u32,
    delay_ms: i32,
) -> Result<String, String> {
    for attempt in 0..max_retries {
        match get_transaction_receipt(tx_hash).await {
            Ok(receipt) => {
                return parse_instance_address_from_receipt(&receipt);
            }
            Err(e) => {
                web_sys::console::log_1(&JsValue::from_str(&format!(
                    "[Citrea] Receipt attempt {}/{}: {e}",
                    attempt + 1, max_retries
                )));
                if attempt + 1 < max_retries {
                    let delay = js_sys::Promise::new(&mut |resolve, _| {
                        web_sys::window().unwrap()
                            .set_timeout_with_callback_and_timeout_and_arguments_0(
                                &resolve, delay_ms,
                            )
                            .ok();
                    });
                    let _ = wasm_bindgen_futures::JsFuture::from(delay).await;
                }
            }
        }
    }
    Err(format!("Receipt not available after {max_retries} attempts"))
}

/// ABI-encode `createInstance(string, string[], string[])`.
///
/// Layout (all offsets in bytes from start of params, i.e. after selector):
/// ```
///   selector (4 bytes): 6f794b70
///   word 0 (offset to string):   → points to templateInscriptionId data
///   word 1 (offset to string[]): → points to stepNames array
///   word 2 (offset to string[]): → points to stepActionTypes array
///   ... then the dynamic data for each
/// ```
fn encode_create_instance(
    template_id: &str,
    step_names: &[String],
    step_action_types: &[String],
) -> Result<String, String> {
    // We'll build the calldata as raw hex bytes.
    // ABI encoding for dynamic types is offset-based.

    let selector = "6f794b70";

    // Encode the three dynamic parameters separately, then compute offsets.
    // Head: 3 words (3 offsets) = 96 bytes = 0x60
    let head_size: usize = 3 * 32; // 96 bytes

    let encoded_template_id = abi_encode_string(template_id);
    let encoded_step_names = abi_encode_string_array(step_names);
    let encoded_step_action_types = abi_encode_string_array(step_action_types);

    let offset0 = head_size;
    let offset1 = offset0 + encoded_template_id.len() / 2; // hex chars / 2 = bytes
    let offset2 = offset1 + encoded_step_names.len() / 2;

    let mut calldata = String::with_capacity(8 + 192 + encoded_template_id.len() + encoded_step_names.len() + encoded_step_action_types.len());
    calldata.push_str("0x");
    calldata.push_str(selector);
    calldata.push_str(&format!("{:064x}", offset0));
    calldata.push_str(&format!("{:064x}", offset1));
    calldata.push_str(&format!("{:064x}", offset2));
    calldata.push_str(&encoded_template_id);
    calldata.push_str(&encoded_step_names);
    calldata.push_str(&encoded_step_action_types);

    Ok(calldata)
}

/// ABI-encode a `string` value (length-prefixed, padded to 32 bytes).
fn abi_encode_string(s: &str) -> String {
    let bytes = s.as_bytes();
    let len = bytes.len();
    // length word
    let mut hex = format!("{:064x}", len);
    // data words (padded to 32-byte boundary)
    for chunk in bytes.chunks(32) {
        let mut word = String::with_capacity(64);
        for b in chunk {
            word.push_str(&format!("{:02x}", b));
        }
        // Right-pad to 64 hex chars (32 bytes)
        while word.len() < 64 {
            word.push('0');
        }
        hex.push_str(&word);
    }
    // An empty string is just its zero length word (canonical ABI: no data words).
    hex
}

/// ABI-encode a `string[]` value.
///
/// Layout:
/// ```
///   word 0: array length
///   word 1..N: offsets to each string (relative to start of array data)
///   then: encoded strings one after another
/// ```
fn abi_encode_string_array(strings: &[String]) -> String {
    let n = strings.len();

    // Encode each string individually
    let encoded_strings: Vec<String> = strings.iter().map(|s| abi_encode_string(s)).collect();

    // The offsets area starts after: length word (32) + N offset words (N * 32)
    let data_area_start = n * 32; // bytes after the offsets (relative to after length word)

    let mut cumulative_offset = data_area_start;
    let mut offset_words = String::new();
    let mut data_section = String::new();

    for encoded in &encoded_strings {
        offset_words.push_str(&format!("{:064x}", cumulative_offset));
        data_section.push_str(encoded);
        cumulative_offset += encoded.len() / 2; // hex chars / 2 = bytes
    }

    let mut result = format!("{:064x}", n); // array length
    result.push_str(&offset_words);
    result.push_str(&data_section);
    result
}

// ── Instance: executeStep ─────────────────────────────────────────

/// Call `BINSTProcess.executeStep(uint8 status, string data)`.
///
/// `status = 1` = Completed (enum StepStatus in the contract).
/// `data` = JSON-encoded evidence string (empty string for pilot).
///
/// ABI encoding for `executeStep(1, "")`:
/// ```
/// selector  f16e3a23
/// uint8(1)  0000000000000000000000000000000000000000000000000000000000000001
/// offset    0000000000000000000000000000000000000000000000000000000000000040
/// len(0)    0000000000000000000000000000000000000000000000000000000000000000
/// ```
///
/// Selector: `0xf16e3a23`
pub async fn execute_step(instance_address: &str, step_index: u32) -> Result<String, String> {
    let from = get_l2_address()?;

    let calldata = "0xf16e3a23\
        0000000000000000000000000000000000000000000000000000000000000001\
        0000000000000000000000000000000000000000000000000000000000000040\
        0000000000000000000000000000000000000000000000000000000000000000";

    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[Citrea] BINSTProcess({instance_address}).executeStep(step={step_index}, status=Completed)"
    )));

    send_transaction(&from, instance_address, calldata).await
}

/// Read `BINSTProcess.currentStepIndex()` via `eth_call`.
///
/// Returns the 0-based index of the step currently being executed.
/// Selector: `0x334f45ec`
pub async fn get_current_step_index(instance_address: &str) -> Result<u32, String> {
    let raw = eth_call(instance_address, "0x334f45ec").await?;

    // Result is a 32-byte big-endian uint256 hex string.
    // Strip "0x", parse last 8 hex chars as u32 (step count will never overflow u32).
    let hex = raw.trim_start_matches("0x");
    if hex.len() < 8 {
        return Ok(0);
    }
    let tail = &hex[hex.len() - 8..];
    u32::from_str_radix(tail, 16).map_err(|e| format!("parse step index: {e}"))
}

/// Read `BINSTProcess.completed()` via `eth_call`.
///
/// Returns `true` when all steps have been executed.
/// Selector: `0x9d9a7fe9`
pub async fn is_process_completed(instance_address: &str) -> Result<bool, String> {
    let raw = eth_call(instance_address, "0x9d9a7fe9").await?;

    // Result is a 32-byte bool — non-zero = true.
    let hex = raw.trim_start_matches("0x");
    Ok(hex.chars().any(|c| c != '0'))
}

// ── Read-only queries ─────────────────────────────────────────

/// Parse a 32-byte hex uint256 result to u64.
fn parse_uint256(hex_result: &str) -> Result<u64, String> {
    let hex = hex_result.trim_start_matches("0x");
    if hex.is_empty() || hex.chars().all(|c| c == '0') {
        return Ok(0);
    }
    // Take last 16 hex chars (u64 range) to avoid overflow
    let start = if hex.len() > 16 { hex.len() - 16 } else { 0 };
    u64::from_str_radix(&hex[start..], 16).map_err(|e| format!("parse uint256: {e}"))
}

/// Parse a 32-byte hex address result (right-aligned, 20 bytes).
fn parse_address(hex_result: &str) -> String {
    let hex = hex_result.trim_start_matches("0x");
    if hex.len() < 40 {
        return format!("0x{hex}");
    }
    // Address is in the rightmost 40 hex chars
    let addr = &hex[hex.len() - 40..];
    format!("0x{addr}")
}

// ── Factory reads ─────────────────────────────────────────────

/// Read `BINSTProcessFactory.getInstanceCount()`.
///
/// Selector: `0xae34325c`
pub async fn fetch_instance_count(factory_address: &str) -> Result<u64, String> {
    let raw = eth_call(factory_address, "0xae34325c").await?;
    parse_uint256(&raw)
}

/// Read `BINSTProcessFactory.allInstances(uint256 index)` — instance address by index.
///
/// Selector: `allInstances(uint256)` = `0x9b0dc489`
pub async fn fetch_instance_at(factory_address: &str, index: u64) -> Result<String, String> {
    let calldata = format!("0x9b0dc489{:064x}", index);
    let raw = eth_call(factory_address, &calldata).await?;
    Ok(parse_address(&raw))
}

/// Read `BINSTProcessFactory.getTemplateInstances(string _templateInscriptionId)`.
///
/// Returns the list of instance addresses created for a specific L1 template.
/// Selector: `getTemplateInstances(string)` = `0xb43bed00`
pub async fn fetch_template_instances(
    factory_address: &str,
    template_inscription_id: &str,
) -> Result<Vec<String>, String> {
    // ABI: selector + offset(0x20) + encoded string
    let encoded_string = abi_encode_string(template_inscription_id);
    let calldata = format!(
        "0xb43bed00{:064x}{}",
        32_u64, // offset to string data
        encoded_string
    );

    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(&format!(
        "[Citrea] getTemplateInstances({}) calldata len={}",
        template_inscription_id,
        calldata.len()
    )));

    let raw = eth_call(factory_address, &calldata).await?;
    decode_abi_address_array(&raw)
}

/// Decode an ABI-encoded `address[]` return value.
///
/// Layout: offset(32) + length(32) + N×address(32 each, right-padded).
fn decode_abi_address_array(hex_result: &str) -> Result<Vec<String>, String> {
    let hex = hex_result.trim_start_matches("0x");
    if hex.is_empty() || hex.len() < 128 {
        // Minimum: offset word + length word = 128 hex chars
        // If length is 0 the result can be just offset + length(0) = 128 chars
        if hex.len() >= 128 {
            let len_hex = &hex[64..128];
            let len = usize::from_str_radix(len_hex.trim_start_matches('0').max("0"), 16)
                .map_err(|e| format!("decode array length: {e}"))?;
            if len == 0 {
                return Ok(vec![]);
            }
        }
        return Ok(vec![]);
    }

    // word 0: offset to array data (typically 0x20 = 32 bytes)
    // word 1: array length
    let len_hex = &hex[64..128];
    let len = usize::from_str_radix(len_hex.trim_start_matches('0').max("0"), 16)
        .map_err(|e| format!("decode array length: {e}"))?;

    if len == 0 {
        return Ok(vec![]);
    }

    let mut addresses = Vec::with_capacity(len);
    for i in 0..len {
        let start = 128 + i * 64; // each address is 32 bytes = 64 hex chars
        let end = start + 64;
        if end > hex.len() {
            break;
        }
        let word = &hex[start..end];
        // Address is rightmost 40 hex chars of the 32-byte word
        let addr = &word[word.len() - 40..];
        addresses.push(format!("0x{addr}"));
    }

    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(&format!(
        "[Citrea] decoded {} instance addresses from getTemplateInstances",
        addresses.len()
    )));

    Ok(addresses)
}

// ── Instance reads ────────────────────────────────────────────

/// Read `BINSTProcess.creator()`.
///
/// Selector: `0x02d05d3f`
pub async fn get_instance_creator(instance_address: &str) -> Result<String, String> {
    let raw = eth_call(instance_address, "0x02d05d3f").await?;
    Ok(parse_address(&raw))
}

/// Read `BINSTProcess.totalSteps()`.
///
/// Selector: `0x6931b3ae`
pub async fn get_total_steps(instance_address: &str) -> Result<u32, String> {
    let raw = eth_call(instance_address, "0x6931b3ae").await?;
    parse_uint256(&raw).map(|v| v as u32)
}

/// Read `BINSTProcess.templateInscriptionId()` via `eth_call`.
///
/// Returns the L1 template inscription ID string.
/// Selector: `0x0270a0b3`
pub async fn get_template_inscription_id(instance_address: &str) -> Result<String, String> {
    let raw = eth_call(instance_address, "0x0270a0b3").await?;
    decode_abi_string(&raw)
}

/// Read `BINSTProcess.getStep(uint256 index)` via `eth_call`.
///
/// Returns `(stepName, actionType)` for the given step index.
/// Selector: `getStep(uint256)` = `0x7874888a`
pub async fn get_step_name(instance_address: &str, index: u32) -> Result<(String, String), String> {
    let calldata = format!("0x7874888a{:064x}", index);
    let raw = eth_call(instance_address, &calldata).await?;

    // Return type: (string, string) — two dynamic types
    // Layout: offset0(32) | offset1(32) | string0(len+data) | string1(len+data)
    let hex = raw.trim_start_matches("0x");
    if hex.len() < 128 {
        return Ok((format!("Step {}", index + 1), "approval".to_string()));
    }

    // Read offsets (in bytes)
    let off0 = usize::from_str_radix(
        &hex[0..64].trim_start_matches('0').max("0"),
        16,
    ).unwrap_or(64) * 2; // convert to hex char offset
    let off1 = usize::from_str_radix(
        &hex[64..128].trim_start_matches('0').max("0"),
        16,
    ).unwrap_or(128) * 2;

    let name = decode_abi_string_at(hex, off0).unwrap_or_else(|_| format!("Step {}", index + 1));
    let action = decode_abi_string_at(hex, off1).unwrap_or_else(|_| "approval".to_string());

    Ok((name, action))
}

/// Read `BINSTProcess.getStepState(uint256 index)` via `eth_call`.
///
/// Returns `(StepStatus, actor, data, timestamp)`.
/// StepStatus: 0=Pending, 1=Completed, 2=Rejected.
/// Selector: `getStepState(uint256)` = `0xac5b070a`
pub async fn get_step_state_status(instance_address: &str, index: u32) -> Result<u32, String> {
    let calldata = format!("0xac5b070a{:064x}", index);
    let raw = eth_call(instance_address, &calldata).await?;

    // Return type: (uint8 status, address actor, string data, uint256 timestamp)
    // We only need the status (first word).
    let hex = raw.trim_start_matches("0x");
    if hex.len() < 64 {
        return Ok(0); // Pending
    }
    let tail = &hex[..64];
    let status = u32::from_str_radix(
        tail.trim_start_matches('0').max("0"),
        16,
    ).unwrap_or(0);
    Ok(status)
}

/// Decode a string from ABI hex at a given hex-char offset.
fn decode_abi_string_at(hex: &str, offset: usize) -> Result<String, String> {
    if offset + 64 > hex.len() {
        return Err("string offset out of bounds".into());
    }
    let len_hex = &hex[offset..offset + 64];
    let len = usize::from_str_radix(len_hex.trim_start_matches('0').max("0"), 16)
        .map_err(|e| format!("decode string length: {e}"))?;
    if len == 0 {
        return Ok(String::new());
    }
    let data_start = offset + 64;
    let data_hex = &hex[data_start..];
    let data_bytes: Vec<u8> = (0..len.min(data_hex.len() / 2))
        .filter_map(|i| u8::from_str_radix(&data_hex[i * 2..i * 2 + 2], 16).ok())
        .collect();
    String::from_utf8(data_bytes).map_err(|e| format!("decode string utf8: {e}"))
}

/// Decode an ABI-encoded `string` return value from hex.
fn decode_abi_string(hex_result: &str) -> Result<String, String> {
    let hex = hex_result.trim_start_matches("0x");
    if hex.len() < 128 {
        return Ok(String::new());
    }
    // word 0: offset to string data (always 0x20 = 32 for a single return)
    // word 1: string length
    let len_hex = &hex[64..128];
    let len = usize::from_str_radix(len_hex.trim_start_matches('0').max("0"), 16)
        .map_err(|e| format!("decode string length: {e}"))?;
    if len == 0 {
        return Ok(String::new());
    }
    // word 2+: string data
    let data_hex = &hex[128..];
    let data_bytes: Vec<u8> = (0..len.min(data_hex.len() / 2))
        .filter_map(|i| u8::from_str_radix(&data_hex[i * 2..i * 2 + 2], 16).ok())
        .collect();
    String::from_utf8(data_bytes).map_err(|e| format!("decode string utf8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference calldata from Foundry:
    //   cast calldata "createInstance(string,string[],string[])" ab '["x"]' '[""]'
    const CREATE_AB_X_EMPTY: &str = "0x6f794b70\
        0000000000000000000000000000000000000000000000000000000000000060\
        00000000000000000000000000000000000000000000000000000000000000a0\
        0000000000000000000000000000000000000000000000000000000000000120\
        0000000000000000000000000000000000000000000000000000000000000002\
        6162000000000000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000001\
        0000000000000000000000000000000000000000000000000000000000000020\
        0000000000000000000000000000000000000000000000000000000000000001\
        7800000000000000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000001\
        0000000000000000000000000000000000000000000000000000000000000020\
        0000000000000000000000000000000000000000000000000000000000000000";

    //   cast calldata "createInstance(string,string[],string[])" "tmpl-i0" \
    //     '["Review","Pay"]' '["approval","payment"]'
    const CREATE_TWO_STEPS: &str = "0x6f794b70\
        0000000000000000000000000000000000000000000000000000000000000060\
        00000000000000000000000000000000000000000000000000000000000000a0\
        0000000000000000000000000000000000000000000000000000000000000180\
        0000000000000000000000000000000000000000000000000000000000000007\
        746d706c2d693000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000002\
        0000000000000000000000000000000000000000000000000000000000000040\
        0000000000000000000000000000000000000000000000000000000000000080\
        0000000000000000000000000000000000000000000000000000000000000006\
        5265766965770000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000003\
        5061790000000000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000002\
        0000000000000000000000000000000000000000000000000000000000000040\
        0000000000000000000000000000000000000000000000000000000000000080\
        0000000000000000000000000000000000000000000000000000000000000008\
        617070726f76616c000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000007\
        7061796d656e7400000000000000000000000000000000000000000000000000";

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn create_instance_matches_cast_with_empty_action_type() {
        let got = encode_create_instance("ab", &strings(&["x"]), &strings(&[""])).unwrap();
        assert_eq!(got, CREATE_AB_X_EMPTY);
    }

    #[test]
    fn create_instance_matches_cast_with_two_steps() {
        let got = encode_create_instance(
            "tmpl-i0",
            &strings(&["Review", "Pay"]),
            &strings(&["approval", "payment"]),
        )
        .unwrap();
        assert_eq!(got, CREATE_TWO_STEPS);
    }

    #[test]
    fn empty_string_is_one_length_word() {
        assert_eq!(abi_encode_string(""), "0".repeat(64));
    }

    #[test]
    fn decode_string_round_trips_return_value() {
        // A single `string` return: offset 0x20, then the encoded string.
        let ret = format!("0x{:064x}{}", 32, abi_encode_string("KYC Verification"));
        assert_eq!(decode_abi_string(&ret).unwrap(), "KYC Verification");
    }

    #[test]
    fn parses_uint_and_address_words() {
        assert_eq!(parse_uint256(&format!("0x{:064x}", 5115)).unwrap(), 5115);
        assert_eq!(parse_uint256("0x").unwrap(), 0);
        let word = format!("0x{:0>64}", "6a1d2adbac8682773ed6700d2118c709c8ce5000");
        assert_eq!(parse_address(&word), "0x6a1d2adbac8682773ed6700d2118c709c8ce5000");
    }

    #[test]
    fn chain_ids_compare_numerically() {
        assert!(chain_ids_match("0x13fb", "0x13fb"));
        assert!(chain_ids_match("0x13FB", "0x013fb"));
        assert!(!chain_ids_match("0x1", "0x13fb"));
        assert!(!chain_ids_match("", "0x13fb"));
    }
}
