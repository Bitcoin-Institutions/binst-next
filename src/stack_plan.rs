// reused-from: binst-pilot-webapp@dda2d1a:src/stack_plan.rs (2026-11-16) — adapted: none (verbatim)
//! Stack execution planner — institution-aware ordering of inscription intents.
//!
//! Pure Rust, no DOM, no async, no WASM imports — fully unit-testable on the
//! native target.
//!
//! Given the current [`crate::stack::Stack`] entries and the localStorage
//! snapshot (as a slice of JSON values), [`build_plan`] returns an
//! [`ExecutionPlan`]: a flat sequence of [`PlannedStep`]s in broadcast order,
//! each annotated with exactly how to resolve its parent UTXO at execution time.
//!
//! The three institution states covered:
//!
//! | Institution state | `ParentSource` used |
//! |---|---|
//! | In-stack (not yet broadcast) | `InBatch { batch_idx }` — live reveal UTXO from the same run |
//! | In-mempool (unconfirmed) | `FetchMempool { txid }` — fetch output 0 at sign time |
//! | Confirmed (6+ confs) | `FetchMempool { txid }` — same fetch, always works |
//!
//! ## Sibling deferred rule
//!
//! When two or more children share the same parent institution in the same
//! batch, each child's reveal tx must spend the parent sat (as input 0).  After
//! child A's reveal is broadcast the sat moves to output 0 of that reveal tx —
//! the UTXO child B needs cannot be known until child A is confirmed.  The
//! planner therefore marks every child beyond the first as `deferred = true`.
//! The UI renders deferred steps as locked and excludes them from the current
//! signing round; they become signable in the next session once child A's sat
//! location is known via `fetch_inscription_sat_utxo`.

use binst_inscription::BinstEntity;

use crate::stack::StackEntry;

// ── Parent source ─────────────────────────────────────────────────

/// How to obtain the parent UTXO at execution time.
///
/// Resolved by the execution driver in `stack_ui::on_sign_inscribe`.
#[derive(Debug, Clone, PartialEq)]
pub enum ParentSource {
    /// The parent institution is also in this batch at `batch_idx`
    /// (zero-based index into [`ExecutionPlan::steps`]).
    /// After that step succeeds its `reveal_utxo` is passed directly.
    InBatch { batch_idx: usize },

    /// The parent inscription was already broadcast (mempool or confirmed).
    /// Fetch output 0 of `txid` from Mempool.space at sign time.
    /// `inscription_id` is kept for logging / display.
    FetchMempool { txid: String, inscription_id: String },

    /// No parent — root entity (institution).
    None,
}

// ── Institution state ─────────────────────────────────────────────

/// State of the institution this step belongs to, at planning time.
#[derive(Debug, Clone, PartialEq)]
pub enum InstitutionState {
    /// Institution is also in this execution batch (not yet on-chain).
    InBatch,
    /// Broadcast but unconfirmed (`confirmed = false` in localStorage).
    InMempool,
    /// 6+ confirmations (`confirmed = true` in localStorage).
    Confirmed,
    /// Known inscription ID but not in localStorage (indexer-only).
    External,
    /// Root entity — this step *is* the institution.
    Root,
}

// ── Planned step ──────────────────────────────────────────────────

/// One resolved inscription step, ready for the execution driver.
#[derive(Debug, Clone)]
pub struct PlannedStep {
    /// Zero-based index into the original [`StackEntry`] slice.
    pub stack_idx: usize,
    /// Human-readable label (copied from `StackEntry::label`).
    pub label: String,
    /// The entity to inscribe.
    pub entity: BinstEntity,
    /// Ordinals inscription ID of the parent — embedded as tag 3 in
    /// the envelope script. `None` for root entities (institutions).
    pub parent_inscription_id: Option<String>,
    /// Human-readable parent label for display.
    /// `None` for root institutions.
    pub parent_display: Option<String>,
    /// How to resolve the parent UTXO at execution time.
    pub parent_source: ParentSource,
    /// Stable column grouping key.
    ///
    /// - In-batch institution: `"stk:{institution_stack_idx}"` (provisional, collision-free)
    /// - On-chain institution:  the institution's human-readable name from localStorage
    /// - External (no match):   the raw inscription ID string
    pub institution_key: String,
    /// Human-readable institution name for the column header.
    pub institution_label: String,
    /// State of the institution at planning time.
    pub institution_state: InstitutionState,
    /// `true` when this step is a sibling child that cannot be signed in the
    /// current round because the parent sat will still be held by an earlier
    /// sibling's unconfirmed reveal tx.
    pub deferred: bool,
}

