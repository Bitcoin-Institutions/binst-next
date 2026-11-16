//! The institution shown to visitors who have no wallet.

use crate::model::{Source, Tree};

/// A real institution on testnet4 (block 158993), inscribed with the fixed
/// reveal builder: its holder still has the sat and its child is a verified one.
pub const FEATURED_ID: &str = "8c511a203232ace2bf5c643a29b329720da1d840a01a94b0dc7cc3ae65c6145di0";

/// Date the bundled snapshot was taken.
const SNAPSHOT_DATE: &str = "2026-11-16";
const SNAPSHOT_JSON: &str = include_str!("../assets/featured.json");

/// The bundled copy of the featured tree, used when the indexer cannot be
/// reached. Always labelled as a snapshot.
pub fn snapshot(id: &str) -> Option<Tree> {
    if id != FEATURED_ID {
        return None;
    }
    let mut tree: Tree = serde_json::from_str(SNAPSHOT_JSON).ok()?;
    tree.source = Source::Snapshot { date: SNAPSHOT_DATE.to_string() };
    Some(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_only_for_the_featured_institution() {
        assert!(snapshot("00").is_none());
    }

    #[test]
    fn bundled_snapshot_is_either_absent_or_the_featured_tree() {
        if let Some(tree) = snapshot(FEATURED_ID) {
            assert_eq!(tree.institution.id, FEATURED_ID);
            assert!(matches!(tree.source, Source::Snapshot { .. }));
        }
    }
}
