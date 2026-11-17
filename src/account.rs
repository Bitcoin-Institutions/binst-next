//! A watch-only Bitcoin account for "sign elsewhere" wallets.
//!
//! The user pastes the account's output descriptor (or extended key with its
//! origin) from Sparrow, Bitcoin Core (`listdescriptors`), Electrum or a
//! hardware signer. The page derives the receive address and its key, and
//! writes the key's origin into every PSBT so the signer recognises its inputs.
//! No private key ever reaches the page.

use std::str::FromStr;

use bitcoin::bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub};
use bitcoin::key::XOnlyPublicKey;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::{Address, KnownHrp};

/// What the page needs from an imported account, for one receive index.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub address: String,
    /// Untweaked x-only key of the address (also the institution's admin key).
    pub internal: XOnlyPublicKey,
    /// Master fingerprint and full path of that key, when the descriptor had them.
    pub origin: Option<(Fingerprint, DerivationPath)>,
    /// The descriptor as typed, normalised (for display and later sessions).
    pub source: String,
}

/// Parse a Taproot account descriptor and derive receive address `index`.
///
/// Accepted forms (checksum optional, `h` or `'` for hardened):
/// - `tr([fp/86'/1'/0']tpub…/0/*)` (Bitcoin Core `listdescriptors`)
/// - `tr([fp/86'/1'/0']tpub…/<0;1>/*)` (Sparrow, multipath)
/// - `[fp/86'/1'/0']tpub…` (key expression only)
/// - `tr([fp/86'/1'/0'/0/5]<x-only hex>)` (one address, from `getaddressinfo`)
pub fn parse(input: &str, index: u32) -> Result<Account, String> {
    let mut s: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(hash) = s.find('#') {
        let given = s[hash + 1..].to_string();
        s.truncate(hash);
        if crate::reveal::descriptor_checksum(&s).as_deref() != Some(given.as_str()) {
            return Err("The descriptor's checksum does not match: it was changed or mistyped. Copy it again.".into());
        }
    }
    let source = s.clone();
    if let Some(inner) = s.strip_prefix("tr(") {
        s = inner.strip_suffix(')').ok_or("A tr(…) descriptor must end with ')'")?.to_string();
        if s.contains(',') {
            return Err("This descriptor has a script tree; use a single-key tr(…) account".into());
        }
    } else if s.starts_with("wpkh(") || s.starts_with("pkh(") || s.starts_with("sh(") || s.starts_with("wsh(") {
        return Err("BINST needs a Taproot (tr) account; this descriptor is not Taproot".into());
    }

    let (origin, key) = match s.strip_prefix('[') {
        Some(rest) => {
            let close = rest.find(']').ok_or("Key origin is missing its ']'")?;
            (Some(parse_origin(&rest[..close])?), rest[close + 1..].to_string())
        }
        None => (None, s),
    };

    // A single key: tr([origin]<xonly>).
    if key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
        let internal = XOnlyPublicKey::from_str(&key).map_err(|e| format!("Bad key: {e}"))?;
        return Ok(account(internal, origin, source));
    }

    let mut parts = key.split('/');
    let xpub_str = parts.next().unwrap_or_default();
    let xpub = Xpub::from_str(xpub_str).map_err(|e| format!("Not an extended public key: {e}"))?;
    let rest: Vec<&str> = parts.collect();
    // Steps after the xpub: the receive chain is 0, then the wildcard index.
    let mut steps: Vec<ChildNumber> = Vec::new();
    for (i, step) in rest.iter().enumerate() {
        let n = match *step {
            "*" if i + 1 == rest.len() => index,
            "<0;1>" => 0,
            "*" => return Err("'*' must be the last step".into()),
            other if other.ends_with(['h', '\'']) => return Err(format!("Hardened step {other} after an xpub cannot be derived")),
            other => other.parse::<u32>().map_err(|_| format!("Bad derivation step {other:?}"))?,
        };
        steps.push(ChildNumber::from_normal_idx(n).map_err(|e| e.to_string())?);
    }
    if rest.is_empty() {
        steps = vec![ChildNumber::from_normal_idx(0).unwrap(), ChildNumber::from_normal_idx(index).map_err(|e| e.to_string())?];
    }
    let secp = Secp256k1::verification_only();
    let derived = xpub.derive_pub(&secp, &steps).map_err(|e| format!("Derivation failed: {e}"))?;
    let internal = XOnlyPublicKey::from(derived.public_key);
    let origin = origin.map(|(fp, path)| (fp, path.extend(&steps)));
    Ok(account(internal, origin, source))
}

