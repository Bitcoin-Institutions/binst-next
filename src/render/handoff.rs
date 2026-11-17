//! The hand-off dialog: send a PSBT to a signer outside the browser and take
//! the signed result back.

use crate::qr::{self, QrFormat};
use crate::render::common::esc;
use crate::state::Handoff;

fn qr_block(h: &Handoff) -> String {
    let bytes = bitcoin::base64::Engine::decode(&bitcoin::base64::engine::general_purpose::STANDARD, &h.psbt_b64).unwrap_or_default();
    let frames = match qr::frames(&bytes, h.format) {
        Ok(frames) => frames,
        Err(why) => return format!(r#"<p class="form-error">{}</p>"#, esc(&why)),
    };
    let svgs: String = frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let svg = qr::svg(f).unwrap_or_default();
            format!(r#"<div class="qr-frame"{}>{svg}</div>"#, if i == 0 { "" } else { " hidden" })
        })
        .collect();
    let toggle = |f: QrFormat, label: &str| {
        format!(
            r#"<button class="tab{}" aria-pressed="{}" data-act="handoff-format" data-format="{}">{label}</button>"#,
            if f == h.format { " is-on" } else { "" },
            f == h.format,
            match f {
                QrFormat::Bbqr => "bbqr",
                QrFormat::Ur => "ur",
            }
        )
    };
    format!(
        r#"<div class="handoff-qr"><div class="tabs" role="group" aria-label="QR format">{}{}</div><div id="handoff-qr" class="qr-stage" aria-label="Animated QR code of the PSBT">{svgs}</div><p class="fineprint"><span id="handoff-qr-count">frame 1 of {}</span> · {}</p></div>"#,
        toggle(QrFormat::Ur, "UR"),
        toggle(QrFormat::Bbqr, "BBQr"),
        frames.len(),
        esc(h.format.label()),
    )
}

pub fn dialog(h: &Handoff) -> String {
    let core_cmd = format!("bitcoin-cli -testnet4 -rpcwallet=<your-wallet> walletprocesspsbt \"{}\"", h.psbt_b64);
    let what = if h.step == "commit" {
        "The commit spends plain tBTC from your address into the inscription's one-time output."
    } else {
        "The page already signed the inscription input. Your signer only signs the parent inscription's input, which proves you hold the institution."
    };
    let error = h.error.as_ref().map(|e| format!(r#"<p class="form-error" role="alert">{}</p>"#, esc(e))).unwrap_or_default();
    let camera = match &h.scanning {
        Some(status) => format!(
            r#"<div class="scanner"><video id="handoff-video" playsinline muted></video><p id="handoff-scan-status" class="fineprint" role="status">{}</p><button class="btn" data-act="handoff-camera" data-on="0">Stop the camera</button></div>"#,
            esc(status)
        ),
        None => r#"<button class="btn" data-act="handoff-camera" data-on="1">Scan the signed QR with the camera</button>"#.to_string(),
    };
    format!(
        r#"<div class="backdrop"></div><div class="modal modal--wide" role="dialog" aria-modal="true" aria-labelledby="handoff-title">
<header class="modal-head"><h2 id="handoff-title">Sign the {step} elsewhere</h2><button class="icon-btn" data-act="handoff-cancel" aria-label="Cancel">×</button></header>
<p class="tab-help">{label}. {what}</p>
<div class="handoff-grid">
<section aria-labelledby="ho-send"><h3 id="ho-send">1 · Take the PSBT to your signer</h3>
<div class="btn-row"><button class="btn primary" data-act="handoff-download">Download binst-{step}.psbt</button><button class="btn" data-act="copy" data-text="{b64}">Copy base64</button></div>
<p class="fineprint"><strong>Sparrow:</strong> File → Open Transaction (file, clipboard or QR), sign, then Save or Show QR. <strong>Electrum:</strong> Tools → Load transaction. <strong>Coldcard, Keystone, Passport, SeedSigner, Jade:</strong> scan the QR below or use the file on a microSD card.</p>
{qr}
<details><summary>Bitcoin Core command</summary><pre class="mono code">{cmd}</pre><button class="btn" data-act="copy" data-text="{cmd}">Copy command</button><p class="fineprint">Paste the <code>psbt</code> field of the answer below. Or in Bitcoin Core GUI: File → Load PSBT from clipboard.</p></details>
</section>
<section aria-labelledby="ho-back"><h3 id="ho-back">2 · Hand back the signed PSBT</h3>
<label class="field"><span>Paste the signed PSBT (base64 or hex) or signed transaction (hex)</span><textarea id="handoff-in" rows="5" spellcheck="false" autocomplete="off"></textarea></label>
<div class="btn-row"><button class="btn primary" data-act="handoff-paste">Use the pasted PSBT</button><label class="btn">Load a .psbt file<input type="file" id="handoff-file" data-change="handoff-file" accept=".psbt,.txt,.txn,application/octet-stream" hidden></label></div>
{camera}
{error}
<p class="fineprint">The page checks that what comes back is the very transaction it sent; a changed input, output or amount is refused. Nothing is broadcast until both sides are signed.</p>
</section>
</div>
<footer class="modal-foot"><button class="btn" data-act="handoff-cancel">Cancel</button></footer></div>"#,
        step = esc(&h.step),
        label = esc(&h.label),
        b64 = esc(&h.psbt_b64),
        qr = qr_block(h),
        cmd = esc(&core_cmd),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_offers_every_channel_and_escapes() {
        let h = Handoff {
            step: "commit".into(),
            label: "Institution \"<x>\"".into(),
            psbt_b64: "cHNidP8BAA==".into(),
            format: QrFormat::Ur,
            error: Some("bad <input>".into()),
            scanning: None,
        };
        let html = dialog(&h);
        for needle in ["handoff-download", "handoff-paste", "handoff-file", "handoff-camera", "walletprocesspsbt", "qr-frame"] {
            assert!(html.contains(needle), "{needle}");
        }
        assert!(html.contains("&lt;x&gt;"));
        assert!(html.contains("bad &lt;input&gt;"));
    }
}