// ── Execution plan ────────────────────────────────────────────────

/// Fully-resolved execution plan produced by [`build_plan`].
#[derive(Debug, Clone, Default)]
pub struct ExecutionPlan {
    /// Steps in broadcast order (parents always precede children).
    pub steps: Vec<PlannedStep>,
    /// Human-readable warnings to show the user before signing.
    pub warnings: Vec<String>,
}

// ── Planner ───────────────────────────────────────────────────────

/// Build a deterministic [`ExecutionPlan`] from the stack + localStorage state.
///
/// `local` is the raw slice returned by
/// `crate::storage::load_local_inscriptions()` — each element is a
/// `serde_json::Value` with at least `inscription_id`, `confirmed`, and
/// `reveal_txid` fields.
pub fn build_plan(
    entries: &[StackEntry],
    local: &[serde_json::Value],
) -> ExecutionPlan {
    let mut steps: Vec<PlannedStep> = Vec::with_capacity(entries.len());
    let mut warnings: Vec<String> = Vec::new();

    for (stack_idx, entry) in entries.iter().enumerate() {
        // ── 1. Derive parent_inscription_id from entity body ──────
        // Primary: read from the entity's institution_id / template_id field.
        // Fallback: if empty but entry.parent_ref points to an Institution
        //   in the batch, use that institution's label as a provisional key
        //   (pre-broadcast; the real inscription ID isn't known yet).
        let parent_inscription_id: Option<String> = {
            let from_entity = entity_parent_id(&entry.entity);
            if from_entity.is_some() {
                from_entity
            } else if let Some(pr) = entry.parent_ref {
                // parent_ref points to an Institution entry in the batch
                entries.get(pr).and_then(|parent_entry| {
                    if matches!(&parent_entry.entity, BinstEntity::Institution(_)) {
                        Some(parent_entry.label.clone())
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        };

        // ── 2. Resolve institution_key, institution_label & institution_state ──
        let (institution_key, institution_label, institution_state) =
            resolve_institution(stack_idx, entry, entries, local);

        // ── 3. Resolve ParentSource ───────────────────────────────
        let parent_source = match &parent_inscription_id {
            None => ParentSource::None,
            Some(pid) => {
                // (a) Is the parent inscription also in this batch?
                if let Some(batch_idx) = find_in_batch(pid, &steps) {
                    ParentSource::InBatch { batch_idx }
                } else {
                    // (b/c/d) Already broadcast or external — fetch at sign time.
                    let txid = pid
                        .split('i')
                        .next()
                        .unwrap_or(pid.as_str())
                        .to_string();

                    // Warn if we can't confirm the parent exists anywhere.
                    let in_local = local.iter().any(|v| {
                        v["inscription_id"].as_str() == Some(pid.as_str())
                    });
                    if !in_local {
                        warnings.push(format!(
                            "'{label}': parent inscription {pid} not found in local storage — \
                             will attempt to fetch from mempool (may fail if not yet broadcast)",
                            label = entry.label,
                        ));
                    }

                    ParentSource::FetchMempool {
                        txid,
                        inscription_id: pid.clone(),
                    }
                }
            }
        };

        // parent_display: human name for the parent (in-batch = "stack #N", on-chain = label/id)
        let parent_display: Option<String> = match &parent_inscription_id {
            None => None,
            Some(pid) => {
                // If parent_ref is set, use the referenced entry's label.
                if let Some(pr) = entry.parent_ref {
                    entries.get(pr).map(|p| format!("stack #{} — {}", pr, p.label))
                } else {
                    // Try localStorage name, fall back to pid itself.
                    let name = local.iter().find_map(|v| {
                        if v["inscription_id"].as_str() == Some(pid.as_str()) {
                            v["name"].as_str().map(|s| s.to_string())
                        } else { None }
                    });
                    Some(name.unwrap_or_else(|| pid.clone()))
                }
            }
        };

        steps.push(PlannedStep {
            stack_idx,
            label: entry.label.clone(),
            entity: entry.entity.clone(),
            parent_inscription_id,
            parent_display,
            parent_source,
            institution_key,
            institution_label,
            institution_state,
            deferred: false,
        });
    }

    // ── 4. Sibling-deferred pass ──────────────────────────────────
    // For each institution_key, find all child steps that are in-batch
    // (i.e. their parent is also being inscribed in this round, not yet on-chain).
    // Only the FIRST such child per institution is allowed to proceed;
    // all subsequent siblings are deferred because each one requires the
    // parent sat location that only becomes known after the prior sibling confirms.
    //
    // Children whose parent is already on-chain (FetchMempool) are NOT deferred
    // — the indexer's current output always reflects the live sat location regardless of
    // how many prior children exist.
    {
        use std::collections::HashMap;
        // institution_key → index of the first in-batch child step already seen
        let mut first_in_batch_child: HashMap<String, usize> = HashMap::new();

        for i in 0..steps.len() {
            let is_in_batch_child = matches!(steps[i].parent_source, ParentSource::InBatch { .. })
                && !matches!(steps[i].entity, BinstEntity::Institution(_));

            if is_in_batch_child {
                let key = steps[i].institution_key.clone();
                if first_in_batch_child.contains_key(&key) {
                    // A sibling was already planned for this institution in this
                    // round — defer this one.
                    steps[i].deferred = true;
                    warnings.push(format!(
                        "'{}': deferred — another child of the same institution is already \
                         in this signing round. Sign that child first, wait for confirmation, \
                         then sign this one.",
                        steps[i].label,
                    ));
                } else {
                    first_in_batch_child.insert(key, i);
                }
            }
        }
    }

    ExecutionPlan { steps, warnings }
}

// ── Helpers ───────────────────────────────────────────────────────

/// Extract the Ordinals parent inscription ID from an entity's body fields.
fn entity_parent_id(entity: &BinstEntity) -> Option<String> {
    match entity {
        BinstEntity::ProcessTemplate(pt) if !pt.institution_id.is_empty() => {
            Some(pt.institution_id.clone())
        }
        BinstEntity::ProcessInstance(pi) if !pi.template_id.is_empty() => {
            Some(pi.template_id.clone())
        }
        _ => None,
    }
}

/// Resolve the institution key and state for a stack entry.
///
/// Returns `(institution_key, institution_state)`.
/// Resolve the institution key, display label and state for a stack entry.
///
/// Returns `(institution_key, institution_label, institution_state)`.
///
/// `institution_key` is the stable column-grouping key:
/// - In-batch institution: `"stk:{institution_stack_idx}"` — collision-free even
///   when two institutions share the same name.
/// - On-chain institution: human-readable name from localStorage.
/// - External / unknown:   raw inscription ID string.
///
/// `institution_label` is the human-readable name shown in the column header.
fn resolve_institution(
    stack_idx: usize,
    entry: &StackEntry,
    all_entries: &[StackEntry],
    local: &[serde_json::Value],
) -> (String, String, InstitutionState) {
    // Root entities own themselves.
    if matches!(entry.entity, BinstEntity::Institution(_)) {
        let key = format!("stk:{stack_idx}");
        return (key, entry.label.clone(), InstitutionState::Root);
    }

    // ── Priority 1: parent_ref (set by design.rs at add time) ────────────
    // This is the most reliable link — a direct stack-index pointer to the
    // parent Institution entry, independent of names or inscription IDs.
    if let Some(pr) = entry.parent_ref {
        if let Some(parent) = all_entries.get(pr) {
            if matches!(parent.entity, BinstEntity::Institution(_)) {
                let key = format!("stk:{pr}");
                return (key, parent.label.clone(), InstitutionState::InBatch);
            }
        }
    }

    // ── Priority 2: institution_id from entity body ───────────────────────
    let inst_id = match &entry.entity {
        BinstEntity::ProcessTemplate(pt) => {
            if pt.institution_id.is_empty() { None } else { Some(pt.institution_id.as_str()) }
        }
        BinstEntity::ProcessInstance(pi) => {
            if pi.template_id.is_empty() { None } else { Some(pi.template_id.as_str()) }
        }
        _ => None,
    };

    let Some(iid) = inst_id else {
        // No institution_id and no parent_ref.
        let key = format!("stk:{stack_idx}");
        return (key, entry.label.clone(), InstitutionState::External);
    };

    // Is an institution with this label/id already in the batch (lower index)?
    let batch_inst = all_entries[..stack_idx].iter().enumerate().find(|(_, e)| {
        matches!(&e.entity, BinstEntity::Institution(_))
            && (e.label == iid || in_batch_inscription_matches(e, iid, local))
    });
    if let Some((bi, inst_entry)) = batch_inst {
        let key = format!("stk:{bi}");
        return (key, inst_entry.label.clone(), InstitutionState::InBatch);
    }

    // Is it in localStorage? Use institution NAME as column label, stable name as key.
    if let Some(local_entry) = local.iter().find(|v| {
        v["inscription_id"].as_str() == Some(iid)
    }) {
        let confirmed = local_entry["confirmed"].as_bool().unwrap_or(false);
        let state = if confirmed { InstitutionState::Confirmed } else { InstitutionState::InMempool };
        let name = local_entry["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(iid)
            .to_string();
        // Key = "onchain:{iid}" so two on-chain institutions with same name don't collide.
        let key = format!("onchain:{iid}");
        return (key, name, state);
    }

    // Not found anywhere — external unknown institution.
    (format!("ext:{iid}"), iid.to_string(), InstitutionState::External)
}

/// Check whether an in-batch Institution entry corresponds to a given
/// inscription ID by looking for it in localStorage (it may have been
/// inscribed in a prior session and re-added to the stack by label).
fn in_batch_inscription_matches(
    entry: &StackEntry,
    inscription_id: &str,
    local: &[serde_json::Value],
) -> bool {
    local.iter().any(|v| {
        v["inscription_id"].as_str() == Some(inscription_id)
            && v["name"].as_str() == Some(&entry.label)
    })
}

/// Find a step already in `steps` that *is* the parent institution for `pid`.
///
/// `pid` may be an institution name (provisional, pre-broadcast) OR an
/// on-chain inscription ID.  We match by:
/// - step label == pid  (name match, provisional)
/// - step institution_key == pid
/// - in_batch_inscription_matches via local (inscription ID → name)
fn find_in_batch(pid: &str, steps: &[PlannedStep]) -> Option<usize> {
    steps.iter().position(|s| {
        matches!(&s.entity, BinstEntity::Institution(_))
            && (s.label == pid || s.institution_key == pid)
    })
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use binst_inscription::{InstitutionBody, ProcessTemplateBody};

    fn inst_entry(label: &str) -> StackEntry {
        crate::stack::StackEntry {
            entity: BinstEntity::Institution(InstitutionBody {
                v: 0,
                name: label.into(),
                admin: "deadbeef".into(),
                citrea_contract: None,
                membership_rune: None,
                description: None,
                website: None,
                vault: None,
            }),
            label: label.into(),
            parent_ref: None,
        }
    }

    fn tmpl_entry(label: &str, institution_id: &str, parent_ref: Option<usize>) -> StackEntry {
        crate::stack::StackEntry {
            entity: BinstEntity::ProcessTemplate(ProcessTemplateBody {
                v: 0,
                name: label.into(),
                institution_id: institution_id.into(),
                description: None,
                citrea_contract: None,
                steps: vec![],
            }),
            label: label.into(),
            parent_ref,
        }
    }

    fn local_entry(inscription_id: &str, name: &str, confirmed: bool) -> serde_json::Value {
        serde_json::json!({
            "inscription_id": inscription_id,
            "reveal_txid": &inscription_id[..64],
            "name": name,
            "confirmed": confirmed,
        })
    }

    // ── root entity ───────────────────────────────────────────────

    #[test]
    fn plan_single_institution_has_no_parent() {
        let entries = vec![inst_entry("ACME")];
        let plan = build_plan(&entries, &[]);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].parent_source, ParentSource::None);
        assert_eq!(plan.steps[0].institution_state, InstitutionState::Root);
        assert!(plan.warnings.is_empty());
    }

    // ── in-batch parent ───────────────────────────────────────────

    #[test]
    fn plan_template_after_institution_uses_in_batch() {
        // The institution has label "ACME" and the template references it
        // by label as institution_id (provisional, pre-broadcast).
        let entries = vec![
            inst_entry("ACME"),
            tmpl_entry("Onboarding", "ACME", Some(0)),
        ];
        let plan = build_plan(&entries, &[]);
        assert_eq!(plan.steps.len(), 2);
        // Institution step
        assert_eq!(plan.steps[0].parent_source, ParentSource::None);
        // Template step: parent is in-batch at step 0
        assert_eq!(
            plan.steps[1].parent_source,
            ParentSource::InBatch { batch_idx: 0 }
        );
        assert_eq!(plan.steps[1].institution_state, InstitutionState::InBatch);
        assert!(plan.warnings.is_empty());
    }

    // ── mempool parent ────────────────────────────────────────────

    #[test]
    fn plan_template_with_mempool_parent_uses_fetch() {
        let iid = "abcdef0000000000000000000000000000000000000000000000000000000001i0";
        let txid = "abcdef0000000000000000000000000000000000000000000000000000000001";
        let entries = vec![tmpl_entry("Onboarding", iid, None)];
        let local = vec![local_entry(iid, "ACME", false)]; // confirmed=false
        let plan = build_plan(&entries, &local);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(
            plan.steps[0].parent_source,
            ParentSource::FetchMempool {
                txid: txid.to_string(),
                inscription_id: iid.to_string(),
            }
        );
        assert_eq!(plan.steps[0].institution_state, InstitutionState::InMempool);
        assert!(plan.warnings.is_empty());
    }

    // ── confirmed parent ──────────────────────────────────────────

    #[test]
    fn plan_template_with_confirmed_parent_uses_fetch() {
        let iid = "abcdef0000000000000000000000000000000000000000000000000000000001i0";
        let txid = "abcdef0000000000000000000000000000000000000000000000000000000001";
        let entries = vec![tmpl_entry("Onboarding", iid, None)];
        let local = vec![local_entry(iid, "ACME", true)]; // confirmed=true
        let plan = build_plan(&entries, &local);
        assert_eq!(
            plan.steps[0].parent_source,
            ParentSource::FetchMempool {
                txid: txid.to_string(),
                inscription_id: iid.to_string(),
            }
        );
        assert_eq!(plan.steps[0].institution_state, InstitutionState::Confirmed);
    }

    // ── external parent (warning emitted) ────────────────────────

    #[test]
    fn plan_template_with_external_parent_warns() {
        let iid = "ffff000000000000000000000000000000000000000000000000000000000001i0";
        let entries = vec![tmpl_entry("Onboarding", iid, None)];
        let plan = build_plan(&entries, &[]); // local is empty
        assert_eq!(plan.steps[0].institution_state, InstitutionState::External);
        assert_eq!(plan.warnings.len(), 1);
        assert!(plan.warnings[0].contains("not found in local storage"));
    }

    // ── empty stack ───────────────────────────────────────────────

    #[test]
    fn plan_empty_stack() {
        let plan = build_plan(&[], &[]);
        assert!(plan.steps.is_empty());
        assert!(plan.warnings.is_empty());
    }

    // ── multi-institution batch ───────────────────────────────────

    #[test]
    fn plan_two_institutions_independent() {
        let entries = vec![
            inst_entry("ACME"),
            inst_entry("GlobalBank"),
        ];
        let plan = build_plan(&entries, &[]);
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].parent_source, ParentSource::None);
        assert_eq!(plan.steps[1].parent_source, ParentSource::None);
        assert_eq!(plan.steps[0].institution_state, InstitutionState::Root);
        assert_eq!(plan.steps[1].institution_state, InstitutionState::Root);
        assert!(!plan.steps[0].deferred);
        assert!(!plan.steps[1].deferred);
    }

    // ── single child — not deferred ───────────────────────────────

    #[test]
    fn plan_single_child_not_deferred() {
        let entries = vec![
            inst_entry("ACME"),
            tmpl_entry("Onboarding", "ACME", Some(0)),
        ];
        let plan = build_plan(&entries, &[]);
        assert_eq!(plan.steps.len(), 2);
        assert!(!plan.steps[0].deferred, "institution must not be deferred");
        assert!(!plan.steps[1].deferred, "first child must not be deferred");
        assert!(plan.warnings.is_empty());
    }

    // ── two children same in-batch parent — second deferred ───────

    #[test]
    fn plan_two_children_same_in_batch_parent_second_deferred() {
        let entries = vec![
            inst_entry("ACME"),
            tmpl_entry("Onboarding", "ACME", Some(0)),
            tmpl_entry("Offboarding", "ACME", Some(0)),
        ];
        let plan = build_plan(&entries, &[]);
        assert_eq!(plan.steps.len(), 3);
        assert!(!plan.steps[0].deferred, "institution must not be deferred");
        assert!(!plan.steps[1].deferred, "first child must not be deferred");
        assert!(plan.steps[2].deferred,  "second child must be deferred");
        // Exactly one deferred warning
        assert_eq!(plan.warnings.iter().filter(|w| w.contains("deferred")).count(), 1);
    }

    // ── three children same in-batch parent — second and third deferred ──

    #[test]
    fn plan_three_children_same_in_batch_parent_second_third_deferred() {
        let entries = vec![
            inst_entry("ACME"),
            tmpl_entry("Alpha",   "ACME", Some(0)),
            tmpl_entry("Beta",    "ACME", Some(0)),
            tmpl_entry("Gamma",   "ACME", Some(0)),
        ];
        let plan = build_plan(&entries, &[]);
        assert_eq!(plan.steps.len(), 4);
        assert!(!plan.steps[0].deferred);
        assert!(!plan.steps[1].deferred);
        assert!(plan.steps[2].deferred);
        assert!(plan.steps[3].deferred);
        assert_eq!(plan.warnings.iter().filter(|w| w.contains("deferred")).count(), 2);
    }

    // ── children of *different* in-batch institutions — none deferred ──

    #[test]
    fn plan_children_of_different_institutions_not_deferred() {
        let entries = vec![
            inst_entry("ACME"),
            inst_entry("GlobalBank"),
            tmpl_entry("Onboarding", "ACME",       Some(0)),
            tmpl_entry("KYC",        "GlobalBank",  Some(1)),
        ];
        let plan = build_plan(&entries, &[]);
        assert_eq!(plan.steps.len(), 4);
        assert!(!plan.steps[2].deferred, "ACME child must not be deferred");
        assert!(!plan.steps[3].deferred, "GlobalBank child must not be deferred");
        assert!(plan.warnings.is_empty());
    }

    // ── children of on-chain parent — never deferred ──────────────

    #[test]
    fn plan_two_children_of_confirmed_parent_not_deferred() {
        // Both children reference an already-confirmed institution on-chain.
        // They get FetchMempool sources; the indexer's current output handles sat tracking.
        // Neither should be deferred.
        let iid = "abcdef0000000000000000000000000000000000000000000000000000000001i0";
        let entries = vec![
            tmpl_entry("Onboarding",  iid, None),
            tmpl_entry("Offboarding", iid, None),
        ];
        let local = vec![local_entry(iid, "ACME", true)];
        let plan = build_plan(&entries, &local);
        assert_eq!(plan.steps.len(), 2);
        assert!(!plan.steps[0].deferred, "first child of on-chain parent must not be deferred");
        assert!(!plan.steps[1].deferred, "second child of on-chain parent must not be deferred");
    }
}
