//! Small helpers shared by the views.

use crate::model::{Layer, UserError};

/// Escape text for HTML content and double-quoted attributes. Every value
/// that did not come from this source file goes through here.
pub fn esc(s: &str) -> String {
    html_escape::encode_double_quoted_attribute(s).into_owned()
}

/// Cut `s` to `max` characters, ending with an ellipsis when it was longer.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

/// The reveal txid part of an inscription id.
pub fn txid_of(inscription_id: &str) -> &str {
    inscription_id.split('i').next().unwrap_or(inscription_id)
}

pub fn btc_tx_url(txid: &str) -> String {
    format!("https://mempool.space/testnet4/tx/{}", esc(txid))
}

pub fn btc_address_url(address: &str) -> String {
    format!("https://mempool.space/testnet4/address/{}", esc(address))
}

pub fn citrea_address_url(address: &str) -> String {
    format!("https://explorer.testnet.citrea.xyz/address/{}", esc(address))
}

/// An outbound link that opens in a new tab.
pub fn ext_link(url: &str, label: &str) -> String {
    format!(r#"<a class="ext" href="{url}" target="_blank" rel="noopener noreferrer">{} ↗</a>"#, esc(label))
}

/// A monospace value with a copy button.
pub fn copyable(value: &str, shown: &str) -> String {
    format!(
        r#"<span class="mono">{}</span><button class="icon-btn" data-act="copy" data-text="{}" aria-label="Copy" title="Copy">⧉</button>"#,
        esc(shown),
        esc(value)
    )
}

pub fn layer_attr(layer: Layer) -> &'static str {
    match layer {
        Layer::Bitcoin => "btc",
        Layer::Citrea => "evm",
    }
}

pub fn layer_name(layer: Layer) -> &'static str {
    match layer {
        Layer::Bitcoin => "Bitcoin testnet4",
        Layer::Citrea => "Citrea testnet",
    }
}

/// An error shown in place, with a way to try again.
pub fn error_card(error: &UserError, retry_act: &str, retry_attrs: &str) -> String {
    format!(
        r#"<div class="error-card" role="alert"><div><strong>{} failed.</strong><p>{}</p></div><button class="btn" data-act="{retry_act}" {retry_attrs}>Retry</button></div>"#,
        esc(&error.what),
        esc(&error.why)
    )
}

/// `YYYY-MM-DD` (UTC) for a Unix timestamp.
pub fn date(unix: u64) -> String {
    // Days-to-civil conversion (proleptic Gregorian calendar).
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// FNV-1a, used to skip DOM writes when a region's HTML did not change.
pub fn fingerprint(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_neutralises_markup_and_quotes() {
        assert_eq!(esc(r#"<img src=x onerror="a()">&"#), "&lt;img src=x onerror=&quot;a()&quot;&gt;&amp;");
    }

    #[test]
    fn clip_adds_an_ellipsis_only_when_needed() {
        assert_eq!(clip("Acme", 10), "Acme");
        assert_eq!(clip("Acme Financial Cooperative", 12), "Acme Financ…");
        assert_eq!(clip("ñandú ñandú ñandú", 6), "ñandú…");
    }

    #[test]
    fn txid_is_the_part_before_the_index() {
        assert_eq!(txid_of("abci0"), "abc");
        assert_eq!(txid_of("abc"), "abc");
    }

    #[test]
    fn copyable_escapes_both_the_value_and_the_label() {
        let html = copyable(r#"a"b"#, "<x>");
        assert!(html.contains(r#"data-text="a&quot;b""#));
        assert!(html.contains("&lt;x&gt;"));
    }

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_794_000_000), "2026-11-06");
        assert_eq!(date(1_794_009_599), "2026-11-06");
    }

    #[test]
    fn fingerprint_changes_with_the_content() {
        assert_eq!(fingerprint("a"), fingerprint("a"));
        assert_ne!(fingerprint("a"), fingerprint("b"));
    }
}
