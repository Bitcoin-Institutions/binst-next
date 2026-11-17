//! PSBTs as animated QR codes, out and back in.
//!
//! - **BBQr** (Coinkite, <https://github.com/coinkite/BBQr/blob/master/BBQr.md>)
//!   for Coldcard Q; Sparrow reads it too.
//! - **UR `crypto-psbt`** (Blockchain Commons BCR-2020-006) for Keystone,
//!   Passport, SeedSigner, Jade and Sparrow.
//!
//! `Collector` takes the text of each scanned frame, in any order, and returns
//! the bytes once every part is in. Plain one-frame PSBTs (base64 or hex) and
//! signed transactions (hex) pass straight through.

use std::collections::BTreeMap;

use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QrFormat {
    Bbqr,
    Ur,
}

impl QrFormat {
    pub fn label(self) -> &'static str {
        match self {
            QrFormat::Bbqr => "BBQr (Coldcard)",
            QrFormat::Ur => "UR (Keystone, Passport, SeedSigner, Jade, Sparrow)",
        }
    }
}

/// UR fragment size in bytes. Small frames scan reliably from a laptop screen.
const UR_FRAGMENT: usize = 180;

/// The frames to show, in order, for a PSBT. UR uses fountain codes, so after
/// the plain parts it adds as many again of mixed parts: a scanner that missed
/// a frame recovers it from those without waiting for the whole loop.
pub fn frames(psbt: &[u8], format: QrFormat) -> Result<Vec<String>, String> {
    match format {
        QrFormat::Bbqr => {
            let options = bbqr::split::SplitOptions {
                max_version: bbqr::qr::Version::V20,
                ..Default::default()
            };
            bbqr::split::Split::try_from_data(psbt, bbqr::file_type::FileType::Psbt, options)
                .map(|s| s.parts)
                .map_err(|e| format!("BBQr: {e}"))
        }
        QrFormat::Ur => {
            let cbor = cbor_bytes(psbt);
            if cbor.len() <= UR_FRAGMENT {
                return Ok(vec![ur::encode(&cbor, &ur::Type::Custom("crypto-psbt")).to_uppercase()]);
            }
            let mut encoder = ur::Encoder::new(&cbor, UR_FRAGMENT, "crypto-psbt").map_err(|e| format!("UR: {e}"))?;
            let first = encoder.next_part().map_err(|e| format!("UR: {e}"))?;
            let seq_len = ur_seq(&first).map(|(_, n)| n).unwrap_or(1);
            let mut parts = vec![first.to_uppercase()];
            for _ in 1..seq_len * 2 {
                parts.push(encoder.next_part().map_err(|e| format!("UR: {e}"))?.to_uppercase());
            }
            Ok(parts)
        }
    }
}

