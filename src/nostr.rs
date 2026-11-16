//! Nostr events that carry an institution's members and roles.
//!
//! Nothing here is trusted because a relay said so. The chain of proof is:
//!
//!   1. The institution's current holder (a Taproot address, read from the
//!      inscription indexer) signs a BIP-322 message naming a Nostr key.
//!   2. That key publishes an announcement (kind 30078, `d` = `binst:<id>`)
//!      carrying the signature. This is the *binding*.
//!   3. The bound key defines roles as NIP-58 badge definitions (kind 30009)
//!      and gives them to members with badge awards (kind 8).
//!   4. NIP-58 has no revocation, so BINST adds one: a replaceable list
//!      (kind 30078, `d` = `binst:<id>:revoked`) from the same key.
//!
//! Roles are shown, not enforced on L2. If the institution changes hands the
//! binding stops verifying until the new holder signs one.

use bitcoin::bech32::{self, Bech32, Hrp};
use bitcoin::secp256k1::{schnorr::Signature, Message, Secp256k1, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const KIND_BADGE_AWARD: u32 = 8;
pub const KIND_BADGE_DEFINITION: u32 = 30009;
pub const KIND_APP_DATA: u32 = 30078;

/// `t` tag on every announcement, so institutions can be listed from a relay.
pub const DISCOVERY_TAG: &str = "binst-institution";

/// Relays asked by default. Reading needs no account.
pub const DEFAULT_RELAYS: [&str; 3] = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"];

// ── Events ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

/// An event before the signer adds `id`, `pubkey` and `sig`: what a NIP-07
/// signer's `signEvent` takes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Template {
    pub kind: u32,
    pub created_at: u64,
    pub tags: Vec<Vec<String>>,
    pub content: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// NIP-01 event id: SHA-256 of `[0, pubkey, created_at, kind, tags, content]`
/// serialised without whitespace.
pub fn event_id(pubkey: &str, created_at: u64, kind: u32, tags: &[Vec<String>], content: &str) -> String {
    let serialised = serde_json::json!([0, pubkey, created_at, kind, tags, content]).to_string();
    hex(&Sha256::digest(serialised.as_bytes()))
}

impl Event {
    /// Check that the id is the hash of the content and the signature is the
    /// author's. An event that fails this is discarded.
    pub fn verify(&self) -> Result<(), String> {
        if event_id(&self.pubkey, self.created_at, self.kind, &self.tags, &self.content) != self.id {
            return Err("the event id does not match its content".to_string());
        }
        let key = unhex::<32>(&self.pubkey).and_then(|k| XOnlyPublicKey::from_slice(&k).ok()).ok_or("bad author key")?;
        let id = unhex::<32>(&self.id).ok_or("bad event id")?;
        let sig = unhex::<64>(&self.sig).and_then(|s| Signature::from_slice(&s).ok()).ok_or("bad signature encoding")?;
        Secp256k1::verification_only()
            .verify_schnorr(&sig, &Message::from_digest(id), &key)
            .map_err(|_| "the signature is not the author's".to_string())
    }

    /// The first value of the first tag called `name`.
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags.iter().find(|t| t.first().map(String::as_str) == Some(name)).and_then(|t| t.get(1)).map(String::as_str)
    }

    fn tags_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Vec<String>> {
        self.tags.iter().filter(move |t| t.first().map(String::as_str) == Some(name))
    }
}

// ── Keys as people type them ─────────────────────────────────────

/// `npub1…` for a hex public key.
pub fn npub(pubkey_hex: &str) -> String {
    unhex::<32>(pubkey_hex)
        .and_then(|bytes| bech32::encode::<Bech32>(Hrp::parse("npub").ok()?, &bytes).ok())
        .unwrap_or_else(|| pubkey_hex.to_string())
}

/// A public key typed as `npub1…` or as 64 hex characters, as lowercase hex.
pub fn parse_pubkey(input: &str) -> Result<String, String> {
    let input = input.trim();
    let bytes: [u8; 32] = if input.starts_with("npub1") {
        let (hrp, data) = bech32::decode(input).map_err(|_| "that npub is not valid".to_string())?;
        if hrp.as_str() != "npub" {
            return Err("that is not an npub".to_string());
        }
        data.try_into().map_err(|_| "that npub is not valid".to_string())?
    } else {
        unhex::<32>(&input.to_lowercase()).ok_or("enter an npub or a 64-character hex key")?
    };
    XOnlyPublicKey::from_slice(&bytes).map_err(|_| "that is not a valid public key".to_string())?;
    Ok(hex(&bytes))
}

