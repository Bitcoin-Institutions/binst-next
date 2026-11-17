//! The side panel: details of the selected node with its actions in place,
//! and the define-process form.

use crate::model::{short, Definition, Instance, NodeRef, Provenance, Tree};
use crate::render::common::{btc_address_url, btc_tx_url, citrea_address_url, copyable, esc, ext_link, txid_of};
use crate::state::{AppState, Busy, DefineDraft};

const ACTIONS: [(&str, &str); 4] =
    [("approval", "Approval"), ("signature", "Signature"), ("payment", "Payment"), ("verification", "Verification")];

fn panel(kicker: &str, title: &str, body: &str) -> String {
    format!(
        r#"<div class="sheet-panel" role="complementary" aria-label="{kicker}"><header class="sheet-head"><div><p class="kicker">{kicker}</p><h2>{}</h2></div><button class="icon-btn" data-act="close-detail" aria-label="Close panel">×</button></header><div class="sheet-body">{body}</div></div>"#,
        esc(title)
    )
}

fn fact(label: &str, value_html: &str) -> String {
    format!(r#"<div class="fact"><dt>{label}</dt><dd>{value_html}</dd></div>"#)
}

fn needs(layer: &str, text: &str) -> String {
    format!(r#"<p class="needs"><span class="needs-chip needs-chip--{layer}">{}</span>{text}</p>"#, if layer == "btc" { "₿ Bitcoin wallet" } else { "⬡ Citrea wallet" })
}

pub fn detail(state: &AppState, node: &NodeRef) -> String {
    match node {
        NodeRef::Institution(id) => state.tree(id).map(|tree| institution(state, tree)).unwrap_or_default(),
        NodeRef::Definition { institution, id } => state
            .tree(institution)
            .and_then(|tree| tree.definition(id).map(|def| definition(state, tree, def)))
            .unwrap_or_default(),
        NodeRef::Instance { institution, address } => state
            .tree(institution)
            .and_then(|tree| tree.instance(address).map(|(def, inst)| instance(state, tree, def, inst)))
            .unwrap_or_default(),
    }
}

fn institution(state: &AppState, tree: &Tree) -> String {
    let inst = &tree.institution;
    let mut body = String::new();
    if !inst.description.is_empty() {
        body.push_str(&format!(r#"<p class="lede">{}</p>"#, esc(&inst.description)));
    }
    body.push_str(r#"<dl class="facts">"#);
    body.push_str(&fact(
        "Inscription",
        &format!("{} {}", copyable(&inst.id, &short(&inst.id)), ext_link(&btc_tx_url(txid_of(&inst.id)), "Reveal transaction")),
    ));
    if !inst.holder.is_empty() {
        body.push_str(&fact(
            "Held by",
            &format!("{} {}", copyable(&inst.holder, &short(&inst.holder)), ext_link(&btc_address_url(&inst.holder), "Address")),
        ));
    }
    if !inst.admin.is_empty() {
        body.push_str(&fact("Admin key (declared)", &copyable(&inst.admin, &short(&inst.admin))));
    }
    if inst.website.starts_with("https://") {
        body.push_str(&fact("Website", &ext_link(&esc(&inst.website), &inst.website)));
    }
    body.push_str("</dl>");
    if !inst.pending {
        body.push_str(&format!(
            r##"<div class="actions"><a class="btn" href="#/i/{}/members">Members and roles</a></div>"##,
            esc(&inst.id)
        ));
    }
    body.push_str(r#"<p class="explain">Whoever holds this inscription controls the institution. A process definition is its child: the child's reveal must spend the institution's sat, and the indexer checks that.</p>"#);

    let inst_id = esc(&inst.id);
    if inst.pending {
        body.push_str(r#"<p class="needs">This institution is still in the mempool. You can define processes once it has one confirmation.</p>"#);
    } else if state.holds(tree) {
        body.push_str(&format!(
            r#"<div class="actions"><button class="btn primary" data-act="define-open" data-inst="{inst_id}">Define a process</button></div>"#
        ));
        body.push_str(&needs("btc", "Two signatures: the commit, then the institution's input in the reveal."));
    } else if state.btc.is_some() {
        body.push_str(&format!(
            r#"<div class="actions"><button class="btn" disabled>Define a process</button></div>{}"#,
            needs("btc", &format!("Only the holder can define processes. Your wallet is not {}.", esc(&short(&inst.holder))))
        ));
    } else {
        body.push_str(&format!(
            r#"<div class="actions"><button class="btn" data-act="connect" data-layer="btc">Connect Bitcoin wallet</button></div>{}"#,
            needs("btc", "Connect the wallet that holds this institution to define processes.")
        ));
    }
    panel("Institution", &inst.name, &body)
}

fn step_list(def: &Definition, instance: Option<&Instance>) -> String {
    let mut out = String::from(r#"<ol class="steps">"#);
    for (index, step) in def.steps.iter().enumerate() {
        let (class, status) = match instance {
            Some(i) if i.completed || (index as u32) < i.current_step => ("is-done", "Done"),
            Some(i) if index as u32 == i.current_step => ("is-current", "Next"),
            Some(_) => ("", "Waiting"),
            None => ("", ""),
        };
        out.push_str(&format!(
            r#"<li class="step {class}"><span class="step-n">{}</span><span class="step-name">{}</span><span class="chip">{}</span>{}</li>"#,
            index + 1,
            esc(&step.name),
            esc(&step.action),
            if status.is_empty() { String::new() } else { format!(r#"<span class="step-status">{status}</span>"#) }
        ));
    }
    out.push_str("</ol>");
    out
}

fn definition(state: &AppState, tree: &Tree, def: &Definition) -> String {
    let mut body = String::new();
    if !def.description.is_empty() {
        body.push_str(&format!(r#"<p class="lede">{}</p>"#, esc(&def.description)));
    }
    body.push_str(&step_list(def, None));
    body.push_str(r#"<dl class="facts">"#);
    body.push_str(&fact(
        "Inscription",
        &format!("{} {}", copyable(&def.id, &short(&def.id)), ext_link(&btc_tx_url(txid_of(&def.id)), "Reveal transaction")),
    ));
    body.push_str(&fact(
        "Provenance",
        match def.provenance {
            Provenance::Verified => r#"<span class="badge badge-live">✓ Verified child</span> its reveal spent the institution's sat"#,
            Provenance::Declared => r#"<span class="badge badge-warn">⚠ Declared, unverified</span> its body names this institution, but its reveal did not spend the institution's sat"#,
            Provenance::Pending => r#"<span class="badge badge-pending">Pending</span> broadcast from this browser, not indexed yet"#,
        },
    ));
    body.push_str(&fact("Instances", &def.instances.len().to_string()));
    body.push_str("</dl>");

    let (inst_id, def_id) = (esc(&tree.institution.id), esc(&def.id));
    if def.provenance == Provenance::Pending {
        body.push_str(r#"<p class="needs">Instances can be started once this definition confirms and the indexer lists it.</p>"#);
    } else {
        let busy = matches!(&state.busy, Some(Busy::StartingInstance(id)) if *id == def.id);
        body.push_str(&format!(
            r#"<div class="actions"><button class="btn primary" data-act="start-instance" data-inst="{inst_id}" data-id="{def_id}"{}>{}</button></div>"#,
            if busy || state.busy.is_some() { " disabled" } else { "" },
            if busy { "Waiting for your wallet…" } else { "Start an instance" }
        ));
        body.push_str(&needs("evm", "One transaction on Citrea testnet."));
        body.push_str(r#"<p class="explain">On this pilot factory anyone can start an instance. Requiring the institution's Bitcoin key is in the Lab.</p>"#);
    }
    panel("Process definition", &def.name, &body)
}

fn instance(state: &AppState, tree: &Tree, def: &Definition, inst: &Instance) -> String {
    let mut body = format!(
        r#"<p class="lede">{}</p>"#,
        if inst.completed {
            "Every step is done.".to_string()
        } else {
            format!("Step {} of {} is next.", inst.current_step + 1, inst.total_steps.max(1))
        }
    );
    body.push_str(&step_list(def, Some(inst)));
    body.push_str(r#"<dl class="facts">"#);
    body.push_str(&fact(
        "Contract",
        &format!("{} {}", copyable(&inst.address, &short(&inst.address)), ext_link(&citrea_address_url(&inst.address), "Citrea explorer")),
    ));
    if !inst.creator.is_empty() {
        body.push_str(&fact("Started by", &copyable(&inst.creator, &short(&inst.creator))));
    }
    body.push_str(&fact("Definition", &esc(&def.name)));
    body.push_str(&fact(
        "Share",
        &copyable(&format!("#/x/{}", inst.address), "Link to this instance"),
    ));
    body.push_str("</dl>");

    if !inst.completed {
        let busy = matches!(&state.busy, Some(Busy::ExecutingStep(a)) if *a == inst.address);
        let next = def.steps.get(inst.current_step as usize).map(|s| s.name.as_str()).unwrap_or("next step");
        body.push_str(&format!(
            r#"<div class="actions"><button class="btn primary" data-act="execute-step" data-inst="{}" data-def="{}" data-id="{}"{}>{}</button></div>"#,
            esc(&tree.institution.id),
            esc(&def.id),
            esc(&inst.address),
            if busy || state.busy.is_some() { " disabled" } else { "" },
            if busy { "Waiting for your wallet and the chain…".to_string() } else { format!("Execute “{}”", esc(next)) }
        ));
        body.push_str(&needs("evm", "One transaction on Citrea testnet."));
        body.push_str(r#"<p class="explain">On this pilot factory anyone can execute a step.</p>"#);
    }
    panel("Instance", &def.name, &body)
}

/// The define-process form. Inputs are read when a button is pressed, so
/// typing never re-renders the form.
pub fn define_form(state: &AppState, draft: &DefineDraft) -> String {
    let name = state.tree(&draft.institution).map(|t| t.institution.name.clone()).unwrap_or_default();
    let disabled = if draft.busy { " disabled" } else { "" };
    let mut steps = String::new();
    for (index, step) in draft.steps.iter().enumerate() {
        let options: String = ACTIONS
            .iter()
            .map(|(value, label)| {
                format!(r#"<option value="{value}"{}>{label}</option>"#, if *value == step.action { " selected" } else { "" })
            })
            .collect();
        steps.push_str(&format!(
            r#"<div class="step-row"><span class="step-n">{n}</span><input class="d-step-name" type="text" maxlength="60" placeholder="Step name" aria-label="Step {n} name" value="{}"{disabled}><select class="d-step-action" aria-label="Step {n} action"{disabled}>{options}</select><button class="icon-btn" data-act="define-remove" data-index="{index}" aria-label="Remove step {n}"{}>×</button></div>"#,
            esc(&step.name),
            if draft.busy || draft.steps.len() == 1 { " disabled" } else { "" },
            n = index + 1,
        ));
    }
    let error = draft.error.as_ref().map(|e| format!(r#"<p class="form-error" role="alert">{}</p>"#, esc(e))).unwrap_or_default();
    let progress = if draft.busy { format!(r#"<p class="progress" role="status"><i class="spinner"></i>{}</p>"#, esc(&draft.progress)) } else { String::new() };
    let fee = if state.create.fee_rate > 0 { format!("{} sat/vB", state.create.fee_rate) } else { "estimating…".to_string() };
    let body = format!(
        r#"<p class="lede">A process definition is inscribed as a child of <strong>{}</strong>.</p>
<label class="field"><span>Name</span><input id="d-name" type="text" maxlength="80" placeholder="e.g. Member onboarding" value="{}"{disabled}></label>
<label class="field"><span>Description <em>optional</em></span><textarea id="d-desc" rows="2" maxlength="280"{disabled}>{}</textarea></label>
<div class="field"><span>Steps, in order</span>{steps}<button class="btn small" data-act="define-add"{disabled}>+ Add step</button></div>
{error}{progress}
<div class="actions"><button class="btn primary" data-act="define-submit"{disabled}>Inscribe definition</button><button class="btn" data-act="define-close"{disabled}>Cancel</button></div>
{}<p class="explain">Fee rate {fee}. The institution's UTXO is spent in the reveal and returned to your address, which is what makes this a verified child.</p>"#,
        esc(&name),
        esc(&draft.name),
        esc(&draft.description),
        needs("btc", "Two signatures: the commit, then the institution's input in the reveal."),
    );
    panel("New process definition", "Define a process", &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Institution, Load, Source, Stage, Step};
    use crate::state::{BtcSession, EvmSession};

    const ID: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
    const DEF: &str = "505eb39ed86053d5568a23bd128cd153142a21de02326e15354cb70059901e40i0";
    const ADDR: &str = "0x00000000000000000000000000000000000000a1";

    fn state() -> AppState {
        let mut s = AppState::new(ID);
        s.trees.insert(
            ID.into(),
            Load::Ready(Tree {
                institution: Institution {
                    id: ID.into(), name: "Acme".into(), description: "<i>x</i>".into(), website: "javascript:alert(1)".into(),
                    admin: "ab".repeat(32), holder: "tb1pholder".into(), pending: false, height: 0,
                },
                definitions: vec![Definition {
                    id: DEF.into(), name: "KYC".into(), description: String::new(),
                    steps: vec![
                        Step { name: "Review".into(), action: "approval".into() },
                        Step { name: "Sign".into(), action: "signature".into() },
                    ],
                    provenance: Provenance::Verified,
                    instances: vec![Instance { address: ADDR.into(), creator: String::new(), current_step: 1, total_steps: 2, completed: false }],
                    instances_stage: Stage::Ready,
                }],
                definitions_stage: Stage::Ready,
                source: Source::Live,
                notes: vec![],
            }),
        );
        s
    }

    #[test]
    fn institution_panel_escapes_and_drops_unsafe_links() {
        let html = detail(&state(), &NodeRef::Institution(ID.into()));
        assert!(html.contains("&lt;i&gt;x&lt;/i&gt;"));
        assert!(!html.contains("javascript:"));
    }

    #[test]
    fn define_is_offered_only_to_the_holder() {
        let mut s = state();
        let node = NodeRef::Institution(ID.into());
        assert!(detail(&s, &node).contains(r#"data-act="connect" data-layer="btc""#));
        s.btc = Some(BtcSession { address: "tb1pother".into(), ..BtcSession::default() });
        let html = detail(&s, &node);
        assert!(html.contains("Only the holder"));
        assert!(!html.contains(r#"data-act="define-open""#));
        s.btc = Some(BtcSession { address: "tb1pholder".into(), ..BtcSession::default() });
        assert!(detail(&s, &node).contains(r#"data-act="define-open""#));
    }

    #[test]
    fn instance_panel_marks_steps_and_offers_the_next_one() {
        let mut s = state();
        s.evm = Some(EvmSession { address: "0x1".into(), network: "citrea-testnet".into(), ..EvmSession::default() });
        let html = detail(&s, &NodeRef::Instance { institution: ID.into(), address: ADDR.into() });
        assert!(html.contains("is-done"));
        assert!(html.contains("is-current"));
        assert!(html.contains("Execute “Sign”"));
        assert!(html.contains(r#"data-act="execute-step""#));
    }

    #[test]
    fn busy_disables_the_action_and_says_why() {
        let mut s = state();
        s.busy = Some(Busy::ExecutingStep(ADDR.into()));
        let html = detail(&s, &NodeRef::Instance { institution: ID.into(), address: ADDR.into() });
        assert!(html.contains("disabled"));
        assert!(html.contains("Waiting for your wallet"));
    }

    #[test]
    fn define_form_keeps_typed_values_and_locks_while_busy() {
        let s = state();
        let mut draft = DefineDraft {
            institution: ID.into(),
            name: "On\"boarding".into(),
            steps: vec![Step { name: "A".into(), action: "payment".into() }],
            ..DefineDraft::default()
        };
        let html = define_form(&s, &draft);
        assert!(html.contains(r#"value="On&quot;boarding""#));
        assert!(html.contains(r#"<option value="payment" selected>"#));
        draft.busy = true;
        draft.progress = "Signing".into();
        let html = define_form(&s, &draft);
        assert!(html.contains("spinner"));
        assert!(html.contains(r#"data-act="define-submit" disabled"#));
    }
}
