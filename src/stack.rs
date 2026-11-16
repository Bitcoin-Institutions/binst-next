// reused-from: binst-pilot-webapp@dda2d1a:src/stack.rs (2026-11-16) — adapted: none (verbatim)
//! Changes Stack — collect multiple BINST inscription intents before
//! consolidating them into a single Bitcoin transaction.
//!
//! Think of it as a "shopping cart" for on-chain actions.  The user
//! creates institutions, process templates, step executions, etc.
//! during a session, and they accumulate here.  When ready, the stack
//! is handed to [`crate::txbuilder`] which produces a single commit +
//! reveal transaction (or a batch of them).
//!
//! This module is **pure Rust** — no DOM, no WASM imports — so every
//! function is unit-testable on the native target.

use binst_inscription::BinstEntity;

// ── Entry ────────────────────────────────────────────────────────

/// One pending inscription in the stack.
#[derive(Debug, Clone)]
pub struct StackEntry {
    /// The BINST entity to inscribe.
    pub entity: BinstEntity,

    /// Human-readable label for the UI (e.g. "ACME Corp", "Onboarding v2").
    pub label: String,

    /// Optional index into the same stack — the parent inscription that
    /// this entry depends on (e.g. a process template points at its
    /// institution).  Resolved at build time to the parent's inscription
    /// ID once the commit tx is known.
    pub parent_ref: Option<usize>,
}

// ── Stack ────────────────────────────────────────────────────────

/// Ordered collection of pending inscription intents.
#[derive(Debug, Clone, Default)]
pub struct Stack {
    entries: Vec<StackEntry>,
}

impl Stack {
    /// Create an empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of pending entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Immutable view of all entries.
    pub fn entries(&self) -> &[StackEntry] {
        &self.entries
    }

    /// Push a new inscription intent.  Returns the index of the new entry.
    pub fn push(&mut self, entity: BinstEntity, label: impl Into<String>, parent_ref: Option<usize>) -> usize {
        let idx = self.entries.len();
        self.entries.push(StackEntry {
            entity,
            label: label.into(),
            parent_ref,
        });
        idx
    }

    /// Remove an entry by index.  Returns `None` if out of bounds.
    ///
    /// **Note:** this invalidates `parent_ref` indices greater than `index`.
    /// Call [`Self::validate`] after bulk removals.
    pub fn remove(&mut self, index: usize) -> Option<StackEntry> {
        if index >= self.entries.len() {
            return None;
        }
        let removed = self.entries.remove(index);

        // Adjust parent_ref indices that pointed past the removed entry.
        for entry in &mut self.entries {
            if let Some(ref mut pr) = entry.parent_ref {
                if *pr == index {
                    // Parent was removed — orphan this entry.
                    entry.parent_ref = None;
                } else if *pr > index {
                    *pr -= 1;
                }
            }
        }

        Some(removed)
    }

    /// Move an entry from `from` to `to`.  Returns `false` if either
    /// index is out of bounds.
    #[allow(dead_code)]
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.entries.len() || to >= self.entries.len() || from == to {
            return false;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);

        // Rebuild parent_ref indices after the move.
        // A full re-index is safest for a small vec.
        let remap = |old: usize| -> usize {
            if old == from {
                to
            } else if from < to {
                // Shifted left: indices (from+1..=to) decrease by 1.
                if old > from && old <= to { old - 1 } else { old }
            } else {
                // Shifted right: indices (to..from) increase by 1.
                if old >= to && old < from { old + 1 } else { old }
            }
        };

        for entry in &mut self.entries {
            if let Some(ref mut pr) = entry.parent_ref {
                *pr = remap(*pr);
            }
        }

