//! Top-down layout of an institution tree: the institution on top, process
//! definitions on the second level, their instances on the lowest.
//!
//! Pure geometry, so it is tested natively. `render::tree` turns it into SVG.

pub const ROOT_W: f32 = 300.0;
pub const ROOT_H: f32 = 92.0;
pub const DEF_W: f32 = 232.0;
pub const DEF_H: f32 = 80.0;
pub const LEAF_W: f32 = 156.0;
pub const LEAF_H: f32 = 60.0;
const COLUMN_GAP: f32 = 36.0;
const LEAF_GAP: f32 = 14.0;
const LEVEL_GAP: f32 = 76.0;
const ROW_GAP: f32 = 22.0;
const MARGIN: f32 = 24.0;
/// Leaves per row under one definition; more wrap onto further rows.
pub const LEAVES_PER_ROW: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    Root,
    /// Column `usize` on the second level.
    Branch(usize),
    /// Leaf `.1` under column `.0`.
    Leaf(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    #[cfg(test)]
    fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.x + other.w && other.x < self.x + self.w && self.y < other.y + other.h && other.y < self.y + self.h
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: Slot,
    pub to: Slot,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub boxes: Vec<(Slot, Rect)>,
    pub edges: Vec<Edge>,
}

impl Layout {
    pub fn rect(&self, slot: Slot) -> Option<Rect> {
        self.boxes.iter().find(|(s, _)| *s == slot).map(|(_, r)| *r)
    }
}

/// Lay out a tree whose second level has `leaves.len()` columns, column `i`
/// carrying `leaves[i]` leaves.
pub fn layout(leaves: &[usize]) -> Layout {
    let column_width = |n: usize| {
        let per_row = n.min(LEAVES_PER_ROW).max(1) as f32;
        (per_row * LEAF_W + (per_row - 1.0) * LEAF_GAP).max(DEF_W)
    };
    let columns_width: f32 =
        leaves.iter().map(|&n| column_width(n)).sum::<f32>() + COLUMN_GAP * leaves.len().saturating_sub(1) as f32;
    let content_width = columns_width.max(ROOT_W);
    let width = content_width + 2.0 * MARGIN;

    let mut boxes = Vec::new();
    let mut edges = Vec::new();
    let root = Rect { x: MARGIN + (content_width - ROOT_W) / 2.0, y: MARGIN, w: ROOT_W, h: ROOT_H };
    boxes.push((Slot::Root, root));

    let branch_y = root.bottom() + LEVEL_GAP;
    let leaf_y = branch_y + DEF_H + LEVEL_GAP;
    let mut x = MARGIN + (content_width - columns_width) / 2.0;
    let mut height = if leaves.is_empty() { root.bottom() } else { branch_y + DEF_H };

    for (column, &count) in leaves.iter().enumerate() {
        let col_w = column_width(count);
        boxes.push((Slot::Branch(column), Rect { x: x + (col_w - DEF_W) / 2.0, y: branch_y, w: DEF_W, h: DEF_H }));
        edges.push(Edge { from: Slot::Root, to: Slot::Branch(column) });

        // Every row uses the first row's width, so a wrapped leaf sits
        // exactly under the leaf above it.
        let in_row = count.min(LEAVES_PER_ROW) as f32;
        let row_w = in_row * LEAF_W + (in_row - 1.0) * LEAF_GAP;
        for leaf in 0..count {
            let row = leaf / LEAVES_PER_ROW;
            let col = (leaf % LEAVES_PER_ROW) as f32;
            let rect = Rect {
                x: x + (col_w - row_w) / 2.0 + col * (LEAF_W + LEAF_GAP),
                y: leaf_y + row as f32 * (LEAF_H + ROW_GAP),
                w: LEAF_W,
                h: LEAF_H,
            };
            boxes.push((Slot::Leaf(column, leaf), rect));
            // The first row hangs from the definition; later rows hang from
            // the leaf above, so no edge crosses a box.
            let from = leaf.checked_sub(LEAVES_PER_ROW).map_or(Slot::Branch(column), |above| Slot::Leaf(column, above));
            edges.push(Edge { from, to: Slot::Leaf(column, leaf) });
            height = height.max(rect.bottom());
        }
        x += col_w + COLUMN_GAP;
    }

    Layout { width, height: height + MARGIN, boxes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_overlaps(l: &Layout) {
        for (i, (_, a)) in l.boxes.iter().enumerate() {
            for (_, b) in l.boxes.iter().skip(i + 1) {
                assert!(!a.overlaps(b), "{a:?} overlaps {b:?}");
            }
            assert!(a.x >= 0.0 && a.x + a.w <= l.width + 0.01, "{a:?} leaves the canvas");
            assert!(a.bottom() <= l.height + 0.01);
        }
    }

    #[test]
    fn a_lone_institution_is_centred() {
        let l = layout(&[]);
        assert_eq!(l.boxes.len(), 1);
        assert!(l.edges.is_empty());
        let root = l.rect(Slot::Root).unwrap();
        assert!((root.center_x() - l.width / 2.0).abs() < 0.01);
    }

    #[test]
    fn three_levels_sit_on_three_rows() {
        let l = layout(&[2, 0, 1]);
        let root = l.rect(Slot::Root).unwrap();
        let branch = l.rect(Slot::Branch(0)).unwrap();
        let leaf = l.rect(Slot::Leaf(0, 0)).unwrap();
        assert!(root.bottom() < branch.y);
        assert!(branch.bottom() < leaf.y);
        assert_eq!(l.rect(Slot::Branch(2)).unwrap().y, branch.y);
        assert_eq!(l.rect(Slot::Leaf(2, 0)).unwrap().y, leaf.y);
        no_overlaps(&l);
    }

    #[test]
    fn nothing_overlaps_for_many_shapes() {
        for shape in [vec![0], vec![1], vec![3], vec![4], vec![7, 2], vec![1, 1, 1, 1, 1, 1], vec![9, 0, 5, 3]] {
            no_overlaps(&layout(&shape));
        }
    }

    #[test]
    fn leaves_are_centred_under_their_definition() {
        let l = layout(&[3, 1]);
        let branch = l.rect(Slot::Branch(0)).unwrap();
        let middle = l.rect(Slot::Leaf(0, 1)).unwrap();
        assert!((branch.center_x() - middle.center_x()).abs() < 0.01);
        let single = l.rect(Slot::Leaf(1, 0)).unwrap();
        assert!((l.rect(Slot::Branch(1)).unwrap().center_x() - single.center_x()).abs() < 0.01);
    }

    #[test]
    fn extra_leaves_wrap_to_a_new_row_and_every_node_has_one_parent_edge() {
        let l = layout(&[5]);
        let first = l.rect(Slot::Leaf(0, 0)).unwrap();
        let wrapped = l.rect(Slot::Leaf(0, 3)).unwrap();
        assert!(wrapped.y > first.bottom());
        for (slot, _) in l.boxes.iter().filter(|(s, _)| *s != Slot::Root) {
            assert_eq!(l.edges.iter().filter(|e| e.to == *slot).count(), 1, "{slot:?}");
        }
    }

    #[test]
    fn the_root_is_centred_over_the_columns() {
        let l = layout(&[2, 2, 2]);
        let first = l.rect(Slot::Branch(0)).unwrap();
        let last = l.rect(Slot::Branch(2)).unwrap();
        let root = l.rect(Slot::Root).unwrap();
        assert!((root.center_x() - (first.x + last.x + last.w) / 2.0).abs() < 0.01);
    }
}