/// One QR frame as an SVG string (dark modules on a light background).
pub fn svg(text: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L).map_err(|e| format!("QR: {e}"))?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(260, 260)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// `ur:type/<seq>-<len>/…` → (seq, len).
fn ur_seq(part: &str) -> Option<(usize, usize)> {
    let lower = part.to_ascii_lowercase();
    let mut pieces = lower.strip_prefix("ur:")?.split('/');
    pieces.next()?;
    let (a, b) = pieces.next()?.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// CBOR byte string (major type 2) around `data`.
fn cbor_bytes(data: &[u8]) -> Vec<u8> {
    let len = data.len();
    let mut out = match len {
        0..=23 => vec![0x40 | len as u8],
        24..=0xff => vec![0x58, len as u8],
        0x100..=0xffff => vec![0x59, (len >> 8) as u8, len as u8],
        _ => {
            let l = len as u32;
            vec![0x5a, (l >> 24) as u8, (l >> 16) as u8, (l >> 8) as u8, l as u8]
        }
    };
    out.extend_from_slice(data);
    out
}

fn cbor_unwrap(data: &[u8]) -> Result<Vec<u8>, String> {
    let (&head, rest) = data.split_first().ok_or("empty UR payload")?;
    if head >> 5 != 2 {
        return Err("UR payload is not a CBOR byte string".into());
    }
    let (len, body) = match head & 0x1f {
        n @ 0..=23 => (n as usize, rest),
        24 => (*rest.first().ok_or("short CBOR")? as usize, &rest[1..]),
        25 if rest.len() >= 2 => (u16::from_be_bytes([rest[0], rest[1]]) as usize, &rest[2..]),
        26 if rest.len() >= 4 => (u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize, &rest[4..]),
        _ => return Err("unsupported CBOR length".into()),
    };
    body.get(..len).map(<[u8]>::to_vec).ok_or_else(|| "short CBOR".into())
}

/// Progress after one scanned frame.
#[derive(Debug, PartialEq)]
pub enum Scan {
    /// Everything is in: the PSBT or transaction, as bytes or text bytes.
    Done(Vec<u8>),
    /// Parts seen so far, out of the total (UR's total is an estimate).
    Partial { have: usize, of: usize },
    /// Not a frame this page understands.
    Ignored(String),
}

#[derive(Default)]
pub struct Collector {
    bbqr: BTreeMap<usize, String>,
    bbqr_total: usize,
    ur: Option<ur::Decoder>,
    ur_seen: std::collections::BTreeSet<usize>,
    ur_len: usize,
}

impl Collector {
    pub fn push(&mut self, text: &str) -> Scan {
        let text = text.trim();
        if text.len() >= 8 && text.starts_with("B$") {
            return self.push_bbqr(text);
        }
        if text.to_ascii_lowercase().starts_with("ur:") {
            return self.push_ur(text);
        }
        if text.starts_with("cHNidP8") || text.to_ascii_lowercase().starts_with("70736274ff") || text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Scan::Done(text.as_bytes().to_vec());
        }
        Scan::Ignored("This QR is not a PSBT".into())
    }

    fn push_bbqr(&mut self, text: &str) -> Scan {
        let total = usize::from_str_radix(&text[4..6], 36).unwrap_or(0);
        let index = usize::from_str_radix(&text[6..8], 36).unwrap_or(usize::MAX);
        if total == 0 || index >= total {
            return Scan::Ignored("Bad BBQr header".into());
        }
        if self.bbqr_total != total {
            self.bbqr.clear();
            self.bbqr_total = total;
        }
        self.bbqr.insert(index, text.to_string());
        if self.bbqr.len() < total {
            return Scan::Partial { have: self.bbqr.len(), of: total };
        }
        match bbqr::join::Joined::try_from_parts(self.bbqr.values().cloned().collect()) {
            Ok(joined) => Scan::Done(joined.data),
            Err(e) => {
                self.bbqr.clear();
                Scan::Ignored(format!("BBQr: {e}"))
            }
        }
    }

    fn push_ur(&mut self, text: &str) -> Scan {
        let lower = text.to_ascii_lowercase();
        let kind_ok = lower.starts_with("ur:crypto-psbt/") || lower.starts_with("ur:psbt/") || lower.starts_with("ur:bytes/");
        if !kind_ok {
            return Scan::Ignored("This UR is not a PSBT".into());
        }
        let Some((seq, len)) = ur_seq(&lower) else {
            return match ur::decode(&lower) {
                Ok((_, payload)) => match cbor_unwrap(&payload) {
                    Ok(psbt) => Scan::Done(psbt),
                    Err(e) => Scan::Ignored(e),
                },
                Err(e) => Scan::Ignored(format!("UR: {e}")),
            };
        };
        let decoder = self.ur.get_or_insert_with(ur::Decoder::default);
        if let Err(e) = decoder.receive(&lower) {
            return Scan::Ignored(format!("UR: {e}"));
        }
        self.ur_len = len;
        if seq <= len {
            self.ur_seen.insert(seq);
        }
        if decoder.complete() {
            return match decoder.message() {
                Ok(Some(payload)) => match cbor_unwrap(&payload) {
                    Ok(psbt) => Scan::Done(psbt),
                    Err(e) => Scan::Ignored(e),
                },
                Ok(None) => Scan::Partial { have: self.ur_seen.len(), of: len },
                Err(e) => Scan::Ignored(format!("UR: {e}")),
            };
        }
        Scan::Partial { have: self.ur_seen.len().min(self.ur_len), of: self.ur_len }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn psbt(n: usize) -> Vec<u8> {
        // Pseudo-random bytes, so zlib cannot shrink them into one frame.
        let mut v = b"psbt\xff".to_vec();
        let mut x: u32 = 0x1234_5678;
        v.extend((0..n).map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        }));
        v
    }

    #[test]
    fn bbqr_round_trip_in_any_order() {
        let data = psbt(3000);
        let parts = frames(&data, QrFormat::Bbqr).unwrap();
        assert!(parts.len() > 1);
        assert!(parts[0].starts_with("B$ZP") || parts[0].starts_with("B$2P"));
        let mut c = Collector::default();
        let mut last = Scan::Ignored(String::new());
        for p in parts.iter().rev() {
            last = c.push(p);
        }
        assert_eq!(last, Scan::Done(data));
    }

    #[test]
    fn ur_round_trip_multi_and_single() {
        let data = psbt(1500);
        let parts = frames(&data, QrFormat::Ur).unwrap();
        assert!(parts[0].starts_with("UR:CRYPTO-PSBT/1-"));
        let mut c = Collector::default();
        // Skip one plain part: the mixed parts after it fill the gap.
        let mut done = None;
        for p in parts.iter().enumerate().filter(|(i, _)| *i != 2).map(|(_, p)| p) {
            if let Scan::Done(bytes) = c.push(p) {
                done = Some(bytes);
                break;
            }
        }
        assert_eq!(done, Some(data));

        let small = psbt(40);
        let one = frames(&small, QrFormat::Ur).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(Collector::default().push(&one[0]), Scan::Done(small));
    }

    #[test]
    fn plain_text_passes_and_junk_is_ignored() {
        assert_eq!(Collector::default().push("cHNidP8BAA=="), Scan::Done(b"cHNidP8BAA==".to_vec()));
        assert!(matches!(Collector::default().push("https://example.com"), Scan::Ignored(_)));
        assert!(matches!(Collector::default().push("ur:crypto-account/abc"), Scan::Ignored(_)));
    }

    #[test]
    fn svg_renders() {
        let s = svg("B$ZP0100ABC").unwrap();
        assert!(s.contains("<svg"));
    }

    #[test]
    fn cbor_wrap_unwrap() {
        for n in [0, 23, 24, 255, 256, 70_000] {
            let d = vec![9u8; n];
            assert_eq!(cbor_unwrap(&cbor_bytes(&d)).unwrap(), d);
        }
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    /// Render each frame to pixels the way a camera would see the screen and
    /// decode it with rqrr, the same decoder the page runs on camera frames.
    fn decode_pixels(text: &str) -> String {
        let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L).unwrap();
        let (w, scale, quiet) = (code.width(), 4usize, 4usize);
        let colors = code.to_colors();
        let size = (w + 2 * quiet) * scale;
        let mut img = rqrr::PreparedImage::prepare_from_greyscale(size, size, |x, y| {
            let (mx, my) = (x / scale, y / scale);
            if mx < quiet || my < quiet || mx >= w + quiet || my >= w + quiet {
                return 255;
            }
            if colors[(my - quiet) * w + (mx - quiet)] == qrcode::Color::Dark { 0 } else { 255 }
        });
        let grids = img.detect_grids();
        assert_eq!(grids.len(), 1);
        grids[0].decode().unwrap().1
    }

    #[test]
    fn camera_decoder_reads_our_own_frames() {
        let mut x: u32 = 7;
        let mut data = b"psbt\xff".to_vec();
        data.extend((0..900).map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        }));
        for format in [QrFormat::Bbqr, QrFormat::Ur] {
            let mut c = Collector::default();
            let mut result = None;
            for f in frames(&data, format).unwrap() {
                if let Scan::Done(bytes) = c.push(&decode_pixels(&f)) {
                    result = Some(bytes);
                    break;
                }
            }
            assert_eq!(result.as_deref(), Some(&data[..]), "{format:?}");
        }
    }
}