        true
    }

    /// Drop all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// After a parent Institution entry at `parent_stack_idx` is successfully
    /// broadcast with `inscription_id`, update every child entry that still
    /// holds a provisional parent ID (empty string or the institution label)
    /// so that subsequent planner runs can find the real on-chain ID.
    ///
    /// This prevents the "orphan on retry" bug: when a child's signing is
    /// rejected and re-run after the parent was already removed from the
    /// stack, the child's entity body now carries the real inscription ID
    /// instead of an empty string, so `build_plan` can resolve it via
    /// `entity_parent_id` → `FetchMempool`.
    pub fn patch_child_parent_ids(
        &mut self,
        parent_stack_idx: usize,
        parent_label: &str,
        inscription_id: &str,
    ) {
        for entry in &mut self.entries {
            // Only patch entries that explicitly reference this parent.
            let ref_matches = entry.parent_ref == Some(parent_stack_idx);
            match &mut entry.entity {
                BinstEntity::ProcessTemplate(pt)
                    if ref_matches || pt.institution_id.is_empty() || pt.institution_id == parent_label =>
                {
                    // Only patch if the current institution_id is blank or provisional.
                    if pt.institution_id.is_empty() || pt.institution_id == parent_label {
                        pt.institution_id = inscription_id.to_string();
                    }
                }
                BinstEntity::ProcessInstance(pi)
                    if ref_matches || pi.template_id.is_empty() || pi.template_id == parent_label =>
                {
                    if pi.template_id.is_empty() || pi.template_id == parent_label {
                        pi.template_id = inscription_id.to_string();
                    }
                }
                _ => {}
            }
        }
    }

    /// Validate the stack.  Returns a list of human-readable problems.
    /// An empty vec means the stack is ready to build.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();

        if self.entries.is_empty() {
            problems.push("Stack is empty — add at least one inscription.".into());
            return problems;
        }

        for (i, entry) in self.entries.iter().enumerate() {
            // Check parent_ref bounds.
            if let Some(pr) = entry.parent_ref {
                if pr >= self.entries.len() {
                    problems.push(format!(
                        "Entry {} ({:?}) references parent index {} which is out of bounds.",
                        i, entry.label, pr,
                    ));
                } else if pr >= i {
                    problems.push(format!(
                        "Entry {} ({:?}) references parent index {} which comes after it — parents must precede children.",
                        i, entry.label, pr,
                    ));
                }
            }

            // Entity-specific checks.
            match &entry.entity {
                BinstEntity::Institution(inst) => {
                    if inst.name.trim().is_empty() {
                        problems.push(format!("Entry {} institution has an empty name.", i));
                    }
                    if inst.admin.trim().is_empty() {
                        problems.push(format!("Entry {} institution has no admin pubkey.", i));
                    }
                }
                BinstEntity::ProcessTemplate(tmpl) => {
                    if tmpl.name.trim().is_empty() {
                        problems.push(format!("Entry {} process template has an empty name.", i));
                    }
                    if tmpl.steps.is_empty() {
                        problems.push(format!("Entry {} process template has no steps.", i));
                    }
                }
                BinstEntity::ProcessInstance(inst) => {
                    if inst.creator.trim().is_empty() {
                        problems.push(format!("Entry {} process instance has no creator.", i));
                    }
                }
                BinstEntity::StepExecution(step) => {
                    if step.actor.trim().is_empty() {
                        problems.push(format!("Entry {} step execution has no actor.", i));
                    }
                    if step.status.trim().is_empty() {
                        problems.push(format!("Entry {} step execution has no status.", i));
                    }
                }
                BinstEntity::StateDigest(_) => {
                    // State digests are structurally valid if they deserialize.
                }
            }
        }

        problems
    }

    /// Human-readable summary of the stack for the UI.
    pub fn summary(&self) -> String {
        if self.entries.is_empty() {
            return "Stack is empty.".into();
        }

        let mut institutions = 0u32;
        let mut templates = 0u32;
        let mut instances = 0u32;
        let mut steps = 0u32;
        let mut digests = 0u32;

        for entry in &self.entries {
            match &entry.entity {
                BinstEntity::Institution(_) => institutions += 1,
                BinstEntity::ProcessTemplate(_) => templates += 1,
                BinstEntity::ProcessInstance(_) => instances += 1,
                BinstEntity::StepExecution(_) => steps += 1,
                BinstEntity::StateDigest(_) => digests += 1,
            }
        }

        let mut parts = Vec::new();
        if institutions > 0 { parts.push(format!("{institutions} institution{}", if institutions > 1 { "s" } else { "" })); }
        if templates > 0 { parts.push(format!("{templates} template{}", if templates > 1 { "s" } else { "" })); }
        if instances > 0 { parts.push(format!("{instances} instance{}", if instances > 1 { "s" } else { "" })); }
        if steps > 0 { parts.push(format!("{steps} step{}", if steps > 1 { "s" } else { "" })); }
        if digests > 0 { parts.push(format!("{digests} digest{}", if digests > 1 { "s" } else { "" })); }

        format!("{} inscription{}: {}", self.entries.len(), if self.entries.len() > 1 { "s" } else { "" }, parts.join(", "))
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use binst_inscription::{InstitutionBody, ProcessTemplateBody, StepDef, StepExecutionBody};

    fn sample_institution() -> BinstEntity {
        BinstEntity::Institution(InstitutionBody {
            v: 0,
            name: "ACME".into(),
            admin: "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into(),
            citrea_contract: None,
            membership_rune: None,
            description: None,
            website: None,
            vault: None,
        })
    }

    fn sample_template(parent: Option<usize>) -> (BinstEntity, Option<usize>) {
        (
            BinstEntity::ProcessTemplate(ProcessTemplateBody {
                v: 0,
                name: "Onboarding".into(),
                institution_id: "test_institution_txid_placeholder_i0".into(),
                description: Some("Employee onboarding".into()),
                citrea_contract: None,
                steps: vec![
                    StepDef { name: "Submit".into(), action_type: Some("upload".into()) },
                    StepDef { name: "Review".into(), action_type: None },
                ],
            }),
            parent,
        )
    }

    fn sample_step() -> BinstEntity {
        BinstEntity::StepExecution(StepExecutionBody {
            v: 0,
            step_index: 0,
            status: "completed".into(),
            actor: "cafe1234".into(),
            data_hash: None,
        })
    }

    // ── push / len / is_empty ────────────────────────────

    #[test]
    fn new_stack_is_empty() {
        let s = Stack::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn push_increments_length() {
        let mut s = Stack::new();
        let idx = s.push(sample_institution(), "ACME", None);
        assert_eq!(idx, 0);
        assert_eq!(s.len(), 1);
        assert!(!s.is_empty());
    }

    #[test]
    fn push_returns_sequential_indices() {
        let mut s = Stack::new();
        assert_eq!(s.push(sample_institution(), "A", None), 0);
        let (tmpl, pr) = sample_template(Some(0));
        assert_eq!(s.push(tmpl, "B", pr), 1);
        assert_eq!(s.push(sample_step(), "C", None), 2);
    }

    // ── remove ───────────────────────────────────────────

    #[test]
    fn remove_returns_entry() {
        let mut s = Stack::new();
        s.push(sample_institution(), "ACME", None);
        let removed = s.remove(0);
        assert!(removed.is_some());
        assert!(s.is_empty());
    }

    #[test]
    fn remove_out_of_bounds_returns_none() {
        let mut s = Stack::new();
        assert!(s.remove(0).is_none());
        assert!(s.remove(99).is_none());
    }

    #[test]
    fn remove_adjusts_parent_refs() {
        let mut s = Stack::new();
        s.push(sample_institution(), "Inst0", None);          // 0
        s.push(sample_institution(), "Inst1", None);          // 1
        let (tmpl, _) = sample_template(Some(1));
        s.push(tmpl, "Tmpl→Inst1", Some(1));                  // 2, parent=1

        // Remove index 0 — Inst1 shifts from 1→0, Tmpl's parent should shift 1→0
        s.remove(0);
        assert_eq!(s.entries()[1].parent_ref, Some(0));
    }

    #[test]
    fn remove_orphans_children_of_removed_parent() {
        let mut s = Stack::new();
        s.push(sample_institution(), "Parent", None);         // 0
        let (tmpl, _) = sample_template(Some(0));
        s.push(tmpl, "Child", Some(0));                        // 1

        s.remove(0);
        // Child's parent was removed — should be orphaned (None)
        assert_eq!(s.entries()[0].parent_ref, None);
    }

    // ── reorder ──────────────────────────────────────────

    #[test]
    fn reorder_basic() {
        let mut s = Stack::new();
        s.push(sample_institution(), "A", None);
        s.push(sample_step(), "B", None);
        s.push(sample_step(), "C", None);

        assert!(s.reorder(0, 2));
        assert_eq!(s.entries()[0].label, "B");
        assert_eq!(s.entries()[1].label, "C");
        assert_eq!(s.entries()[2].label, "A");
    }

    #[test]
    fn reorder_out_of_bounds_returns_false() {
        let mut s = Stack::new();
        s.push(sample_institution(), "A", None);
        assert!(!s.reorder(0, 5));
        assert!(!s.reorder(5, 0));
    }

    #[test]
    fn reorder_same_index_returns_false() {
        let mut s = Stack::new();
        s.push(sample_institution(), "A", None);
        assert!(!s.reorder(0, 0));
    }

    // ── clear ────────────────────────────────────────────

    #[test]
    fn clear_empties_stack() {
        let mut s = Stack::new();
        s.push(sample_institution(), "A", None);
        s.push(sample_step(), "B", None);
        s.clear();
        assert!(s.is_empty());
    }

    // ── validate ─────────────────────────────────────────

    #[test]
    fn validate_empty_stack() {
        let s = Stack::new();
        let problems = s.validate();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("empty"));
    }

    #[test]
    fn validate_good_stack() {
        let mut s = Stack::new();
        s.push(sample_institution(), "ACME", None);
        let (tmpl, _) = sample_template(Some(0));
        s.push(tmpl, "Onboarding", Some(0));
        assert!(s.validate().is_empty(), "expected no problems: {:?}", s.validate());
    }

    #[test]
    fn validate_catches_empty_name() {
        let mut s = Stack::new();
        s.push(
            BinstEntity::Institution(InstitutionBody {
                v: 0, name: "".into(), admin: "aa".into(),
                citrea_contract: None, membership_rune: None,
                description: None, website: None, vault: None,
            }),
            "bad",
            None,
        );
        let problems = s.validate();
        assert!(problems.iter().any(|p| p.contains("empty name")));
    }

    #[test]
    fn validate_catches_parent_after_child() {
        let mut s = Stack::new();
        let (tmpl, _) = sample_template(None);
        s.push(tmpl, "Child", Some(1)); // parent_ref=1 but child is at 0
        s.push(sample_institution(), "Parent", None);
        let problems = s.validate();
        assert!(problems.iter().any(|p| p.contains("comes after")));
    }

    #[test]
    fn validate_catches_out_of_bounds_parent() {
        let mut s = Stack::new();
        let (tmpl, _) = sample_template(None);
        s.push(tmpl, "Orphan", Some(99));
        let problems = s.validate();
        assert!(problems.iter().any(|p| p.contains("out of bounds")));
    }

    // ── summary ──────────────────────────────────────────

    #[test]
    fn summary_empty() {
        let s = Stack::new();
        assert!(s.summary().contains("empty"));
    }

    #[test]
    fn summary_mixed_stack() {
        let mut s = Stack::new();
        s.push(sample_institution(), "I", None);
        let (tmpl, _) = sample_template(Some(0));
        s.push(tmpl, "T", Some(0));
        s.push(sample_step(), "S", None);

        let summary = s.summary();
        assert!(summary.contains("3 inscriptions"));
        assert!(summary.contains("1 institution"));
        assert!(summary.contains("1 template"));
        assert!(summary.contains("1 step"));
    }

    // ── patch_child_parent_ids ────────────────────────────

    /// Simulates the "reject on 2nd sig" scenario:
    /// - Institution (idx 0) and process template (idx 1, institution_id="") are in the stack.
    /// - Institution succeeds → patch_child_parent_ids is called.
    /// - Template's institution_id is now the real inscription ID.
    /// - Then remove institution from stack (idx 0) — template's parent_ref becomes None.
    /// - On retry, entity_parent_id finds the real ID → FetchMempool, not isolated.
    #[test]
    fn patch_child_parent_ids_fills_empty_institution_id() {
        let mut s = Stack::new();
        s.push(sample_institution(), "ACME", None);
        s.push(
            BinstEntity::ProcessTemplate(ProcessTemplateBody {
                v: 0,
                name: "Onboarding".into(),
                institution_id: "".into(), // empty — pre-broadcast provisional
                description: None,
                citrea_contract: None,
                steps: vec![StepDef { name: "s1".into(), action_type: None }],
            }),
            "Onboarding",
            Some(0), // parent_ref = institution at idx 0
        );

        let real_id = "d8233c386daa2ba99966a87a2736009081a17e5c3c91bfc6413bd9c468f040ci0";
        s.patch_child_parent_ids(0, "ACME", real_id);

        // Template's institution_id should now be the real inscription ID.
        match &s.entries()[1].entity {
            BinstEntity::ProcessTemplate(pt) => {
                assert_eq!(pt.institution_id, real_id, "institution_id must be patched");
            }
            _ => panic!("expected ProcessTemplate"),
        }

        // Now remove the institution (simulates successful removal).
        s.remove(0);

        // Template is now at idx 0, parent_ref = None (orphaned by remove).
        assert_eq!(s.entries()[0].parent_ref, None);

        // But institution_id is the real ID — retry will resolve via FetchMempool.
        match &s.entries()[0].entity {
            BinstEntity::ProcessTemplate(pt) => {
                assert_eq!(pt.institution_id, real_id, "institution_id must survive parent removal");
            }
            _ => panic!("expected ProcessTemplate"),
        }
    }

    #[test]
    fn patch_child_parent_ids_replaces_provisional_label() {
        // institution_id was set to the institution name (provisional key)
        let label = "ACME";
        let mut s = Stack::new();
        s.push(sample_institution(), label, None);
        s.push(
            BinstEntity::ProcessTemplate(ProcessTemplateBody {
                v: 0,
                name: "KYC".into(),
                institution_id: label.into(), // provisional = institution name
                description: None,
                citrea_contract: None,
                steps: vec![StepDef { name: "s1".into(), action_type: None }],
            }),
            "KYC",
            Some(0),
        );

        let real_id = "abcdef0000000000000000000000000000000000000000000000000000000001i0";
        s.patch_child_parent_ids(0, label, real_id);

        match &s.entries()[1].entity {
            BinstEntity::ProcessTemplate(pt) => {
                assert_eq!(pt.institution_id, real_id);
            }
            _ => panic!("expected ProcessTemplate"),
        }
    }

    #[test]
    fn patch_child_parent_ids_does_not_overwrite_real_id() {
        // If institution_id already has a real inscription ID, don't overwrite.
        let existing_id = "1111110000000000000000000000000000000000000000000000000000000001i0";
        let new_id     = "2222220000000000000000000000000000000000000000000000000000000002i0";
        let mut s = Stack::new();
        s.push(sample_institution(), "OtherInst", None);
        s.push(
            BinstEntity::ProcessTemplate(ProcessTemplateBody {
                v: 0,
                name: "KYC".into(),
                institution_id: existing_id.into(), // already a real on-chain ID
                description: None,
                citrea_contract: None,
                steps: vec![StepDef { name: "s1".into(), action_type: None }],
            }),
            "KYC",
            None, // no parent_ref to "OtherInst"
        );

        // Patch is called for idx=0 "OtherInst" — but the template's institution_id
        // is neither empty nor "OtherInst", so it must be left untouched.
        s.patch_child_parent_ids(0, "OtherInst", new_id);

        match &s.entries()[1].entity {
            BinstEntity::ProcessTemplate(pt) => {
                assert_eq!(pt.institution_id, existing_id, "must NOT overwrite real ID");
            }
            _ => panic!("expected ProcessTemplate"),
        }
    }
}