/// A role's identifier from its name: lowercase letters, digits and dashes.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

// ── What BINST puts in events ────────────────────────────────────

pub fn announcement_d(inscription: &str) -> String {
    format!("binst:{inscription}")
}

pub fn role_d(inscription: &str, slug: &str) -> String {
    format!("binst:{inscription}:role:{slug}")
}

pub fn revocations_d(inscription: &str) -> String {
    format!("binst:{inscription}:revoked")
}

/// The exact text the holder's Bitcoin wallet signs to bind a Nostr key.
pub fn binding_message(inscription: &str, nostr_pubkey: &str, issued: u64) -> String {
    format!("BINST nostr-bind v1\ninscription:{inscription}\nnostr:{nostr_pubkey}\nissued:{issued}")
}

fn tag(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

pub fn announcement_template(inscription: &str, name: &str, holder: &str, signature: &str, issued: u64) -> Template {
    Template {
        kind: KIND_APP_DATA,
        created_at: issued,
        tags: vec![
            tag(&["d", &announcement_d(inscription)]),
            tag(&["t", DISCOVERY_TAG]),
            tag(&["name", name]),
            tag(&["bind", holder, signature, &issued.to_string()]),
        ],
        content: String::new(),
    }
}

pub fn role_template(inscription: &str, slug: &str, name: &str, description: &str, now: u64) -> Template {
    Template {
        kind: KIND_BADGE_DEFINITION,
        created_at: now,
        tags: vec![tag(&["d", &role_d(inscription, slug)]), tag(&["name", name]), tag(&["description", description])],
        content: String::new(),
    }
}

pub fn award_template(inscription: &str, issuer: &str, slug: &str, member: &str, now: u64) -> Template {
    Template {
        kind: KIND_BADGE_AWARD,
        created_at: now,
        tags: vec![
            tag(&["a", &format!("{KIND_BADGE_DEFINITION}:{issuer}:{}", role_d(inscription, slug))]),
            tag(&["p", member]),
        ],
        content: String::new(),
    }
}

/// The whole revocation list; each publication replaces the previous one.
pub fn revocations_template(inscription: &str, revoked: &[(String, String)], now: u64) -> Template {
    let mut tags = vec![tag(&["d", &revocations_d(inscription)])];
    tags.extend(revoked.iter().map(|(member, slug)| tag(&["p", member, slug])));
    Template { kind: KIND_APP_DATA, created_at: now, tags, content: String::new() }
}

// ── Reading and verifying ────────────────────────────────────────

/// A Nostr key the institution's holder vouched for.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub issuer: String,
    pub holder: String,
    pub issued: u64,
}

/// Check an announcement end to end: the event is its author's, it is for
/// this institution, and the current holder signed for the author's key.
pub fn verified_binding(event: &Event, inscription: &str, current_holder: &str) -> Result<Binding, String> {
    if event.kind != KIND_APP_DATA || event.tag("d") != Some(announcement_d(inscription).as_str()) {
        return Err("not an announcement for this institution".to_string());
    }
    event.verify()?;
    let bind = event.tags_named("bind").next().ok_or("the announcement carries no binding")?;
    let (holder, signature, issued) = match bind.as_slice() {
        [_, holder, signature, issued] => (holder, signature, issued.parse::<u64>().map_err(|_| "bad binding date")?),
        _ => return Err("the binding is malformed".to_string()),
    };
    if holder != current_holder {
        return Err(format!("the binding was signed by {holder}, which does not hold the institution now"));
    }
    crate::bip322::verify(holder, &binding_message(inscription, &event.pubkey, issued), signature)
        .map_err(|why| format!("the holder's signature is not valid: {why}"))?;
    Ok(Binding { issuer: event.pubkey.clone(), holder: holder.clone(), issued })
}

