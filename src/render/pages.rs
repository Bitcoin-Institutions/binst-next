//! One function per route.

use crate::model::{short, Load, Provenance, Tree};
use crate::render::common::{btc_tx_url, copyable, error_card, esc, ext_link, txid_of};
use crate::render::tree::{self, TreeOptions};
use crate::router::Route;
use crate::state::{AppState, CreateStep};

pub fn view(state: &AppState) -> String {
    match &state.route {
        Route::Home => home(state),
        Route::Institutions => institutions(state),
        Route::Institution { id, .. } => institution(state, id),
        Route::Members { id } => crate::render::members::page(state, id),
        Route::Instance { address } => format!(
            r#"<section class="page narrow"><p class="kicker">Instance</p><h1>Finding this instance…</h1><p class="sub">Reading {} on Citrea testnet and locating its institution on Bitcoin.</p></section>"#,
            esc(&short(address))
        ),
        Route::New => create(state),
        Route::Settings => settings(state),
        Route::Lab(page) => lab(state, page.as_deref()),
        Route::NotFound(path) => format!(
            r##"<section class="page narrow"><p class="kicker">Not found</p><h1>Nothing lives at this address</h1><p class="sub">“{}” is not a page of this app.</p><div class="actions"><a class="btn primary" href="#/">Go home</a><a class="btn" href="#/institutions">My institutions</a></div></section>"##,
            esc(path)
        ),
    }
}

// ── Settings ─────────────────────────────────────────────────────

