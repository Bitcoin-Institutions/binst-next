//! The institution tree: SVG on wide screens, a nested outline otherwise.

use crate::model::{short, Definition, Instance, Load, NodeRef, Provenance, Source, Stage, Tree};
use crate::render::common::{clip, copyable, error_card, esc};
use crate::state::AppState;
use crate::tree_layout::{layout, Rect, Slot, DEF_W, LEAF_W, ROOT_W};

/// What a column on the second level holds.
enum Column<'a> {
    Definition(&'a Definition),
    /// Placeholder while definitions load.
    Skeleton,
    /// The "+ Define process" slot.
    Add,
}

/// What a box on the lowest level holds.
enum Leaf<'a> {
    Instance(&'a Instance),
    Loading,
    Failed,
    /// The "+ Start instance" slot.
    Add,
}

pub struct TreeOptions<'a> {
    /// False for the home page preview: no actions, no focus stops.
    pub interactive: bool,
    pub zoom: u32,
    pub selected: Option<&'a NodeRef>,
}

fn columns<'a>(tree: &'a Tree, interactive: bool) -> Vec<(Column<'a>, Vec<Leaf<'a>>)> {
    let mut columns: Vec<(Column, Vec<Leaf>)> = Vec::new();
    for def in &tree.definitions {
        let mut leaves: Vec<Leaf> = def.instances.iter().map(Leaf::Instance).collect();
        match &def.instances_stage {
            Stage::Loading => leaves.push(Leaf::Loading),
            Stage::Failed(_) => leaves.push(Leaf::Failed),
            Stage::Ready => {}
        }
        if interactive && def.provenance != Provenance::Pending {
            leaves.push(Leaf::Add);
        }
        columns.push((Column::Definition(def), leaves));
    }
    if tree.definitions_stage == Stage::Loading {
        columns.push((Column::Skeleton, Vec::new()));
        columns.push((Column::Skeleton, Vec::new()));
    }
    if interactive && tree.definitions_stage == Stage::Ready && !tree.institution.pending {
        columns.push((Column::Add, Vec::new()));
    }
    columns
}

// ── SVG ──────────────────────────────────────────────────────────

