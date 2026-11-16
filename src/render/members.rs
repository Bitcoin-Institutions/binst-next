//! The members page: who holds which role in an institution, as published
//! on Nostr and verified against the institution's holder on Bitcoin.

use crate::model::{short, Load, Tree};
use crate::nostr::{npub, Membership, Role};
use crate::render::common::{btc_address_url, copyable, date, error_card, esc, ext_link};
use crate::state::{AppState, MembersView};

const STATUS: &str = r#"<span class="badge badge-pending">Experimental · Nostr relays · verified against Bitcoin</span>"#;

pub fn page(state: &AppState, id: &str) -> String {
    let esc_id = esc(id);
    let name = state.tree(id).map(|t| t.institution.name.clone()).unwrap_or_else(|| short(id));
    let head = format!(
        r##"<nav class="crumbs" aria-label="Breadcrumb"><a href="#/institutions">My institutions</a><span>/</span><a href="#/i/{esc_id}">{}</a><span>/</span><span>Members</span></nav><header class="page-head"><div>{STATUS}<h1>Members and roles</h1><p class="sub">Roles are Nostr badges issued by a key the institution's holder vouched for. They are shown here, not enforced on L2.</p></div><button class="btn small" data-act="reload-members" data-id="{esc_id}">Reload</button></header>"##,
        esc(&name)
    );
    let body = match (state.tree(id), state.members.get(id)) {
        (None, _) => match state.trees.get(id) {
            Some(Load::Failed(error)) => error_card(error, "reload-tree", &format!(r#"data-id="{esc_id}""#)),
            _ => r#"<div class="card" aria-busy="true"><p class="progress"><i class="spinner"></i>Loading the institution…</p></div>"#.to_string(),
        },
        (Some(_), None | Some(Load::Idle | Load::Loading)) => {
            r#"<div class="card" aria-busy="true"><p class="progress"><i class="spinner"></i>Asking the Nostr relays…</p></div>"#.to_string()
        }
        (Some(_), Some(Load::Failed(error))) => error_card(error, "reload-members", &format!(r#"data-id="{esc_id}""#)),
        (Some(tree), Some(Load::Ready(view))) => ready(state, tree, view),
    };
    format!(r#"<section class="page narrow wide-ish">{head}{}{body}{}</section>"#, feedback(state), explainer())
}

fn feedback(state: &AppState) -> String {
    let mut out = String::new();
    if let Some(busy) = &state.members_busy {
        out.push_str(&format!(r#"<p class="progress" role="status"><i class="spinner"></i>{}</p>"#, esc(busy)));
    }
    if let Some(error) = &state.members_error {
        out.push_str(&format!(r#"<p class="form-error" role="alert">{}</p>"#, esc(error)));
    }
    out
}

fn ready(state: &AppState, tree: &Tree, view: &MembersView) -> String {
    let inst_id = esc(&tree.institution.id);
    let holds = state.holds(tree);
    let disabled = if state.members_busy.is_some() { " disabled" } else { "" };
    let relays = format!("{} of {} relays answered", view.answered, view.asked);
    let rejected = if view.rejected.is_empty() {
        String::new()
    } else {
        format!(
            r#"<details class="rejected"><summary>{} announcement{} ignored</summary><ul>{}</ul></details>"#,
            view.rejected.len(),
            if view.rejected.len() == 1 { " was" } else { "s were" },
            view.rejected.iter().map(|why| format!("<li>{}</li>", esc(why))).collect::<String>()
        )
    };

    let Some(membership) = &view.membership else {
        let action = if holds {
            format!(
                r#"<div class="actions"><button class="btn primary" data-act="bind-nostr" data-inst="{inst_id}"{disabled}>Bind a Nostr key</button></div><p class="explain">Two prompts: your Bitcoin wallet signs a message that names your Nostr key (no transaction, no fee), then your Nostr signer publishes it. The message and the announcement are public.</p>"#
            )
        } else if state.btc.is_some() {
            format!(
                r#"<p class="needs"><span class="needs-chip needs-chip--btc">₿ Bitcoin wallet</span>Only the holder ({}) can bind a key.</p>"#,
                esc(&short(&tree.institution.holder))
            )
        } else {
            r#"<div class="actions"><button class="btn" data-act="connect" data-layer="btc">Connect Bitcoin wallet</button></div><p class="explain">The wallet that holds the institution can bind a Nostr key to it.</p>"#.to_string()
        };
        return format!(
            r#"<div class="card"><h2 class="card-title">No Nostr key is bound to this institution</h2><p class="sub">Nothing signed by its holder was found. {relays}.</p>{rejected}{action}</div>"#
        );
    };

    let binding = &membership.binding;
    let mut out = format!(
        r#"<div class="card"><h2 class="card-title">Issuer</h2><dl class="facts"><div class="fact"><dt>Nostr key</dt><dd>{}</dd></div><div class="fact"><dt>Vouched for by</dt><dd>{} {} <span class="badge badge-live">✓ current holder</span></dd></div><div class="fact"><dt>Since</dt><dd>{}</dd></div><div class="fact"><dt>Relays</dt><dd>{relays}</dd></div></dl>{rejected}</div>"#,
        copyable(&npub(&binding.issuer), &short(&npub(&binding.issuer))),
        copyable(&binding.holder, &short(&binding.holder)),
        ext_link(&btc_address_url(&binding.holder), "Address"),
        date(binding.issued),
    );

    if membership.roles.is_empty() {
        out.push_str(r#"<div class="card"><p class="sub">No roles have been defined yet.</p></div>"#);
    }
    for role in &membership.roles {
        out.push_str(&role_card(role, &inst_id, holds, disabled));
    }
    if holds {
        out.push_str(&format!(
            r#"<div class="card"><h2 class="card-title">New role</h2><label class="field"><span>Name</span><input id="r-name" type="text" maxlength="40" placeholder="e.g. Treasurer"{disabled}></label><label class="field"><span>Description <em>optional</em></span><input id="r-desc" type="text" maxlength="140" placeholder="What can this role do?"{disabled}></label><div class="actions"><button class="btn primary" data-act="define-role" data-inst="{inst_id}"{disabled}>Publish role</button></div><p class="explain">Signed by the bound Nostr key in your signer extension. Roles and members are public.</p></div>"#
        ));
    }
    out.push_str(&summary(membership));
    out
}

fn role_card(role: &Role, inst_id: &str, holds: bool, disabled: &str) -> String {
    let slug = esc(&role.slug);
    let mut members = String::new();
    for member in &role.members {
        let key = npub(&member.pubkey);
        let action = if !holds {
            String::new()
        } else if member.revoked {
            format!(
                r#"<button class="btn small" data-act="reinstate-role" data-inst="{inst_id}" data-slug="{slug}" data-member="{}"{disabled}>Reinstate</button>"#,
                esc(&member.pubkey)
            )
        } else {
            format!(
                r#"<button class="btn small" data-act="revoke-role" data-inst="{inst_id}" data-slug="{slug}" data-member="{}"{disabled}>Revoke</button>"#,
                esc(&member.pubkey)
            )
        };
        members.push_str(&format!(
            r#"<li class="member{}"><span class="member-key">{}</span><span class="muted">since {}</span>{}{action}</li>"#,
            if member.revoked { " is-revoked" } else { "" },
            copyable(&key, &short(&key)),
            date(member.awarded_at),
            if member.revoked { r#"<span class="badge badge-warn">Revoked</span>"# } else { "" },
        ));
    }
    if role.members.is_empty() {
        members.push_str(r#"<li class="member muted">Nobody holds this role yet.</li>"#);
    }
    let award = if holds {
        format!(
            r#"<div class="award"><input id="award-{slug}" type="text" placeholder="npub1… of the new member" aria-label="Member key for {}"{disabled}><button class="btn small" data-act="award-role" data-inst="{inst_id}" data-slug="{slug}"{disabled}>Award</button></div>"#,
            esc(&role.name)
        )
    } else {
        String::new()
    };
    format!(
        r#"<div class="card role"><h2 class="card-title">{}</h2>{}<ul class="members">{members}</ul>{award}</div>"#,
        esc(&role.name),
        if role.description.is_empty() { String::new() } else { format!(r#"<p class="sub">{}</p>"#, esc(&role.description)) },
    )
}

fn summary(membership: &Membership) -> String {
    let active: usize = membership.roles.iter().map(|r| r.members.iter().filter(|m| !m.revoked).count()).sum();
    format!(
        r#"<p class="fineprint">{} role{}, {} active membership{}. Every event shown was checked: its signature, its author, and the holder's signature behind that author.</p>"#,
        membership.roles.len(),
        if membership.roles.len() == 1 { "" } else { "s" },
        active,
        if active == 1 { "" } else { "s" },
    )
}

fn explainer() -> String {
    r#"<div class="card"><h2 class="card-title">How this is verified</h2><ol class="verify"><li><strong>Bitcoin says who holds the institution.</strong> The holder address comes from the inscription indexer.</li><li><strong>The holder vouches for a Nostr key.</strong> Their wallet signs a BIP-322 message naming the key and the inscription. A copied signature fails, because the message names one key.</li><li><strong>That key issues roles.</strong> Roles are NIP-58 badge definitions; members receive badge awards. Events from any other key are ignored.</li><li><strong>Revocation is a BINST convention.</strong> NIP-58 has none, so the same key publishes a replaceable revocation list.</li></ol><p class="explain">If the institution changes hands, the binding stops verifying until the new holder signs one. Relays can withhold events, so an empty answer is not proof that nothing was published.</p></div>"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Institution, Source, Stage};
    use crate::nostr::{Binding, Member};
    use crate::state::BtcSession;

    const ID: &str = "8c511a203232ace2bf5c643a29b329720da1d840a01a94b0dc7cc3ae65c6145di0";
    const ISSUER: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    const MEMBER: &str = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

    fn state(view: Option<MembersView>) -> AppState {
        let mut s = AppState::new(ID);
        s.trees.insert(
            ID.into(),
            Load::Ready(Tree {
                institution: Institution {
                    id: ID.into(), name: "Acme".into(), description: String::new(), website: String::new(),
                    admin: String::new(), holder: "tb1pholder".into(), pending: false, height: 0,
                },
                definitions: vec![],
                definitions_stage: Stage::Ready,
                source: Source::Live,
                notes: vec![],
            }),
        );
        if let Some(view) = view {
            s.members.insert(ID.into(), Load::Ready(view));
        }
        s
    }

    fn bound() -> MembersView {
        MembersView {
            membership: Some(Membership {
                binding: Binding { issuer: ISSUER.into(), holder: "tb1pholder".into(), issued: 1_794_000_000 },
                roles: vec![Role {
                    slug: "board".into(),
                    name: "Board <b>".into(),
                    description: String::new(),
                    members: vec![
                        Member { pubkey: MEMBER.into(), awarded_at: 1_794_000_100, revoked: false },
                        Member { pubkey: ISSUER.into(), awarded_at: 1_794_000_200, revoked: true },
                    ],
                }],
            }),
            rejected: vec!["the binding was signed by tb1pold, which does not hold the institution now".into()],
            asked: 3,
            answered: 2,
        }
    }

    #[test]
    fn asking_the_relays_shows_progress() {
        assert!(page(&state(None), ID).contains("Asking the Nostr relays"));
    }

    #[test]
    fn an_unbound_institution_offers_binding_only_to_its_holder() {
        let view = MembersView { membership: None, rejected: vec![], asked: 3, answered: 3 };
        let mut s = state(Some(view));
        let html = page(&s, ID);
        assert!(html.contains("No Nostr key is bound"));
        assert!(html.contains("3 of 3 relays answered"));
        assert!(!html.contains(r#"data-act="bind-nostr""#));
        s.btc = Some(BtcSession { address: "tb1pother".into(), ..BtcSession::default() });
        assert!(page(&s, ID).contains("Only the holder"));
        s.btc = Some(BtcSession { address: "tb1pholder".into(), ..BtcSession::default() });
        assert!(page(&s, ID).contains(r#"data-act="bind-nostr""#));
    }

    #[test]
    fn roles_and_members_are_listed_with_their_state_and_escaped() {
        let html = page(&state(Some(bound())), ID);
        assert!(html.contains("Board &lt;b&gt;"));
        assert!(html.contains("npub1"));
        assert!(html.contains("Revoked"));
        assert!(html.contains("1 announcement was ignored"));
        assert!(html.contains("1 role, 1 active membership."));
        // A visitor sees no management controls.
        assert!(!html.contains(r#"data-act="award-role""#) && !html.contains(r#"data-act="revoke-role""#));
    }

    #[test]
    fn the_holder_gets_award_revoke_and_new_role_controls() {
        let mut s = state(Some(bound()));
        s.btc = Some(BtcSession { address: "tb1pholder".into(), ..BtcSession::default() });
        let html = page(&s, ID);
        assert!(html.contains(r#"data-act="award-role""#));
        assert!(html.contains(r#"data-act="revoke-role""#));
        assert!(html.contains(r#"data-act="reinstate-role""#));
        assert!(html.contains(r#"data-act="define-role""#));
        s.members_busy = Some("Publishing…".into());
        let html = page(&s, ID);
        assert!(html.contains(r#"data-act="define-role" data-inst="8c511a203232ace2bf5c643a29b329720da1d840a01a94b0dc7cc3ae65c6145di0" disabled"#));
        assert!(html.contains("spinner"));
    }

    #[test]
    fn errors_are_shown_in_place() {
        let mut s = state(Some(bound()));
        s.members_error = Some("Member key: that npub is not valid.".into());
        assert!(page(&s, ID).contains(r#"role="alert">Member key"#));
    }
}