fn account(internal: XOnlyPublicKey, origin: Option<(Fingerprint, DerivationPath)>, source: String) -> Account {
    let address = Address::p2tr(&Secp256k1::verification_only(), internal, None, KnownHrp::Testnets).to_string();
    Account { address, internal, origin, source }
}

fn parse_origin(s: &str) -> Result<(Fingerprint, DerivationPath), String> {
    let (fp, path) = s.split_once('/').unwrap_or((s, ""));
    let fingerprint = Fingerprint::from_str(fp).map_err(|_| format!("Bad fingerprint {fp:?} (8 hex digits)"))?;
    let path = format!("m/{}", path.replace('h', "'"));
    let path = DerivationPath::from_str(path.trim_end_matches('/')).map_err(|e| format!("Bad origin path: {e}"))?;
    Ok((fingerprint, path))
}

/// `fp/86'/1'/0'/0/0` for storing an origin in the session.
pub fn origin_to_string(origin: &(Fingerprint, DerivationPath)) -> String {
    let path = origin.1.to_string();
    format!("{}/{}", origin.0, path.trim_start_matches("m/").trim_start_matches('m'))
}

pub fn origin_from_string(s: &str) -> Option<(Fingerprint, DerivationPath)> {
    (!s.is_empty()).then(|| parse_origin(s).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    // BIP-86 test vector: "abandon … about", account m/86'/0'/0'.
    const XPUB: &str = "xpub6BgBgsespWvERF3LHQu6CnqdvfEvtMcQjYrcRzx53QJjSxarj2afYWcLteoGVky7D3UKDP9QyrLprQ3VCECoY49yfdDEHGCtMMj92pReUsQ";
    const KEY_0_0: &str = "cc8a4bc64d897bddc5fbc2f670f7a8ba0b386779106cf1223c6fc5d7cd6fc115";
    const KEY_0_1: &str = "83dfe85a3151d2517290da461fe2815591ef69f2b18a2ce63f01697a8b313145";

    #[test]
    fn bip86_vectors_through_every_form() {
        for desc in [
            format!("tr([73c5da0a/86'/0'/0']{XPUB}/0/*)"),
            crate::reveal::with_checksum(&format!("tr([73c5da0a/86h/0h/0h]{XPUB}/0/*)")),
            format!("tr([73c5da0a/86'/0'/0']{XPUB}/<0;1>/*)"),
            format!("[73c5da0a/86'/0'/0']{XPUB}"),
        ] {
            let a = parse(&desc, 0).unwrap();
            assert_eq!(a.internal.to_string(), KEY_0_0, "{desc}");
            let (fp, path) = a.origin.clone().unwrap();
            assert_eq!(fp.to_string(), "73c5da0a");
            assert_eq!(path.to_string(), "86'/0'/0'/0/0");
            assert!(a.address.starts_with("tb1p"));
            assert_eq!(parse(&desc, 1).unwrap().internal.to_string(), KEY_0_1);
        }
    }

    #[test]
    fn single_key_from_getaddressinfo() {
        let a = parse(&crate::reveal::with_checksum(&format!("tr([73c5da0a/86h/0h/0h/0/0]{KEY_0_0})")), 7).unwrap();
        assert_eq!(a.internal.to_string(), KEY_0_0);
        assert_eq!(origin_to_string(a.origin.as_ref().unwrap()), "73c5da0a/86'/0'/0'/0/0");
        assert_eq!(origin_from_string("73c5da0a/86'/0'/0'/0/0"), a.origin);
    }

    #[test]
    fn a_wrong_checksum_is_refused() {
        let good = crate::reveal::with_checksum(&format!("tr([73c5da0a/86'/0'/0']{XPUB}/0/*)"));
        assert!(parse(&good, 0).is_ok());
        let typo = good.replacen("73c5da0a", "73c5da0b", 1);
        assert!(parse(&typo, 0).unwrap_err().contains("checksum"));
    }

    #[test]
    fn non_taproot_and_trees_are_refused() {
        assert!(parse(&format!("wpkh([73c5da0a/84'/0'/0']{XPUB}/0/*)"), 0).unwrap_err().contains("Taproot"));
        assert!(parse(&format!("tr({KEY_0_0},pk({KEY_0_1}))"), 0).unwrap_err().contains("tree"));
        assert!(parse("tr(nonsense)", 0).is_err());
    }

    #[test]
    fn no_origin_still_gives_an_address() {
        let a = parse(&format!("tr({XPUB}/0/*)"), 0).unwrap();
        assert_eq!(a.internal.to_string(), KEY_0_0);
        assert!(a.origin.is_none());
    }
}