fn settings(state: &AppState) -> String {
    let s = &state.settings;
    let records = crate::recovery::list();
    let keys = if records.is_empty() {
        r#"<p class="empty">No one-time keys kept in this browser.</p>"#.to_string()
    } else {
        records
            .iter()
            .rev()
            .map(|r| {
                let status = if r.reveal_txid.is_empty() && !r.reveal_hex.is_empty() {
                    "<strong>Reveal signed, not broadcast.</strong> Broadcast it again once the commit is out (copy it below, then <code>sendrawtransaction</code> on your node or mempool's Broadcast page), or import the descriptor to take the commit's sats back.".to_string()
                } else if r.reveal_txid.is_empty() {
                    "<strong>Reveal not broadcast.</strong> If the commit confirmed, import this descriptor to get its sats back.".to_string()
                } else {
                    format!("Revealed in <span class=\"mono\">{}</span>. Safe to forget once that is confirmed.", esc(&short(&r.reveal_txid)))
                };
                format!(
                    r#"<li class="recovery"><div><strong>{}</strong> · commit <span class="mono">{}</span></div><p class="fineprint">{status}</p><div class="btn-row"><button class="btn small" data-act="copy" data-text="{}">Copy recovery descriptor</button>{}<button class="btn small" data-act="forget-recovery" data-id="{}" data-revealed="{}">Forget</button></div></li>"#,
                    esc(&r.label),
                    esc(&short(&r.commit_txid)),
                    esc(&r.descriptor),
                    if r.reveal_txid.is_empty() && !r.reveal_hex.is_empty() {
                        format!(r#"<button class="btn small" data-act="copy" data-text="{}">Copy signed reveal</button>"#, esc(&r.reveal_hex))
                    } else {
                        String::new()
                    },
                    esc(&r.commit_txid),
                    if r.reveal_txid.is_empty() { "0" } else { "1" },
                )
            })
            .collect::<String>()
    };
    let wallet = match &state.btc {
        Some(b) if b.is_handoff() => format!(
            r#"<p class="sub">Bitcoin: <strong>{}</strong>, signing elsewhere. Address <span class="mono">{}</span>{}.</p>"#,
            esc(&b.wallet_name),
            esc(&b.address),
            if b.origin.is_empty() { String::new() } else { format!(", key <span class=\"mono\">{}</span>", esc(&b.origin)) }
        ),
        Some(b) => format!(r#"<p class="sub">Bitcoin: <strong>{}</strong> (browser extension).</p>"#, esc(&b.wallet_name)),
        None => String::new(),
    };
    format!(
        r#"<section class="page narrow"><p class="kicker">Settings</p><h1>Your own servers</h1><p class="sub">Leave a field empty for the default. Everything here stays in this browser.</p>{wallet}
<div class="card">
<label class="field"><span>ord server <em>optional</em></span><input id="s-ord" type="url" placeholder="http://127.0.0.1:8080" value="{ord}" spellcheck="false"><small>Replaces the UniSat indexer for institution trees. Run <code>ord --testnet4 --index-addresses server</code>; ord already sends <code>Access-Control-Allow-Origin: *</code>. Default: UniSat through this site's <code>/api/indexer</code>.</small></label>
<label class="field"><span>Chain API (esplora) <em>optional</em></span><input id="s-esplora" type="url" placeholder="{esplora_default}" value="{esplora}" spellcheck="false"><small>UTXOs, transactions, fees and broadcast. Your own mempool or esplora works; it must allow this site's origin (CORS). Browsers cannot reach an Electrum server directly.</small></label>
<label class="field"><span>Citrea RPC <em>optional</em></span><input id="s-citrea" type="url" placeholder="https://rpc.testnet.citrea.xyz" value="{citrea}" spellcheck="false"><small>Reads only (instances, steps). Transactions still go through your Citrea wallet.</small></label>
<div class="actions"><button class="btn primary" data-act="save-settings">Save</button></div>
</div>
<h2 class="section-title">One-time reveal keys</h2><p class="sub">Each inscription locks its commit output to a fresh key that only this browser holds, so your wallet never has to sign the inscription script. The key controls only that output (the reveal fee plus 546 sats). If a reveal never confirms, import the descriptor into Bitcoin Core (<code>importdescriptors</code>) or Sparrow to spend it back.</p>
<ul class="recovery-list">{keys}</ul></section>"#,
        ord = esc(&s.ord_url),
        esplora = esc(&s.esplora_url),
        esplora_default = crate::settings::DEFAULT_ESPLORA,
        citrea = esc(&s.citrea_rpc),
    )
}

// ── Home ─────────────────────────────────────────────────────────

/// What the 3D scene draws, as JSON for `#hero-scene[data-tree]`.
fn hero_data(tree: &Tree) -> String {
    let defs: Vec<serde_json::Value> = tree
        .definitions
        .iter()
        .map(|def| {
            let instances: Vec<serde_json::Value> = def
                .instances
                .iter()
                .map(|i| {
                    let progress = if i.completed {
                        1.0
                    } else if i.total_steps == 0 {
                        0.0
                    } else {
                        (i.current_step as f64 / i.total_steps as f64).clamp(0.0, 1.0)
                    };
                    serde_json::json!({ "progress": progress, "done": i.completed })
                })
                .collect();
            serde_json::json!({
                "name": def.name,
                "verified": def.provenance == Provenance::Verified,
                "instances": instances,
            })
        })
        .collect();
    serde_json::json!({ "name": tree.institution.name, "height": tree.institution.height, "defs": defs }).to_string()
}

/// Figures read from the featured tree. Nothing here is estimated.
fn hero_stats(tree: &Tree) -> String {
    let definitions = tree.definitions.len();
    let verified = tree.definitions.iter().filter(|d| d.provenance == Provenance::Verified).count();
    let instances: usize = tree.definitions.iter().map(|d| d.instances.len()).sum();
    let (mut done, mut total) = (0u32, 0u32);
    for instance in tree.definitions.iter().flat_map(|d| d.instances.iter()) {
        total += instance.total_steps;
        done += if instance.completed { instance.total_steps } else { instance.current_step.min(instance.total_steps) };
    }
    let block = if tree.institution.height > 0 {
        format!(
            r#"<div class="stat"><dt>Inscribed in block</dt><dd>{}</dd><small>Bitcoin testnet4</small></div>"#,
            tree.institution.height
        )
    } else {
        String::new()
    };
    format!(
        r#"<dl class="stats">{block}<div class="stat"><dt>Process definitions</dt><dd>{definitions}</dd><small>{verified} verified on Bitcoin data</small></div><div class="stat"><dt>Instances on Citrea</dt><dd>{instances}</dd><small>testnet, chain 5115</small></div><div class="stat"><dt>Steps executed</dt><dd>{done}<span>/{total}</span></dd><small>across those instances</small></div></dl>"#
    )
}

fn home(state: &AppState) -> String {
    let featured = esc(&state.featured);
    let (stage, stats) = match state.trees.get(&state.featured) {
        Some(Load::Ready(tree)) => (
            format!(
                r##"<a class="hero-stage" href="#/i/{featured}" aria-label="Open the featured institution: {name}"><div id="hero-scene" data-tree="{data}"></div><div class="hero-fallback">{svg}</div></a><p class="hero-caption">{badge}<span>“{name}” is a real institution. The tree, its block and its progress are live; the motion is illustrative.</span></p>"##,
                name = esc(&tree.institution.name),
                data = esc(&hero_data(tree)),
                svg = tree::svg(tree, &TreeOptions { interactive: false, zoom: 100, selected: None }),
                badge = tree::source_badge(&tree.source),
            ),
            hero_stats(tree),
        ),
        Some(Load::Failed(error)) => (error_card(error, "reload-tree", &format!(r#"data-id="{featured}""#)), String::new()),
        _ => (r#"<div class="hero-stage is-loading" aria-busy="true"><div class="tree-skeleton"></div></div>"#.to_string(), String::new()),
    };
    format!(
        r##"<section class="hero">
<div class="hero-copy">
<p class="kicker">Bitcoin Institutions · pilot on testnet4 and Citrea testnet</p>
<h1>Institutions that <em>live on Bitcoin</em>.</h1>
<p class="lede">An institution is an inscription you hold. Its processes are child inscriptions. Each instance of a process executes on an L2 that can be replaced without touching the institution.</p>
<div class="actions"><a class="btn primary large" href="#/i/{featured}">Explore a live institution</a><a class="btn large" href="#/new">Create yours</a></div>
<p class="fineprint">No wallet needed to explore. Unaudited pilot: test coins only.</p>
</div>
<div class="hero-visual">{stage}</div>
</section>
{stats}

<section class="band reveal">
<p class="kicker">The model</p>
<h2>Three levels, two chains</h2>
<ol class="how">
<li class="reveal" style="--i:0"><span class="how-n">1</span><p class="how-layer how-layer--btc">₿ Bitcoin testnet4</p><h3>Inscribe the identity</h3><p>One commit and one reveal create the institution. Whoever holds that inscription controls it.</p></li>
<li class="reveal" style="--i:1"><span class="how-n">2</span><p class="how-layer how-layer--btc">₿ Bitcoin testnet4</p><h3>Define its processes</h3><p>Each process definition is a child inscription. Its reveal spends the institution's sat, so the link is checked on Bitcoin data, not declared.</p></li>
<li class="reveal" style="--i:2"><span class="how-n">3</span><p class="how-layer how-layer--evm">⬡ Citrea testnet</p><h3>Run instances on L2</h3><p>Instances of a process execute step by step on Citrea. The L2 holds only execution state.</p></li>
</ol>
</section>

<section class="band reveal">
<p class="kicker">Honest stage</p>
<h2>What enforces what</h2>
<div class="table-wrap"><table class="trust">
<thead><tr><th>Claim</th><th>Enforced by</th><th>Stage today</th></tr></thead>
<tbody>
<tr><td>The institution exists and has this content</td><td>Bitcoin (the inscription's witness data)</td><td><span class="badge badge-live">Live on testnet4</span></td></tr>
<tr><td>Who controls the institution</td><td>Bitcoin (whoever can spend the inscription's UTXO)</td><td><span class="badge badge-live">Live on testnet4</span></td></tr>
<tr><td>A process definition belongs to the institution</td><td>The indexer's rule: the child's reveal spent the parent's sat. Not a consensus rule.</td><td><span class="badge badge-warn">Depends on the indexer</span></td></tr>
<tr><td>Finding institutions and their definitions</td><td>The UniSat indexer API</td><td><span class="badge badge-warn">Trusted third party</span></td></tr>
<tr><td>Instance state and step order</td><td>The Citrea contract</td><td><span class="badge badge-live">Live on Citrea testnet</span></td></tr>
<tr><td>Who may execute a step</td><td>Nobody yet: the pilot factory is open to anyone</td><td><span class="badge badge-pending">Open gap · see Lab</span></td></tr>
</tbody></table></div>
</section>

<section class="band reveal">
<p class="kicker">Frontier</p>
<h2>Lab</h2>
<p class="sub">Work on test networks that extends what an institution can do. Each card says what is real today.</p>
{}
</section>"##,
        lab_cards()
    )
}

// ── My institutions ──────────────────────────────────────────────

fn institutions(state: &AppState) -> String {
    let mut out = String::from(
        r##"<section class="page"><header class="page-head"><div><p class="kicker">Bitcoin testnet4</p><h1>My institutions</h1><p class="sub">Each institution is a tree: its process definitions below it, their instances on the lowest level.</p></div><a class="btn primary" href="#/new">+ New institution</a></header>"##,
    );
    out.push_str(&view_toolbar(state, None));

    let mut shown_featured = false;
    match &state.mine {
        Load::Idle | Load::Loading => {
            out.push_str(r#"<div class="tree-panel is-loading" aria-busy="true"><div class="tree-skeleton"></div></div>"#);
        }
        Load::Failed(error) => out.push_str(&error_card(error, "reload-mine", "")),
        Load::Ready(ids) => {
            if state.btc.is_none() {
                out.push_str(
                    r#"<div class="callout"><div><strong>Connect a Bitcoin wallet to list the institutions it holds.</strong><p>Institutions created in this browser are listed either way.</p></div><button class="btn" data-act="connect" data-layer="btc">Connect Bitcoin wallet</button></div>"#,
                );
            } else if ids.is_empty() {
                out.push_str(
                    r##"<div class="callout"><div><strong>This wallet holds no institution yet.</strong><p>Creating one takes two signatures and a few hundred test sats.</p></div><a class="btn primary" href="#/new">Create an institution</a></div>"##,
                );
            }
            for id in ids {
                shown_featured |= *id == state.featured;
                out.push_str(&tree::panel(state, id, true));
            }
        }
    }
    if !shown_featured {
        out.push_str(r#"<h2 class="section-title">Featured institution</h2><p class="sub">A real institution on testnet4 that anyone can explore without a wallet.</p>"#);
        out.push_str(&tree::panel(state, &state.featured, true));
    }
    out.push_str("</section>");
    out
}

fn view_toolbar(state: &AppState, reload: Option<&str>) -> String {
    let reload = match reload {
        Some(id) => format!(r#"<button class="btn small" data-act="reload-tree" data-id="{}">Reload</button>"#, esc(id)),
        None => r#"<button class="btn small" data-act="reload-mine">Reload</button>"#.to_string(),
    };
    format!(
        r#"<div class="toolbar" role="toolbar" aria-label="Tree view"><div class="seg"><button class="seg-btn{}" data-act="view-tree" aria-pressed="{}">Tree</button><button class="seg-btn{}" data-act="view-list" aria-pressed="{}">List</button></div><div class="seg"><button class="seg-btn" data-act="zoom" data-delta="-20" aria-label="Zoom out"{dis}>−</button><button class="seg-btn" data-act="zoom" data-delta="0" aria-label="Reset zoom"{dis}>{}%</button><button class="seg-btn" data-act="zoom" data-delta="20" aria-label="Zoom in"{dis}>+</button></div>{reload}</div>"#,
        if state.list_view { "" } else { " is-on" },
        !state.list_view,
        if state.list_view { " is-on" } else { "" },
        state.list_view,
        state.zoom,
        dis = if state.list_view { " disabled" } else { "" },
    )
}

// ── One institution ──────────────────────────────────────────────

fn institution(state: &AppState, id: &str) -> String {
    format!(
        r##"<section class="page"><nav class="crumbs" aria-label="Breadcrumb"><a href="#/institutions">My institutions</a><span>/</span><span>{}</span></nav>{}{}</section>"##,
        esc(&state.tree(id).map(|t| t.institution.name.clone()).unwrap_or_else(|| short(id))),
        view_toolbar(state, Some(id)),
        tree::panel(state, id, false)
    )
}

// ── Create institution ───────────────────────────────────────────

fn stepper(current: CreateStep) -> String {
    let steps = [(CreateStep::Details, "Details"), (CreateStep::Review, "Review"), (CreateStep::Signing, "Sign"), (CreateStep::Done, "Done")];
    let position = steps.iter().position(|(s, _)| *s == current).unwrap_or(0);
    let mut out = String::from(r#"<ol class="stepper" aria-label="Progress">"#);
    for (index, (_, label)) in steps.iter().enumerate() {
        let class = if index < position { "is-done" } else if index == position { "is-current" } else { "" };
        out.push_str(&format!(
            r#"<li class="{class}"{}><span class="stepper-n">{}</span>{label}</li>"#,
            if index == position { r#" aria-current="step""# } else { "" },
            index + 1
        ));
    }
    out.push_str("</ol>");
    out
}

fn create(state: &AppState) -> String {
    let c = &state.create;
    let error = c.error.as_ref().map(|e| format!(r#"<p class="form-error" role="alert">{}</p>"#, esc(e))).unwrap_or_default();
    let body = match c.step {
        CreateStep::Details => format!(
            r#"<label class="field"><span>Name</span><input id="f-name" type="text" maxlength="80" placeholder="e.g. Río Cuarto Water Cooperative" value="{}" autofocus><small>Public and permanent once inscribed.</small></label>
<label class="field"><span>Description <em>optional</em></span><textarea id="f-desc" rows="3" maxlength="280" placeholder="What does this institution do?">{}</textarea></label>
<label class="field"><span>Website <em>optional</em></span><input id="f-web" type="url" maxlength="120" placeholder="https://example.org" value="{}"></label>
{error}
<div class="actions"><button class="btn primary" data-act="create-review">Review</button></div>"#,
            esc(&c.name),
            esc(&c.description),
            esc(&c.website)
        ),
        CreateStep::Review => {
            let wallet = match &state.btc {
                Some(btc) => format!(
                    r#"<div class="fact"><dt>Signed and held by</dt><dd>{} <span class="muted">{}</span></dd></div>"#,
                    copyable(&btc.address, &short(&btc.address)),
                    esc(&btc.wallet_name)
                ),
                None => r#"<div class="fact"><dt>Signed and held by</dt><dd><button class="btn small" data-act="connect" data-layer="btc">Connect Bitcoin wallet</button></dd></div>"#.to_string(),
            };
            let fee = if c.fee_rate > 0 { format!("{} sat/vB (chain API estimate)", c.fee_rate) } else { "estimating…".to_string() };
            format!(
                r#"<dl class="facts">
<div class="fact"><dt>Name</dt><dd>{}</dd></div>
<div class="fact"><dt>Description</dt><dd>{}</dd></div>
<div class="fact"><dt>Website</dt><dd>{}</dd></div>
{wallet}
<div class="fact"><dt>Network</dt><dd>Bitcoin testnet4</dd></div>
<div class="fact"><dt>Fee rate</dt><dd>{fee}</dd></div>
<div class="fact"><dt>Signatures</dt><dd>1 in your wallet: the commit. The page signs the reveal with a one-time key (kept under Settings until you forget it).</dd></div>
</dl>
<p class="explain">Your wallet shows the exact amounts before you sign. The inscription lands at your address; holding it is what controls the institution.</p>
{error}
<div class="actions"><button class="btn primary" data-act="create-submit">{}</button><button class="btn" data-act="create-back">Back</button></div>"#,
                esc(&c.name),
                if c.description.is_empty() { "<span class=\"muted\">None</span>".to_string() } else { esc(&c.description) },
                if c.website.is_empty() { "<span class=\"muted\">None</span>".to_string() } else { esc(&c.website) },
                if state.btc.is_some() { "Sign and inscribe" } else { "Connect wallet to continue" },
            )
        }
        CreateStep::Signing => format!(
            r#"<p class="progress" role="status"><i class="spinner"></i>{}</p><p class="explain">Keep this tab open. Two wallet prompts appear one after the other.</p>"#,
            esc(&c.progress)
        ),
        CreateStep::Done => {
            let id = c.result_id.clone().unwrap_or_default();
            format!(
                r##"<p class="success"><span class="success-mark">✓</span>“{}” is broadcast.</p>
<dl class="facts"><div class="fact"><dt>Inscription</dt><dd>{} {}</dd></div></dl>
<p class="explain">It is in the mempool now. It shows as pending on your tree until it confirms and the indexer lists it, usually one block.</p>
<div class="actions"><a class="btn primary" href="#/i/{}">Open its tree</a><button class="btn" data-act="create-reset">Create another</button></div>"##,
                esc(&c.name),
                copyable(&id, &short(&id)),
                ext_link(&btc_tx_url(txid_of(&id)), "Reveal transaction"),
                esc(&id)
            )
        }
    };
    format!(
        r#"<section class="page narrow"><p class="kicker">Bitcoin testnet4</p><h1>Create an institution</h1><p class="sub">One inscription on Bitcoin. It becomes the root of a tree you can grow with process definitions.</p>{}<div class="card">{body}</div></section>"#,
        stepper(c.step)
    )
}

// ── Lab ──────────────────────────────────────────────────────────

struct LabCard {
    slug: &'static str,
    title: &'static str,
    badge: &'static str,
    badge_class: &'static str,
    what: &'static str,
    adds: &'static str,
}

const LAB: [LabCard; 5] = [
    LabCard {
        slug: "nostr",
        title: "Nostr members and discovery",
        badge: "Built · experimental · public Nostr relays",
        badge_class: "badge-live",
        what: "Members and roles as NIP-58 badges issued by a Nostr key that the institution's holder vouched for with a BIP-322 signature. Every event is verified before it is shown.",
        adds: "Adds membership without a token, and lists announced institutions from relays. Roles are shown, not enforced on L2.",
    },
    LabCard {
        slug: "r7",
        title: "Bitcoin key authorises L2 steps",
        badge: "Planned · needs a new factory on Citrea testnet",
        badge_class: "badge-pending",
        what: "The holder signs each step with a BIP-322 message; the contract verifies it with Citrea's Schnorr precompile.",
        adds: "Closes the gap where anyone can execute a step. The contract trusts the key fixed when the instance is created.",
    },
    LabCard {
        slug: "ark",
        title: "Ark treasury",
        badge: "Planned · signet",
        badge_class: "badge-pending",
        what: "An institution treasury as an Ark wallet on Second's signet server, running in the browser.",
        adds: "Adds cheap off-chain payments. Signet funds, separate from testnet4.",
    },
    LabCard {
        slug: "ctv",
        title: "Covenant vault",
        badge: "Planned · covenant signet",
        badge_class: "badge-pending",
        what: "A vault whose only spend path is fixed in advance with OP_CHECKTEMPLATEVERIFY, plus a recovery key.",
        adds: "Shows how control of an institution could be guarded once covenants exist. Not activated on Bitcoin.",
    },
    LabCard {
        slug: "",
        title: "BitVMX",
        badge: "Research only",
        badge_class: "badge-muted",
        what: "Disputable computation verified on Bitcoin without a soft fork. Deployed only as a bridge on Rootstock testnet.",
        adds: "Could one day let Bitcoin enforce L2 steps. No browser demo is practical today.",
    },
];

fn lab_cards() -> String {
    let mut out = String::from(r#"<div class="lab-grid">"#);
    for card in &LAB {
        let title = if card.slug.is_empty() {
            card.title.to_string()
        } else {
            format!(r##"<a href="#/lab/{}">{}</a>"##, card.slug, card.title)
        };
        out.push_str(&format!(
            r#"<article class="lab-card"><span class="badge {}">{}</span><h3>{title}</h3><p>{}</p><p class="muted">{}</p></article>"#,
            card.badge_class, card.badge, card.what, card.adds
        ));
    }
    out.push_str("</div>");
    out
}

/// Institutions found on relays. The names are the announcers' claims.
fn announced(state: &AppState) -> String {
    let body = match &state.announced {
        Load::Idle | Load::Loading => r#"<p class="progress"><i class="spinner"></i>Asking the Nostr relays…</p>"#.to_string(),
        Load::Failed(error) => error_card(error, "reload-announced", ""),
        Load::Ready(list) if list.is_empty() => {
            r#"<p class="sub">No institution has been announced on these relays yet. One appears here once its holder binds a Nostr key to it.</p>"#.to_string()
        }
        Load::Ready(list) => format!(
            r#"<ul class="members">{}</ul><p class="explain">Names are what the announcers wrote. Open one to check it: its tree comes from Bitcoin, and its members page verifies the holder's signature.</p>"#,
            list.iter()
                .map(|(id, name)| {
                    format!(
                        r##"<li class="member"><a class="member-key" href="#/i/{}">{}</a><span class="mono muted">{}</span><span class="badge badge-muted">Unverified until opened</span></li>"##,
                        esc(id),
                        if name.is_empty() { "Unnamed".to_string() } else { esc(name) },
                        esc(&short(id))
                    )
                })
                .collect::<String>()
        ),
    };
    format!(
        r#"<div class="card"><div class="page-head"><h2 class="card-title">Institutions announced on Nostr</h2><button class="btn small" data-act="reload-announced">Reload</button></div>{body}</div>"#
    )
}

fn lab(state: &AppState, page: Option<&str>) -> String {
    match page.and_then(|slug| LAB.iter().find(|c| c.slug == slug)) {
        Some(card) if card.slug == "nostr" => format!(
            r##"<section class="page narrow"><nav class="crumbs" aria-label="Breadcrumb"><a href="#/lab">Lab</a><span>/</span><span>{}</span></nav><span class="badge {}">{}</span><h1>{}</h1><p class="lede">{}</p><p class="sub">{}</p><div class="callout"><div><strong>Open any institution's tree and choose “Members and roles”.</strong><p>Reading works for everyone. Binding a key and issuing roles need the holder's Bitcoin wallet and a NIP-07 Nostr signer.</p></div><a class="btn primary" href="#/institutions">My institutions</a></div>{}</section>"##,
            card.title, card.badge_class, card.badge, card.title, card.what, card.adds, announced(state)
        ),
        Some(card) => format!(
            r##"<section class="page narrow"><nav class="crumbs" aria-label="Breadcrumb"><a href="#/lab">Lab</a><span>/</span><span>{}</span></nav><span class="badge {}">{}</span><h1>{}</h1><p class="lede">{}</p><p class="sub">{}</p><div class="callout"><div><strong>Not built yet.</strong><p>This page will host the working demo. Nothing here is simulated in the meantime.</p></div></div></section>"##,
            card.title, card.badge_class, card.badge, card.title, card.what, card.adds
        ),
        None => format!(
            r#"<section class="page"><header class="page-head"><div><p class="kicker">Test networks only</p><h1>Lab</h1><p class="sub">Technologies evaluated for replacing or extending what an institution can do. Each card says what is real today.</p></div></header>{}</section>"#,
            lab_cards()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UserError;
    use crate::state::BtcSession;

    const ID: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";

    #[test]
    fn my_institutions_without_a_wallet_shows_the_featured_tree_and_a_connect_prompt() {
        let mut s = AppState::new(ID);
        s.route = Route::Institutions;
        s.mine = Load::Ready(vec![]);
        let html = view(&s);
        assert!(html.contains("Featured institution"));
        assert!(html.contains(r#"data-act="connect" data-layer="btc""#));
    }

    #[test]
    fn an_empty_wallet_is_told_how_to_start() {
        let mut s = AppState::new(ID);
        s.route = Route::Institutions;
        s.btc = Some(BtcSession::default());
        s.mine = Load::Ready(vec![]);
        assert!(view(&s).contains("holds no institution yet"));
    }

    #[test]
    fn a_failed_listing_shows_the_reason_and_a_retry() {
        let mut s = AppState::new(ID);
        s.route = Route::Institutions;
        s.mine = Load::Failed(UserError::new("Listing your wallet's inscriptions", "indexer HTTP 429"));
        let html = view(&s);
        assert!(html.contains("indexer HTTP 429"));
        assert!(html.contains(r#"data-act="reload-mine""#));
    }

    #[test]
    fn the_featured_tree_is_not_repeated_when_the_wallet_holds_it() {
        let mut s = AppState::new(ID);
        s.route = Route::Institutions;
        s.mine = Load::Ready(vec![ID.to_string()]);
        assert!(!view(&s).contains("Featured institution"));
    }

    #[test]
    fn create_keeps_typed_values_escaped_and_reports_errors() {
        let mut s = AppState::new(ID);
        s.route = Route::New;
        s.create.name = "A \"quoted\" <name>".into();
        s.create.error = Some("Give the institution a name.".into());
        let html = view(&s);
        assert!(html.contains("A &quot;quoted&quot; &lt;name&gt;"));
        assert!(html.contains(r#"role="alert""#));
    }

    #[test]
    fn review_states_the_signatures_and_asks_for_a_wallet() {
        let mut s = AppState::new(ID);
        s.route = Route::New;
        s.create.step = CreateStep::Review;
        s.create.name = "Acme".into();
        let html = view(&s);
        assert!(html.contains("1 in your wallet: the commit"));
        assert!(html.contains("Connect wallet to continue"));
    }

    #[test]
    fn unknown_paths_are_escaped_in_the_not_found_page() {
        let mut s = AppState::new(ID);
        s.route = Route::NotFound("<img src=x>".into());
        let html = view(&s);
        assert!(html.contains("&lt;img src=x&gt;"));
        assert!(!html.contains("<img src=x>"));
    }

    fn featured_tree() -> Tree {
        use crate::model::{Definition, Instance, Institution, Source, Stage, Step};
        Tree {
            institution: Institution {
                id: ID.into(), name: "Acme \"Co\"".into(), description: String::new(), website: String::new(),
                admin: String::new(), holder: "tb1p".into(), pending: false, height: 158993,
            },
            definitions: vec![Definition {
                id: "d".into(), name: "KYC".into(), description: String::new(),
                steps: vec![Step { name: "a".into(), action: "approval".into() }],
                provenance: Provenance::Verified,
                instances: vec![
                    Instance { address: "0x1".into(), creator: String::new(), current_step: 1, total_steps: 3, completed: false },
                    Instance { address: "0x2".into(), creator: String::new(), current_step: 2, total_steps: 2, completed: true },
                ],
                instances_stage: Stage::Ready,
            }],
            definitions_stage: Stage::Ready,
            source: Source::Live,
            notes: vec![],
        }
    }

    #[test]
    fn hero_data_is_valid_json_with_real_progress() {
        let data: serde_json::Value = serde_json::from_str(&hero_data(&featured_tree())).unwrap();
        assert_eq!(data["name"], "Acme \"Co\"");
        assert_eq!(data["height"], 158993);
        assert_eq!(data["defs"][0]["verified"], true);
        assert_eq!(data["defs"][0]["instances"][1]["done"], true);
        assert!((data["defs"][0]["instances"][0]["progress"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn hero_stats_count_what_the_tree_holds() {
        let html = hero_stats(&featured_tree());
        assert!(html.contains("<dd>158993</dd>"));
        assert!(html.contains("<dd>3<span>/5</span></dd>"));
        assert!(html.contains("1 verified on Bitcoin data"));
    }

    #[test]
    fn home_embeds_the_scene_data_escaped_and_keeps_the_static_tree() {
        let mut s = AppState::new(ID);
        s.trees.insert(ID.into(), Load::Ready(featured_tree()));
        let html = view(&s);
        assert!(html.contains(r#"id="hero-scene" data-tree="{&quot;"#));
        assert!(html.contains("hero-fallback"));
        assert!(html.contains("<svg"));
        assert!(html.contains("the motion is illustrative"));
    }

    #[test]
    fn every_lab_page_says_it_is_not_built() {
        let mut s = AppState::new(ID);
        assert!(lab(&s, Some("nostr")).contains("Members and roles"));
        for slug in ["r7", "ark", "ctv"] {
            assert!(lab(&s, Some(slug)).contains("Not built yet."), "{slug}");
        }
        // Announced institutions are listed as claims, escaped, until opened.
        s.announced = Load::Ready(vec![(ID.to_string(), "<b>Acme</b>".to_string())]);
        let html = lab(&s, Some("nostr"));
        assert!(html.contains("&lt;b&gt;Acme&lt;/b&gt;") && html.contains("Unverified until opened"));
        s.announced = Load::Ready(vec![]);
        assert!(lab(&s, Some("nostr")).contains("No institution has been announced"));
    }
}