fn node_open(class: &str, rect: Rect, attrs: &str, label: &str, level: u8, interactive: bool) -> String {
    let focus = if interactive { r#" tabindex="0""# } else { "" };
    format!(
        r#"<g class="node {class}" transform="translate({:.1} {:.1})" role="treeitem" aria-level="{level}" aria-label="{}"{focus} {attrs}><rect class="node-box" width="{:.0}" height="{:.0}" rx="14"/>"#,
        rect.x,
        rect.y,
        esc(label),
        rect.w,
        rect.h
    )
}

fn text(class: &str, x: f32, y: f32, content: &str) -> String {
    format!(r#"<text class="{class}" x="{x:.0}" y="{y:.0}">{}</text>"#, esc(content))
}

fn selected_class(selected: bool) -> &'static str {
    if selected { " is-selected" } else { "" }
}

fn instance_status(instance: &Instance) -> String {
    if instance.completed {
        "Completed".to_string()
    } else {
        format!("Step {} of {}", instance.current_step + 1, instance.total_steps.max(1))
    }
}

fn instance_progress(instance: &Instance) -> f32 {
    if instance.completed || instance.total_steps == 0 {
        return if instance.completed { 1.0 } else { 0.0 };
    }
    (instance.current_step as f32 / instance.total_steps as f32).clamp(0.0, 1.0)
}

pub fn svg(tree: &Tree, opts: &TreeOptions) -> String {
    let inst_id = &tree.institution.id;
    let columns = columns(tree, opts.interactive);
    let counts: Vec<usize> = columns.iter().map(|(_, leaves)| leaves.len()).collect();
    let lay = layout(&counts);
    let scale = opts.zoom as f32 / 100.0;

    let mut out = format!(
        r#"<svg class="tree" viewBox="0 0 {w:.0} {h:.0}" width="{sw:.0}" height="{sh:.0}" role="tree" aria-label="{label}">"#,
        w = lay.width,
        h = lay.height,
        sw = lay.width * scale,
        sh = lay.height * scale,
        label = esc(&format!("Tree of {}", tree.institution.name)),
    );

    out.push_str(r#"<g class="edges">"#);
    for edge in &lay.edges {
        let (Some(from), Some(to)) = (lay.rect(edge.from), lay.rect(edge.to)) else { continue };
        let (x1, y1, x2, y2) = (from.center_x(), from.bottom(), to.center_x(), to.y);
        let mid = (y1 + y2) / 2.0;
        let ghost = match edge.to {
            Slot::Branch(c) => !matches!(columns[c].0, Column::Definition(_)),
            Slot::Leaf(c, l) => !matches!(columns[c].1[l], Leaf::Instance(_)),
            Slot::Root => false,
        };
        out.push_str(&format!(
            r#"<path class="edge{}" d="M{x1:.1} {y1:.1} C{x1:.1} {mid:.1} {x2:.1} {mid:.1} {x2:.1} {y2:.1}"/>"#,
            if ghost { " edge--ghost" } else { "" }
        ));
    }
    out.push_str("</g>");

    // Institution
    if let Some(rect) = lay.rect(Slot::Root) {
        let inst = &tree.institution;
        let is_selected = matches!(opts.selected, Some(NodeRef::Institution(id)) if id == inst_id);
        let class = format!(
            "node--root{}{}",
            if inst.pending { " is-pending" } else { "" },
            selected_class(is_selected)
        );
        let attrs = format!(r#"data-act="select" data-kind="institution" data-inst="{}""#, esc(inst_id));
        out.push_str(&node_open(&class, rect, &attrs, &format!("Institution {}", inst.name), 1, opts.interactive));
        out.push_str(&text("node-kicker", 20.0, 27.0, "INSTITUTION"));
        out.push_str(&text("node-title", 20.0, 54.0, &clip(&inst.name, 26)));
        let sub = if inst.pending {
            "Pending · waiting for the indexer".to_string()
        } else {
            match tree.definitions_stage {
                Stage::Loading => "Loading process definitions…".to_string(),
                Stage::Failed(_) => "Process definitions unavailable".to_string(),
                Stage::Ready => match tree.definitions.len() {
                    0 => "No process definitions yet".to_string(),
                    1 => "1 process definition".to_string(),
                    n => format!("{n} process definitions"),
                },
            }
        };
        out.push_str(&text("node-sub", 20.0, 76.0, &sub));
        out.push_str(&format!(r#"<text class="node-glyph" x="{:.0}" y="38">₿</text></g>"#, ROOT_W - 34.0));
    }

    for (c, (column, leaves)) in columns.iter().enumerate() {
        let Some(rect) = lay.rect(Slot::Branch(c)) else { continue };
        match column {
            Column::Definition(def) => {
                let is_selected = matches!(opts.selected, Some(NodeRef::Definition { id, .. }) if *id == def.id);
                let pending = def.provenance == Provenance::Pending;
                let declared = def.provenance == Provenance::Declared;
                let class = format!(
                    "node--def{}{}{}",
                    if pending { " is-pending" } else { "" },
                    if declared { " is-declared" } else { "" },
                    selected_class(is_selected)
                );
                let attrs = format!(
                    r#"data-act="select" data-kind="definition" data-inst="{}" data-id="{}""#,
                    esc(inst_id),
                    esc(&def.id)
                );
                out.push_str(&node_open(&class, rect, &attrs, &format!("Process definition {}", def.name), 2, opts.interactive));
                let steps = match def.steps.len() {
                    1 => "PROCESS · 1 STEP".to_string(),
                    n => format!("PROCESS · {n} STEPS"),
                };
                out.push_str(&text("node-kicker", 16.0, 24.0, &steps));
                out.push_str(&text("node-title", 16.0, 47.0, &clip(&def.name, 22)));
                let link = match def.provenance {
                    Provenance::Verified => "✓ Verified child",
                    Provenance::Declared => "⚠ Declared, unverified",
                    Provenance::Pending => "Pending · in the mempool",
                };
                let sub = if pending {
                    link.to_string()
                } else {
                    match (&def.instances_stage, def.instances.len()) {
                        (Stage::Loading, _) => format!("{link} · loading"),
                        (_, 0) => format!("{link} · no instances"),
                        (_, 1) => format!("{link} · 1 instance"),
                        (_, n) => format!("{link} · {n} instances"),
                    }
                };
                out.push_str(&text("node-sub", 16.0, 66.0, &sub));
                out.push_str("</g>");
            }
            Column::Skeleton => {
                out.push_str(&format!(
                    r#"<g class="node node--skeleton" transform="translate({:.1} {:.1})" aria-hidden="true"><rect class="node-box" width="{DEF_W:.0}" height="{:.0}" rx="14"/></g>"#,
                    rect.x, rect.y, rect.h
                ));
            }
            Column::Add => {
                let attrs = format!(r#"data-act="define-open" data-inst="{}""#, esc(inst_id));
                out.push_str(&node_open("node--add", rect, &attrs, "Define a new process", 2, opts.interactive));
                out.push_str(&format!(
                    r#"<text class="node-add" x="{:.0}" y="{:.0}">+ Define process</text></g>"#,
                    DEF_W / 2.0,
                    rect.h / 2.0 + 5.0
                ));
            }
        }

        let Column::Definition(def) = column else { continue };
        for (l, leaf) in leaves.iter().enumerate() {
            let Some(rect) = lay.rect(Slot::Leaf(c, l)) else { continue };
            match leaf {
                Leaf::Instance(instance) => {
                    let is_selected =
                        matches!(opts.selected, Some(NodeRef::Instance { address, .. }) if *address == instance.address);
                    let class = format!(
                        "node--leaf{}{}",
                        if instance.completed { " is-done" } else { "" },
                        selected_class(is_selected)
                    );
                    let attrs = format!(
                        r#"data-act="select" data-kind="instance" data-inst="{}" data-id="{}""#,
                        esc(inst_id),
                        esc(&instance.address)
                    );
                    let label = format!("Instance {} of {}: {}", short(&instance.address), def.name, instance_status(instance));
                    out.push_str(&node_open(&class, rect, &attrs, &label, 3, opts.interactive));
                    out.push_str(&text("node-mono", 12.0, 22.0, &short(&instance.address)));
                    let track = LEAF_W - 24.0;
                    out.push_str(&format!(
                        r#"<rect class="bar-track" x="12" y="31" width="{track:.0}" height="5" rx="2.5"/><rect class="bar-fill" x="12" y="31" width="{:.1}" height="5" rx="2.5"/>"#,
                        track * instance_progress(instance)
                    ));
                    out.push_str(&text("node-sub", 12.0, 51.0, &instance_status(instance)));
                    out.push_str("</g>");
                }
                Leaf::Loading => {
                    out.push_str(&format!(
                        r#"<g class="node node--skeleton" transform="translate({:.1} {:.1})" aria-hidden="true"><rect class="node-box" width="{LEAF_W:.0}" height="{:.0}" rx="12"/></g>"#,
                        rect.x, rect.y, rect.h
                    ));
                }
                Leaf::Failed => {
                    let attrs = format!(r#"data-act="reload-tree" data-id="{}""#, esc(inst_id));
                    out.push_str(&node_open("node--error", rect, &attrs, "Instances could not be loaded. Retry", 3, opts.interactive));
                    out.push_str(&text("node-sub", 12.0, 26.0, "Instances unavailable"));
                    out.push_str(&text("node-link", 12.0, 46.0, "Retry"));
                    out.push_str("</g>");
                }
                Leaf::Add => {
                    let attrs = format!(
                        r#"data-act="select" data-kind="definition" data-inst="{}" data-id="{}""#,
                        esc(inst_id),
                        esc(&def.id)
                    );
                    out.push_str(&node_open("node--add", rect, &attrs, &format!("Start an instance of {}", def.name), 3, opts.interactive));
                    out.push_str(&format!(
                        r#"<text class="node-add" x="{:.0}" y="{:.0}">+ Start instance</text></g>"#,
                        LEAF_W / 2.0,
                        rect.h / 2.0 + 5.0
                    ));
                }
            }
        }
    }
    out.push_str("</svg>");
    out
}

// ── Outline (narrow screens and "List" view) ─────────────────────

pub fn outline(tree: &Tree, opts: &TreeOptions) -> String {
    let inst = &tree.institution;
    let inst_id = esc(&inst.id);
    let mut out = String::from(r#"<ul class="outline" role="tree">"#);
    let root_selected = matches!(opts.selected, Some(NodeRef::Institution(id)) if *id == inst.id);
    out.push_str(&format!(
        r#"<li role="treeitem" aria-expanded="true"><button class="ol-node ol-root{}" data-act="select" data-kind="institution" data-inst="{inst_id}"><span class="ol-kicker">Institution</span><span class="ol-title">{}</span></button><ul role="group">"#,
        selected_class(root_selected),
        esc(&inst.name)
    ));
    match &tree.definitions_stage {
        Stage::Loading => out.push_str(r#"<li class="ol-note">Loading process definitions…</li>"#),
        Stage::Failed(_) => out.push_str(r#"<li class="ol-note">Process definitions unavailable.</li>"#),
        Stage::Ready if tree.definitions.is_empty() => out.push_str(r#"<li class="ol-note">No process definitions yet.</li>"#),
        Stage::Ready => {}
    }
    for def in &tree.definitions {
        let def_id = esc(&def.id);
        let selected = matches!(opts.selected, Some(NodeRef::Definition { id, .. }) if *id == def.id);
        let pending = def.provenance == Provenance::Pending;
        out.push_str(&format!(
            r#"<li role="treeitem" aria-expanded="true"><button class="ol-node ol-def{}{}" data-act="select" data-kind="definition" data-inst="{inst_id}" data-id="{def_id}"><span class="ol-kicker">Process · {} steps{}</span><span class="ol-title">{}</span></button><ul role="group">"#,
            selected_class(selected),
            if pending { " is-pending" } else { "" },
            def.steps.len(),
            match def.provenance {
                Provenance::Verified => " · verified",
                Provenance::Declared => " · declared, unverified",
                Provenance::Pending => " · pending",
            },
            esc(&def.name)
        ));
        for instance in &def.instances {
            let selected = matches!(opts.selected, Some(NodeRef::Instance { address, .. }) if *address == instance.address);
            out.push_str(&format!(
                r#"<li role="treeitem"><button class="ol-node ol-leaf{}{}" data-act="select" data-kind="instance" data-inst="{inst_id}" data-id="{}"><span class="mono">{}</span><span class="ol-status">{}</span></button></li>"#,
                selected_class(selected),
                if instance.completed { " is-done" } else { "" },
                esc(&instance.address),
                esc(&short(&instance.address)),
                esc(&instance_status(instance))
            ));
        }
        match &def.instances_stage {
            Stage::Loading => out.push_str(r#"<li class="ol-note">Loading instances…</li>"#),
            Stage::Failed(_) => out.push_str(&format!(
                r#"<li class="ol-note">Instances unavailable. <button class="link" data-act="reload-tree" data-id="{inst_id}">Retry</button></li>"#
            )),
            Stage::Ready => {}
        }
        if opts.interactive && !pending {
            out.push_str(&format!(
                r#"<li><button class="ol-add" data-act="select" data-kind="definition" data-inst="{inst_id}" data-id="{def_id}">+ Start instance</button></li>"#
            ));
        }
        out.push_str("</ul></li>");
    }
    if opts.interactive && tree.definitions_stage == Stage::Ready && !inst.pending {
        out.push_str(&format!(r#"<li><button class="ol-add" data-act="define-open" data-inst="{inst_id}">+ Define process</button></li>"#));
    }
    out.push_str("</ul></li></ul>");
    out
}

// ── Panel ────────────────────────────────────────────────────────

pub fn source_badge(source: &Source) -> String {
    match source {
        Source::Live => r#"<span class="badge badge-live"><i class="dot"></i>Live · testnet4 + Citrea</span>"#.to_string(),
        Source::Snapshot { date } => format!(
            r#"<span class="badge badge-warn">Snapshot from {} · indexer unavailable</span>"#,
            esc(date)
        ),
        Source::Local => r#"<span class="badge badge-pending">Pending · not indexed yet</span>"#.to_string(),
    }
}

/// One institution's tree with its header, notes and stage errors.
/// `title_link` makes the name a link to the institution's own page.
pub fn panel(state: &AppState, id: &str, title_link: bool) -> String {
    let esc_id = esc(id);
    match state.trees.get(id) {
        None | Some(Load::Idle) | Some(Load::Loading) => format!(
            r#"<article class="tree-panel is-loading" aria-busy="true"><header class="tree-head"><div><p class="kicker">Institution</p><h2 class="skeleton-text">Loading…</h2></div><div class="tree-meta">{}</div></header><div class="tree-skeleton"></div></article>"#,
            copyable(id, &short(id))
        ),
        Some(Load::Failed(error)) => format!(
            r#"<article class="tree-panel"><header class="tree-head"><div><p class="kicker">Institution</p><h2>{}</h2></div></header>{}</article>"#,
            esc(&short(id)),
            error_card(error, "reload-tree", &format!(r#"data-id="{esc_id}""#))
        ),
        Some(Load::Ready(tree)) => {
            let opts = TreeOptions { interactive: true, zoom: state.zoom, selected: state.selected.as_ref() };
            let name = esc(&tree.institution.name);
            let title = if title_link {
                format!(r##"<a href="#/i/{esc_id}">{name}</a>"##)
            } else {
                name
            };
            let body = if state.list_view {
                outline(tree, &opts)
            } else {
                format!(r#"<div class="tree-scroll">{}</div>"#, svg(tree, &opts))
            };
            let stage_error = match &tree.definitions_stage {
                Stage::Failed(why) => error_card(
                    &crate::model::UserError::new("Loading process definitions", why.clone()),
                    "reload-tree",
                    &format!(r#"data-id="{esc_id}""#),
                ),
                _ => String::new(),
            };
            let mut notes: Vec<String> = tree.notes.iter().map(|n| esc(n)).collect();
            if tree.definitions_stage == Stage::Ready && tree.source == Source::Live {
                notes.push("Process definitions held at other addresses are not listed: the indexer has no children endpoint.".to_string());
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!(r#"<ul class="tree-notes">{}</ul>"#, notes.iter().map(|n| format!("<li>{n}</li>")).collect::<String>())
            };
            format!(
                r#"<article class="tree-panel"><header class="tree-head"><div><p class="kicker">Institution</p><h2>{title}</h2>{desc}</div><div class="tree-meta">{badge}<span class="tree-id">{id_html}</span></div></header>{body}{stage_error}{notes}</article>"#,
                desc = if tree.institution.description.is_empty() {
                    String::new()
                } else {
                    format!(r#"<p class="tree-desc">{}</p>"#, esc(&clip(&tree.institution.description, 160)))
                },
                badge = source_badge(&tree.source),
                id_html = copyable(id, &short(id)),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Institution, Step};

    const ID: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
    const DEF: &str = "505eb39ed86053d5568a23bd128cd153142a21de02326e15354cb70059901e40i0";

    fn tree() -> Tree {
        Tree {
            institution: Institution {
                id: ID.into(), name: "Acme <script>".into(), description: String::new(), website: String::new(),
                admin: String::new(), holder: "tb1p".into(), pending: false, height: 0,
            },
            definitions: vec![Definition {
                id: DEF.into(), name: "KYC \"review\"".into(), description: String::new(),
                steps: vec![Step { name: "Review".into(), action: "approval".into() }],
                provenance: Provenance::Verified,
                instances: vec![
                    Instance { address: "0x00000000000000000000000000000000000000a1".into(), creator: String::new(), current_step: 1, total_steps: 2, completed: false },
                    Instance { address: "0x00000000000000000000000000000000000000a2".into(), creator: String::new(), current_step: 2, total_steps: 2, completed: true },
                ],
                instances_stage: Stage::Ready,
            }],
            definitions_stage: Stage::Ready,
            source: Source::Live,
            notes: vec![],
        }
    }

    fn opts(interactive: bool) -> TreeOptions<'static> {
        TreeOptions { interactive, zoom: 100, selected: None }
    }

    #[test]
    fn svg_has_three_levels_and_escapes_names() {
        let html = svg(&tree(), &opts(true));
        assert!(html.contains(r#"aria-level="1""#) && html.contains(r#"aria-level="2""#) && html.contains(r#"aria-level="3""#));
        assert!(!html.contains("<script>"));
        assert!(html.contains("Acme &lt;script&gt;"));
        assert!(html.contains("KYC &quot;review&quot;"));
        assert!(html.contains("Step 2 of 2"));
        assert!(html.contains("Completed"));
    }

    #[test]
    fn interactive_trees_offer_actions_and_previews_do_not() {
        let interactive = svg(&tree(), &opts(true));
        assert!(interactive.contains("+ Define process"));
        assert!(interactive.contains("+ Start instance"));
        assert!(interactive.contains(r#"tabindex="0""#));
        let preview = svg(&tree(), &opts(false));
        assert!(!preview.contains("+ Define process"));
        assert!(!preview.contains("tabindex"));
    }

    #[test]
    fn a_selected_node_is_marked() {
        let node = NodeRef::Definition { institution: ID.into(), id: DEF.into() };
        let html = svg(&tree(), &TreeOptions { interactive: true, zoom: 100, selected: Some(&node) });
        assert_eq!(html.matches("is-selected").count(), 1);
    }

    #[test]
    fn loading_and_failed_stages_are_visible() {
        let mut t = tree();
        t.definitions_stage = Stage::Loading;
        assert!(svg(&t, &opts(true)).contains("node--skeleton"));
        t.definitions_stage = Stage::Ready;
        t.definitions[0].instances_stage = Stage::Failed("rpc down".into());
        let html = svg(&t, &opts(true));
        assert!(html.contains("Instances unavailable"));
        assert!(html.contains(r#"data-act="reload-tree""#));
    }

    #[test]
    fn pending_nodes_are_marked_and_cannot_start_runs() {
        let mut t = tree();
        t.definitions[0].provenance = Provenance::Pending;
        t.definitions[0].instances.clear();
        let html = svg(&t, &opts(true));
        assert!(html.contains("is-pending"));
        assert!(!html.contains("+ Start instance"));
    }

    #[test]
    fn outline_lists_the_same_nodes() {
        let html = outline(&tree(), &opts(true));
        assert!(html.contains(r#"role="tree""#));
        assert_eq!(html.matches(r#"data-kind="instance""#).count(), 2);
        assert!(html.contains("+ Define process"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn zoom_scales_the_rendered_size_only() {
        let normal = svg(&tree(), &opts(true));
        let zoomed = svg(&tree(), &TreeOptions { interactive: true, zoom: 50, selected: None });
        let view_box = |s: &str| s.split("viewBox=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
        assert_eq!(view_box(&normal), view_box(&zoomed));
        assert_ne!(normal.split("width=\"").nth(1).unwrap().split('"').next(), zoomed.split("width=\"").nth(1).unwrap().split('"').next());
    }
}