/// The newest valid binding among announcements from any author.
pub fn best_binding(events: &[Event], inscription: &str, current_holder: &str) -> (Option<Binding>, Vec<String>) {
    let mut rejected = Vec::new();
    let mut best: Option<Binding> = None;
    for event in events {
        match verified_binding(event, inscription, current_holder) {
            Ok(binding) if best.as_ref().is_none_or(|b| binding.issued > b.issued) => best = Some(binding),
            Ok(_) => {}
            Err(why) => {
                if !rejected.contains(&why) {
                    rejected.push(why);
                }
            }
        }
    }
    (best, rejected)
}

/// Institutions named by announcements, newest first: `(inscription id, the
/// name the announcer gave)`. Only the event's own signature is checked here;
/// whether the holder vouched for it is checked when the institution is opened.
pub fn announced(events: &[Event]) -> Vec<(String, String)> {
    let mut found: Vec<(u64, String, String)> = Vec::new();
    for event in events.iter().filter(|e| e.kind == KIND_APP_DATA && e.verify().is_ok()) {
        let Some(id) = event.tag("d").and_then(|d| d.strip_prefix("binst:")) else { continue };
        if !crate::model::is_inscription_id(id) || found.iter().any(|(_, seen, _)| seen == id) {
            continue;
        }
        let name: String = event.tag("name").unwrap_or("").chars().take(80).collect();
        found.push((event.created_at, id.to_string(), name));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, id, name)| (id, name)).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub pubkey: String,
    pub awarded_at: u64,
    pub revoked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Role {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub members: Vec<Member>,
}

/// An institution's roles and members as its bound Nostr key published them.
#[derive(Clone, Debug, PartialEq)]
pub struct Membership {
    pub binding: Binding,
    pub roles: Vec<Role>,
}

impl Membership {
    pub fn revoked(&self) -> Vec<(String, String)> {
        self.roles
            .iter()
            .flat_map(|r| r.members.iter().filter(|m| m.revoked).map(|m| (m.pubkey.clone(), r.slug.clone())))
            .collect()
    }
}

