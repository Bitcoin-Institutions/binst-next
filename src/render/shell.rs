//! The frame around every page: top bar, in-flight operations, toasts and
//! the wallet picker.

use crate::connect::EvmCandidate;
use crate::model::{short, Layer};
use crate::render::common::{btc_tx_url, esc, layer_attr, layer_name};
use crate::render::icons::{btc_wallet_icon, evm_wallet_icon};
use crate::router::Route;
use crate::state::{AppState, OpStage, ToastKind};

/// A Bitcoin wallet the picker can offer.
pub struct BtcCandidate {
    pub key: String,
    pub name: String,
    pub icon: Option<String>,
}

fn nav_link(route: &Route, current: &Route, label: &str) -> String {
    let on = match (route, current) {
        (Route::Institutions, Route::Institution { .. }) => true,
        (Route::Lab(_), Route::Lab(_)) => true,
        (a, b) => a == b,
    };
    format!(
        r#"<a href="{}"{}>{label}</a>"#,
        crate::router::href(route),
        if on { r#" class="on" aria-current="page""# } else { "" }
    )
}

fn wallet_pill(layer: Layer, address: Option<&str>, wallet: &str, warn: bool) -> String {
    let (glyph, attr) = match layer {
        Layer::Bitcoin => ("₿", "btc"),
        Layer::Citrea => ("⬡", "evm"),
    };
    match address {
        Some(address) => format!(
            r#"<span class="pill pill--{attr} is-on{}" title="{} · {}"><span class="pill-glyph">{glyph}</span><span class="mono">{}</span><button class="pill-x" data-act="disconnect" data-layer="{attr}" aria-label="Disconnect {}">×</button></span>"#,
            if warn { " is-warn" } else { "" },
            esc(wallet),
            layer_name(layer),
            esc(&short(address)),
            layer_name(layer),
        ),
        None => format!(
            r#"<button class="pill pill--{attr}" data-act="connect" data-layer="{attr}"><span class="pill-glyph">{glyph}</span>Connect {}</button>"#,
            match layer {
                Layer::Bitcoin => "Bitcoin",
                Layer::Citrea => "Citrea",
            }
        ),
    }
}

pub fn topbar(state: &AppState) -> String {
    let evm_warn = state.evm.as_ref().is_some_and(|e| e.network != "citrea-testnet" && e.network != "hardhat-local");
    format!(
        r##"<a class="brand" href="#/" aria-label="BINST home"><span class="brand-mark">₿</span><span class="brand-name">BINST</span><span class="brand-tag">pilot</span></a><nav class="nav" aria-label="Main">{}{}{}{}</nav><div class="wallets" aria-label="Wallets">{}{}</div>"##,
        nav_link(&Route::Home, &state.route, "Home"),
        nav_link(&Route::Institutions, &state.route, "My institutions"),
        nav_link(&Route::New, &state.route, "Create"),
        nav_link(&Route::Lab(None), &state.route, "Lab"),
        wallet_pill(
            Layer::Bitcoin,
            state.btc.as_ref().map(|b| b.address.as_str()),
            state.btc.as_ref().map(|b| b.wallet_name.as_str()).unwrap_or(""),
            false
        ),
        wallet_pill(
            Layer::Citrea,
            state.evm.as_ref().map(|e| e.address.as_str()),
            state.evm.as_ref().map(|e| e.wallet_name.as_str()).unwrap_or(""),
            evm_warn
        ),
    )
}

/// Transactions in flight, shown under the top bar until they settle.
pub fn ops(state: &AppState) -> String {
    state
        .ops
        .iter()
        .map(|op| {
            let (class, status) = match &op.stage {
                OpStage::Mempool => ("is-waiting", "in the mempool, waiting for a block".to_string()),
                OpStage::Confirmed(height) => ("is-done", format!("confirmed in block {height}")),
                OpStage::Failed(why) => ("is-failed", esc(why)),
            };
            format!(
                r#"<div class="op {class}" role="status"><i class="dot"></i><span><strong>{}</strong> · {status}</span><a class="ext" href="{}" target="_blank" rel="noopener noreferrer">View transaction ↗</a></div>"#,
                esc(&op.label),
                btc_tx_url(&op.txid)
            )
        })
        .collect()
}

pub fn toasts(state: &AppState) -> String {
    state
        .toasts
        .iter()
        .map(|toast| {
            let (class, role) = match toast.kind {
                ToastKind::Info => ("toast--info", "status"),
                ToastKind::Success => ("toast--success", "status"),
                ToastKind::Error => ("toast--error", "alert"),
            };
            format!(
                r#"<div class="toast {class}" role="{role}"><span>{}</span><button class="icon-btn" data-act="dismiss-toast" data-id="{}" aria-label="Dismiss">×</button></div>"#,
                esc(&toast.text),
                toast.id
            )
        })
        .collect()
}

fn wallet_icon(svg: String, url: Option<&str>, fallback: &str) -> String {
    if svg.starts_with("<svg") {
        return svg;
    }
    // Wallet-supplied icons are only used when they are data: or https: URLs.
    match url.filter(|u| u.starts_with("data:image/") || u.starts_with("https://")) {
        Some(url) => format!(r#"<img src="{}" alt="" width="28" height="28">"#, esc(url)),
        None => format!(r#"<span class="wallet-fallback">{fallback}</span>"#),
    }
}

/// The one place to connect: a dialog with a tab per layer.
pub fn picker(layer: Layer, btc: &[BtcCandidate], evm: &[EvmCandidate], walletconnect: bool) -> String {
    let tab = |l: Layer, label: &str| {
        format!(
            r#"<button role="tab" class="tab{}" aria-selected="{}" data-act="connect" data-layer="{}">{label}</button>"#,
            if l == layer { " is-on" } else { "" },
            l == layer,
            layer_attr(l)
        )
    };
    let (help, list, empty) = match layer {
        Layer::Bitcoin => (
            "Used to create institutions and define processes. Set the wallet to testnet4 first.",
            btc.iter()
                .map(|w| {
                    format!(
                        r#"<button class="wallet-item" data-act="pick-btc" data-key="{}" data-name="{}">{}<span class="wallet-name">{}</span><span class="wallet-sub">window.{}</span></button>"#,
                        esc(&w.key),
                        esc(&w.name),
                        wallet_icon(btc_wallet_icon(&w.key, &w.name), w.icon.as_deref(), "₿"),
                        esc(&w.name),
                        esc(&w.key)
                    )
                })
                .collect::<String>(),
            "No Bitcoin wallet found in this browser. UniSat works on testnet4; install it, switch it to testnet4 and reload.",
        ),
        Layer::Citrea => (
            "Used to start process instances and execute their steps. The wallet is switched to Citrea testnet for you.",
            evm.iter()
                .enumerate()
                .map(|(index, w)| {
                    format!(
                        r#"<button class="wallet-item" data-act="pick-evm" data-index="{index}">{}<span class="wallet-name">{}</span><span class="wallet-sub">{}</span></button>"#,
                        wallet_icon(evm_wallet_icon(&w.name), Some(w.icon.as_str()), "⬡"),
                        esc(&w.name),
                        if w.rdns.is_empty() { "EIP-1193".to_string() } else { esc(&w.rdns) }
                    )
                })
                .collect::<String>(),
            "No EVM wallet found in this browser. MetaMask or any EIP-1193 wallet works.",
        ),
    };
    let wc = if layer == Layer::Citrea && walletconnect {
        r#"<button class="wallet-item" data-act="pick-wc"><span class="wallet-fallback">◎</span><span class="wallet-name">WalletConnect</span><span class="wallet-sub">Scan with a mobile wallet</span></button>"#
    } else {
        ""
    };
    let body = if list.is_empty() && wc.is_empty() {
        format!(r#"<p class="empty">{empty}</p>"#)
    } else {
        format!(r#"<div class="wallet-list">{list}{wc}</div>"#)
    };
    format!(
        r#"<div class="backdrop" data-act="close-picker"></div><div class="modal" role="dialog" aria-modal="true" aria-labelledby="picker-title"><header class="modal-head"><h2 id="picker-title">Connect a wallet</h2><button class="icon-btn" data-act="close-picker" aria-label="Close">×</button></header><div class="tabs" role="tablist">{}{}</div><p class="tab-help">{help}</p>{body}<p class="fineprint">Test networks only. This pilot is unaudited: use test coins.</p></div>"#,
        tab(Layer::Bitcoin, "Bitcoin · testnet4"),
        tab(Layer::Citrea, "Citrea · testnet"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{BtcSession, Op};

    const ID: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";

    #[test]
    fn topbar_offers_one_connect_per_layer_until_connected() {
        let mut s = AppState::new(ID);
        let html = topbar(&s);
        assert!(html.contains(r#"data-act="connect" data-layer="btc""#));
        assert!(html.contains(r#"data-act="connect" data-layer="evm""#));
        s.btc = Some(BtcSession { address: "tb1p0x9489n5mj2zklqdh0wzazehn23tc2ahdflgsm6ws896pvv35k8sdpd82j".into(), wallet_name: "UniSat".into(), ..BtcSession::default() });
        let html = topbar(&s);
        assert!(html.contains("tb1p0x94…dpd82j"));
        assert!(html.contains(r#"data-act="disconnect" data-layer="btc""#));
    }

    #[test]
    fn institution_pages_highlight_my_institutions() {
        let mut s = AppState::new(ID);
        s.route = Route::Institution { id: ID.into(), definition: None };
        let html = topbar(&s);
        assert!(html.contains(r##"<a href="#/institutions" class="on""##));
    }

    #[test]
    fn picker_shows_the_tab_help_and_an_empty_state() {
        let html = picker(Layer::Bitcoin, &[], &[], false);
        assert!(html.contains("No Bitcoin wallet found"));
        assert!(html.contains(r#"aria-selected="true" data-act="connect" data-layer="btc""#));
        let html = picker(
            Layer::Bitcoin,
            &[BtcCandidate { key: "unisat".into(), name: "UniSat <b>".into(), icon: None }],
            &[],
            false,
        );
        assert!(html.contains(r#"data-act="pick-btc" data-key="unisat""#));
        assert!(html.contains("UniSat &lt;b&gt;"));
    }

    #[test]
    fn untrusted_wallet_icon_urls_are_dropped() {
        assert!(wallet_icon(String::new(), Some("javascript:alert(1)"), "₿").contains("wallet-fallback"));
        assert!(wallet_icon(String::new(), Some("https://example.com/i.png"), "₿").contains("<img"));
    }

    #[test]
    fn ops_report_their_stage_with_a_link() {
        let mut s = AppState::new(ID);
        s.ops.push(Op { id: 1, label: "Institution “Acme”".into(), txid: "ab".into(), layer: Layer::Bitcoin, stage: OpStage::Confirmed(129535) });
        let html = ops(&s);
        assert!(html.contains("confirmed in block 129535"));
        assert!(html.contains("https://mempool.space/testnet4/tx/ab"));
    }
}