/// Build the membership from the bound key's events. Events by anyone else,
/// with a bad id or signature, or about another institution are ignored.
pub fn membership(binding: Binding, inscription: &str, events: &[Event]) -> Membership {
    let own: Vec<&Event> = events.iter().filter(|e| e.pubkey == binding.issuer && e.verify().is_ok()).collect();
    let prefix = role_d(inscription, "");

    // Role definitions are replaceable: the newest per `d` wins.
    let mut roles: Vec<(u64, Role)> = Vec::new();
    for event in own.iter().filter(|e| e.kind == KIND_BADGE_DEFINITION) {
        let Some(slug) = event.tag("d").and_then(|d| d.strip_prefix(prefix.as_str())) else { continue };
        if slug.is_empty() {
            continue;
        }
        let role = Role {
            slug: slug.to_string(),
            name: event.tag("name").unwrap_or(slug).to_string(),
            description: event.tag("description").unwrap_or("").to_string(),
            members: Vec::new(),
        };
        match roles.iter_mut().find(|(_, r)| r.slug == role.slug) {
            Some(existing) if existing.0 < event.created_at => *existing = (event.created_at, role),
            Some(_) => {}
            None => roles.push((event.created_at, role)),
        }
    }

    // The newest revocation list.
    let revocations_d = revocations_d(inscription);
    let revoked: Vec<(&str, &str)> = own
        .iter()
        .filter(|e| e.kind == KIND_APP_DATA && e.tag("d") == Some(revocations_d.as_str()))
        .max_by_key(|e| e.created_at)
        .map(|e| e.tags_named("p").filter_map(|t| Some((t.get(1)?.as_str(), t.get(2)?.as_str()))).collect())
        .unwrap_or_default();

    for event in own.iter().filter(|e| e.kind == KIND_BADGE_AWARD) {
        let Some(slug) = event
            .tag("a")
            .and_then(|a| a.strip_prefix(&format!("{KIND_BADGE_DEFINITION}:{}:{prefix}", binding.issuer)))
        else {
            continue;
        };
        let Some((_, role)) = roles.iter_mut().find(|(_, r)| r.slug == slug) else { continue };
        for member in event.tags_named("p").filter_map(|t| t.get(1)) {
            if unhex::<32>(member).is_none() {
                continue;
            }
            match role.members.iter_mut().find(|m| m.pubkey == *member) {
                Some(existing) => existing.awarded_at = existing.awarded_at.min(event.created_at),
                None => role.members.push(Member {
                    pubkey: member.clone(),
                    awarded_at: event.created_at,
                    revoked: revoked.contains(&(member.as_str(), slug)),
                }),
            }
        }
    }

    let mut roles: Vec<Role> = roles.into_iter().map(|(_, role)| role).collect();
    roles.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    for role in &mut roles {
        role.members.sort_by_key(|m| m.awarded_at);
    }
    Membership { binding, roles }
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Throwaway keys and signed fixtures for tests. The seeds are fixed test
    //! values, not secrets.
    use super::*;
    use bitcoin::base64::{engine::general_purpose::STANDARD, Engine};
    use bitcoin::key::{Keypair, TapTweak};
    use bitcoin::{Address, Network};

    pub const INSCRIPTION: &str = "8c511a203232ace2bf5c643a29b329720da1d840a01a94b0dc7cc3ae65c6145di0";

    pub fn keypair(seed: u8) -> Keypair {
        let secret: [u8; 32] = Sha256::digest([b"binst-next test key".as_slice(), &[seed]].concat()).into();
        Keypair::from_seckey_slice(&Secp256k1::new(), &secret).unwrap()
    }

    pub fn pubkey(seed: u8) -> String {
        hex(&keypair(seed).x_only_public_key().0.serialize())
    }

    /// Sign a template as a NIP-07 signer would.
    pub fn sign(seed: u8, template: Template) -> Event {
        let pubkey = pubkey(seed);
        let id = event_id(&pubkey, template.created_at, template.kind, &template.tags, &template.content);
        let digest = Message::from_digest(unhex::<32>(&id).unwrap());
        let sig = Secp256k1::new().sign_schnorr_no_aux_rand(&digest, &keypair(seed));
        Event {
            id,
            pubkey,
            created_at: template.created_at,
            kind: template.kind,
            tags: template.tags,
            content: template.content,
            sig: hex(sig.as_ref()),
        }
    }

    /// The testnet4 Taproot address a wallet shows for this key.
    pub fn address(seed: u8) -> String {
        let secp = Secp256k1::new();
        Address::p2tr(&secp, keypair(seed).x_only_public_key().0, None, Network::Testnet4).to_string()
    }

    /// A BIP-322 simple signature of `message` from `address(seed)`.
    pub fn bip322_sign(seed: u8, message: &str) -> String {
        let secp = Secp256k1::new();
        let tweaked = keypair(seed).tap_tweak(&secp, None).to_keypair();
        let key = tweaked.x_only_public_key().0.serialize();
        let digest = Message::from_digest(crate::bip322::sighash(&key, message.as_bytes(), 0x00));
        let sig = secp.sign_schnorr_no_aux_rand(&digest, &tweaked);
        STANDARD.encode([&[0x01, 0x40][..], sig.as_ref()].concat())
    }

    /// An announcement where wallet `holder_seed` binds Nostr key `issuer_seed`.
    pub fn announcement(holder_seed: u8, issuer_seed: u8, issued: u64) -> Event {
        let message = binding_message(INSCRIPTION, &pubkey(issuer_seed), issued);
        let signature = bip322_sign(holder_seed, &message);
        sign(issuer_seed, announcement_template(INSCRIPTION, "Acme", &address(holder_seed), &signature, issued))
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;

    const HOLDER: u8 = 1;
    const ISSUER: u8 = 2;
    const ALICE: u8 = 3;
    const BOB: u8 = 4;
    const MALLORY: u8 = 9;

    #[test]
    fn a_signed_event_verifies_and_a_changed_one_does_not() {
        let event = sign(ISSUER, role_template(INSCRIPTION, "treasurer", "Treasurer", "Signs payments", 100));
        assert_eq!(event.verify(), Ok(()));
        let mut edited = event.clone();
        edited.content = "x".into();
        assert!(edited.verify().unwrap_err().contains("id"));
        let mut forged = event.clone();
        forged.pubkey = pubkey(MALLORY);
        forged.id = event_id(&forged.pubkey, forged.created_at, forged.kind, &forged.tags, &forged.content);
        assert!(forged.verify().unwrap_err().contains("author"));
    }

    #[test]
    fn event_ids_follow_nip01_serialisation() {
        // [0,"<pubkey>",1,1,[],"a\"b\n"] with the escapes NIP-01 names.
        let pubkey = "00".repeat(32);
        let expected = hex(&Sha256::digest(format!(r#"[0,"{pubkey}",1,1,[],"a\"b\n"]"#).as_bytes()));
        assert_eq!(event_id(&pubkey, 1, 1, &[], "a\"b\n"), expected);
    }

    #[test]
    fn npubs_round_trip_and_bad_keys_are_refused() {
        let key = pubkey(ALICE);
        let encoded = npub(&key);
        assert!(encoded.starts_with("npub1"));
        assert_eq!(parse_pubkey(&encoded), Ok(key.clone()));
        assert_eq!(parse_pubkey(&format!("  {}  ", key.to_uppercase())), Ok(key));
        assert!(parse_pubkey("npub1notvalid").is_err());
        assert!(parse_pubkey("abc").is_err());
        assert!(parse_pubkey("nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5").is_err());
    }

    #[test]
    fn slugs_are_safe_identifiers() {
        assert_eq!(slug("  Board Member! "), "board-member");
        assert_eq!(slug("Tesorero/a"), "tesorero-a");
        assert_eq!(slug("---"), "");
    }

    #[test]
    fn a_binding_signed_by_the_current_holder_is_accepted() {
        let event = announcement(HOLDER, ISSUER, 1_000);
        let binding = verified_binding(&event, INSCRIPTION, &address(HOLDER)).unwrap();
        assert_eq!(binding.issuer, pubkey(ISSUER));
        assert_eq!(binding.issued, 1_000);
    }

    #[test]
    fn a_binding_is_refused_when_the_holder_changed_or_the_proof_is_someone_elses() {
        // The institution moved to another address.
        let event = announcement(HOLDER, ISSUER, 1_000);
        assert!(verified_binding(&event, INSCRIPTION, &address(MALLORY)).unwrap_err().contains("does not hold"));

        // Mallory copies the holder's signature into their own announcement:
        // the signed message names the issuer's key, not Mallory's.
        let message = binding_message(INSCRIPTION, &pubkey(ISSUER), 1_000);
        let stolen = bip322_sign(HOLDER, &message);
        let copy = sign(MALLORY, announcement_template(INSCRIPTION, "Acme", &address(HOLDER), &stolen, 1_000));
        assert!(verified_binding(&copy, INSCRIPTION, &address(HOLDER)).unwrap_err().contains("not valid"));

        // Mallory signs with their own wallet but names the holder's address.
        let own = bip322_sign(MALLORY, &binding_message(INSCRIPTION, &pubkey(MALLORY), 1_000));
        let lie = sign(MALLORY, announcement_template(INSCRIPTION, "Acme", &address(HOLDER), &own, 1_000));
        assert!(verified_binding(&lie, INSCRIPTION, &address(HOLDER)).is_err());

        // A binding for another institution does not carry over.
        let other = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
        assert!(verified_binding(&event, other, &address(HOLDER)).is_err());
    }

    #[test]
    fn the_newest_valid_binding_wins_and_rejections_are_reported() {
        let old = announcement(HOLDER, ISSUER, 1_000);
        let new = announcement(HOLDER, BOB, 2_000);
        let bad = announcement(MALLORY, MALLORY, 3_000);
        let (best, rejected) = best_binding(&[old, bad, new], INSCRIPTION, &address(HOLDER));
        assert_eq!(best.unwrap().issuer, pubkey(BOB));
        assert_eq!(rejected.len(), 1);
    }

    fn binding() -> Binding {
        Binding { issuer: pubkey(ISSUER), holder: address(HOLDER), issued: 1_000 }
    }

    #[test]
    fn roles_and_members_come_only_from_the_bound_key() {
        let issuer = pubkey(ISSUER);
        let events = vec![
            sign(ISSUER, role_template(INSCRIPTION, "treasurer", "Treasurer", "Signs payments", 10)),
            sign(ISSUER, role_template(INSCRIPTION, "board", "Board", "", 11)),
            sign(ISSUER, award_template(INSCRIPTION, &issuer, "treasurer", &pubkey(ALICE), 20)),
            sign(ISSUER, award_template(INSCRIPTION, &issuer, "board", &pubkey(ALICE), 21)),
            sign(ISSUER, award_template(INSCRIPTION, &issuer, "board", &pubkey(BOB), 22)),
            // Someone else awards themselves the role: ignored.
            sign(MALLORY, award_template(INSCRIPTION, &issuer, "board", &pubkey(MALLORY), 23)),
            // An award for a role that was never defined: ignored.
            sign(ISSUER, award_template(INSCRIPTION, &issuer, "ghost", &pubkey(BOB), 24)),
        ];
        let m = membership(binding(), INSCRIPTION, &events);
        assert_eq!(m.roles.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["Board", "Treasurer"]);
        let board = &m.roles[0];
        assert_eq!(board.members.iter().map(|x| x.pubkey.clone()).collect::<Vec<_>>(), vec![pubkey(ALICE), pubkey(BOB)]);
        assert_eq!(m.roles[1].members.len(), 1);
        assert!(m.roles.iter().all(|r| r.members.iter().all(|x| x.pubkey != pubkey(MALLORY))));
    }

    #[test]
    fn a_redefined_role_keeps_its_newest_definition() {
        let events = vec![
            sign(ISSUER, role_template(INSCRIPTION, "board", "Board", "old", 10)),
            sign(ISSUER, role_template(INSCRIPTION, "board", "Board of directors", "new", 30)),
        ];
        let m = membership(binding(), INSCRIPTION, &events);
        assert_eq!(m.roles.len(), 1);
        assert_eq!(m.roles[0].name, "Board of directors");
    }

    #[test]
    fn the_newest_revocation_list_marks_members_as_revoked() {
        let issuer = pubkey(ISSUER);
        let events = vec![
            sign(ISSUER, role_template(INSCRIPTION, "board", "Board", "", 10)),
            sign(ISSUER, award_template(INSCRIPTION, &issuer, "board", &pubkey(ALICE), 20)),
            sign(ISSUER, award_template(INSCRIPTION, &issuer, "board", &pubkey(BOB), 21)),
            sign(ISSUER, revocations_template(INSCRIPTION, &[(pubkey(ALICE), "board".into()), (pubkey(BOB), "board".into())], 30)),
            // A newer list reinstates Alice.
            sign(ISSUER, revocations_template(INSCRIPTION, &[(pubkey(BOB), "board".into())], 40)),
            // A list from another key changes nothing.
            sign(MALLORY, revocations_template(INSCRIPTION, &[(pubkey(ALICE), "board".into())], 50)),
        ];
        let m = membership(binding(), INSCRIPTION, &events);
        let board = &m.roles[0];
        assert!(!board.members.iter().find(|x| x.pubkey == pubkey(ALICE)).unwrap().revoked);
        assert!(board.members.iter().find(|x| x.pubkey == pubkey(BOB)).unwrap().revoked);
        assert_eq!(m.revoked(), vec![(pubkey(BOB), "board".to_string())]);
    }

    #[test]
    fn announced_institutions_are_listed_once_newest_first_and_only_from_valid_events() {
        let other = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
        let second = sign(BOB, announcement_template(other, "Second", "tb1p", "sig", 2_000));
        let mut tampered = announcement(HOLDER, ISSUER, 3_000);
        tampered.content = "edited".into();
        let events = vec![
            announcement(HOLDER, ISSUER, 1_000),
            announcement(MALLORY, MALLORY, 1_500),
            second,
            tampered,
            // Not an announcement: a role definition and a revocation list.
            sign(ISSUER, role_template(INSCRIPTION, "board", "Board", "", 10)),
            sign(ISSUER, revocations_template(INSCRIPTION, &[], 20)),
        ];
        assert_eq!(announced(&events), vec![(other.to_string(), "Second".to_string()), (INSCRIPTION.to_string(), "Acme".to_string())]);
        assert!(announcement(HOLDER, ISSUER, 1).tags.contains(&vec!["t".to_string(), DISCOVERY_TAG.to_string()]));
    }

    #[test]
    fn roles_of_another_institution_are_not_mixed_in() {
        let other = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
        let events = vec![sign(ISSUER, role_template(other, "board", "Board", "", 10))];
        assert!(membership(binding(), INSCRIPTION, &events).roles.is_empty());
    }
}
