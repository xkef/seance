//! Window → Tab → SplitTree layout model.
//!
//! A [`Window`] holds an ordered list of [`Tab`]s; each tab owns one
//! [`SplitTree`] whose leaves are panes and whose internal nodes recursively
//! divide the available area. The tree is pure owned data (no VT / renderer
//! dependency) so the server can own the authoritative layout and ship it over
//! the wire for the client to position and draw.
//!
//! Geometry is resolved by [`SplitTree::panes_positioned`], which walks the
//! tree over a pixel [`Rect`] and yields one [`PositionedPane`] per leaf.
//! Divider chrome (borders, gaps) is intentionally out of scope here — panes
//! tile the area edge to edge; border quads are painted separately.

use serde::{Deserialize, Serialize};

use crate::identity::{PaneRef, TabId, WindowId};

/// How a [`SplitTree::Split`] divides its area between the two children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitAxis {
    /// Children sit side by side (`first` left, `second` right); the divider
    /// runs vertically and `ratio` is a fraction of the width.
    Columns,
    /// Children stack (`first` top, `second` bottom); the divider runs
    /// horizontally and `ratio` is a fraction of the height.
    Rows,
}

/// A binary tree of pane splits. Leaves reference a pane; internal nodes
/// divide their area along an axis by `ratio`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SplitTree {
    Leaf(PaneRef),
    Split {
        axis: SplitAxis,
        /// Fraction of the parent area (along `axis`) given to `first`;
        /// `second` receives the remainder. Clamped to `[0.0, 1.0]` when
        /// resolved so an out-of-range value never produces a negative rect.
        ratio: f32,
        first: Box<SplitTree>,
        second: Box<SplitTree>,
    },
}

/// Integer pixel rectangle within a window's content area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Pixel dimensions of a single terminal cell, used to derive a pane's
/// column/row count from its pixel rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellSize {
    pub width: u32,
    pub height: u32,
}

/// One resolved leaf: the pane, its pixel rect, and the cell grid that fits
/// inside that rect for the given [`CellSize`]. `cols` / `rows` are `0` when
/// the rect (or the cell size) is too small to hold a single cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionedPane {
    pub pane: PaneRef,
    pub rect: Rect,
    pub cols: u16,
    pub rows: u16,
}

impl SplitTree {
    pub fn leaf(pane: PaneRef) -> Self {
        SplitTree::Leaf(pane)
    }

    pub fn split(axis: SplitAxis, ratio: f32, first: SplitTree, second: SplitTree) -> Self {
        SplitTree::Split {
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// Number of leaves (panes) in the tree.
    pub fn pane_count(&self) -> usize {
        match self {
            SplitTree::Leaf(_) => 1,
            SplitTree::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    /// The first leaf in depth-first `first`-before-`second` order. Every
    /// tree has at least one leaf, so this never returns a placeholder.
    pub fn first_pane(&self) -> PaneRef {
        match self {
            SplitTree::Leaf(pane) => *pane,
            SplitTree::Split { first, .. } => first.first_pane(),
        }
    }

    /// Whether `pane` is a leaf somewhere in the tree.
    pub fn contains(&self, pane: PaneRef) -> bool {
        match self {
            SplitTree::Leaf(p) => *p == pane,
            SplitTree::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// Walk the tree over `area`, appending one [`PositionedPane`] per leaf.
    /// Leaves are emitted in depth-first `first`-before-`second` order, which
    /// is left-to-right / top-to-bottom on screen.
    pub fn panes_positioned(&self, area: Rect, cell: CellSize) -> Vec<PositionedPane> {
        let mut out = Vec::with_capacity(self.pane_count());
        self.collect(area, cell, &mut out);
        out
    }

    fn collect(&self, area: Rect, cell: CellSize, out: &mut Vec<PositionedPane>) {
        match self {
            SplitTree::Leaf(pane) => out.push(PositionedPane {
                pane: *pane,
                rect: area,
                cols: cells_along(area.width, cell.width),
                rows: cells_along(area.height, cell.height),
            }),
            SplitTree::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_area(area, *axis, *ratio);
                first.collect(a, cell, out);
                second.collect(b, cell, out);
            }
        }
    }
}

fn split_area(area: Rect, axis: SplitAxis, ratio: f32) -> (Rect, Rect) {
    let ratio = ratio.clamp(0.0, 1.0);
    match axis {
        SplitAxis::Columns => {
            let first_w = ((area.width as f32) * ratio).round() as u32;
            let first_w = first_w.min(area.width);
            let first = Rect {
                width: first_w,
                ..area
            };
            let second = Rect {
                x: area.x + first_w,
                width: area.width - first_w,
                ..area
            };
            (first, second)
        }
        SplitAxis::Rows => {
            let first_h = ((area.height as f32) * ratio).round() as u32;
            let first_h = first_h.min(area.height);
            let first = Rect {
                height: first_h,
                ..area
            };
            let second = Rect {
                y: area.y + first_h,
                height: area.height - first_h,
                ..area
            };
            (first, second)
        }
    }
}

fn cells_along(pixels: u32, cell: u32) -> u16 {
    if cell == 0 {
        return 0;
    }
    (pixels / cell).min(u16::MAX as u32) as u16
}

/// A tab within a window: a stable [`TabId`], a display title, the split
/// tree it owns, and which of that tree's panes currently has focus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub root: SplitTree,
    /// The focused pane. Always a leaf present in `root`; [`Tab::new`]
    /// seeds it with the tree's first pane.
    pub active_pane: PaneRef,
}

impl Tab {
    pub fn new(id: TabId, title: impl Into<String>, root: SplitTree) -> Self {
        let active_pane = root.first_pane();
        Self {
            id,
            title: title.into(),
            root,
            active_pane,
        }
    }

    /// Set the focused pane, ignoring the request if `pane` is not a leaf of
    /// this tab's tree. Returns whether the focus changed.
    pub fn focus(&mut self, pane: PaneRef) -> bool {
        if self.root.contains(pane) {
            self.active_pane = pane;
            true
        } else {
            false
        }
    }

    pub fn panes_positioned(&self, area: Rect, cell: CellSize) -> Vec<PositionedPane> {
        self.root.panes_positioned(area, cell)
    }
}

/// A window: an ordered list of tabs and the index of the active one. Only
/// the active tab is positioned for rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub id: WindowId,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl Window {
    pub fn new(id: WindowId, tabs: Vec<Tab>) -> Self {
        Self {
            id,
            tabs,
            active_tab: 0,
        }
    }

    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    /// Positions the panes of the active tab. Returns an empty vec when the
    /// window has no tabs or `active_tab` is out of range.
    pub fn panes_positioned(&self, area: Rect, cell: CellSize) -> Vec<PositionedPane> {
        self.active()
            .map(|tab| tab.panes_positioned(area, cell))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{DomainId, PaneEpoch, PaneId};

    fn pane(id: u64) -> PaneRef {
        PaneRef {
            domain: DomainId(1),
            pane_id: PaneId(id),
            epoch: PaneEpoch(0),
        }
    }

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 800,
        height: 600,
    };
    const CELL: CellSize = CellSize {
        width: 8,
        height: 16,
    };

    #[test]
    fn single_leaf_fills_area() {
        let tree = SplitTree::leaf(pane(1));
        let placed = tree.panes_positioned(AREA, CELL);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].pane, pane(1));
        assert_eq!(placed[0].rect, AREA);
        assert_eq!(placed[0].cols, 100);
        assert_eq!(placed[0].rows, 37);
    }

    #[test]
    fn even_columns_split_tiles_width() {
        let tree = SplitTree::split(
            SplitAxis::Columns,
            0.5,
            SplitTree::leaf(pane(1)),
            SplitTree::leaf(pane(2)),
        );
        let placed = tree.panes_positioned(AREA, CELL);
        assert_eq!(placed.len(), 2);
        assert_eq!(
            placed[0].rect,
            Rect {
                x: 0,
                y: 0,
                width: 400,
                height: 600
            }
        );
        assert_eq!(
            placed[1].rect,
            Rect {
                x: 400,
                y: 0,
                width: 400,
                height: 600
            }
        );
        // Children tile the parent exactly, no gap, no overlap.
        assert_eq!(placed[0].rect.width + placed[1].rect.width, AREA.width);
    }

    #[test]
    fn asymmetric_rows_split_gives_remainder_to_second() {
        let tree = SplitTree::split(
            SplitAxis::Rows,
            0.25,
            SplitTree::leaf(pane(1)),
            SplitTree::leaf(pane(2)),
        );
        let placed = tree.panes_positioned(AREA, CELL);
        assert_eq!(placed[0].rect.height, 150);
        assert_eq!(placed[1].rect.y, 150);
        assert_eq!(placed[1].rect.height, 450);
        assert_eq!(placed[0].rect.height + placed[1].rect.height, AREA.height);
    }

    #[test]
    fn nested_split_positions_every_leaf() {
        // Left column is one pane; right column splits into top/bottom.
        let tree = SplitTree::split(
            SplitAxis::Columns,
            0.5,
            SplitTree::leaf(pane(1)),
            SplitTree::split(
                SplitAxis::Rows,
                0.5,
                SplitTree::leaf(pane(2)),
                SplitTree::leaf(pane(3)),
            ),
        );
        let placed = tree.panes_positioned(AREA, CELL);
        assert_eq!(placed.len(), 3);
        assert_eq!(tree.pane_count(), 3);
        assert_eq!(
            placed[0].rect,
            Rect {
                x: 0,
                y: 0,
                width: 400,
                height: 600
            }
        );
        assert_eq!(
            placed[1].rect,
            Rect {
                x: 400,
                y: 0,
                width: 400,
                height: 300
            }
        );
        assert_eq!(
            placed[2].rect,
            Rect {
                x: 400,
                y: 300,
                width: 400,
                height: 300
            }
        );
    }

    #[test]
    fn out_of_range_ratio_is_clamped() {
        let tree = SplitTree::split(
            SplitAxis::Columns,
            2.0,
            SplitTree::leaf(pane(1)),
            SplitTree::leaf(pane(2)),
        );
        let placed = tree.panes_positioned(AREA, CELL);
        assert_eq!(placed[0].rect.width, AREA.width);
        assert_eq!(placed[1].rect.width, 0);
        assert_eq!(placed[1].cols, 0);
    }

    #[test]
    fn zero_cell_size_yields_zero_grid() {
        let tree = SplitTree::leaf(pane(1));
        let placed = tree.panes_positioned(
            AREA,
            CellSize {
                width: 0,
                height: 0,
            },
        );
        assert_eq!(placed[0].cols, 0);
        assert_eq!(placed[0].rows, 0);
    }

    #[test]
    fn window_positions_only_the_active_tab() {
        let tab0 = Tab::new(TabId(0), "shell", SplitTree::leaf(pane(1)));
        let tab1 = Tab::new(
            TabId(1),
            "editor",
            SplitTree::split(
                SplitAxis::Columns,
                0.5,
                SplitTree::leaf(pane(2)),
                SplitTree::leaf(pane(3)),
            ),
        );
        let mut window = Window::new(WindowId(1), vec![tab0, tab1]);
        assert_eq!(window.panes_positioned(AREA, CELL).len(), 1);
        window.active_tab = 1;
        assert_eq!(window.panes_positioned(AREA, CELL).len(), 2);
    }

    #[test]
    fn tab_seeds_active_pane_and_guards_focus() {
        let mut tab = Tab::new(
            TabId(0),
            "shell",
            SplitTree::split(
                SplitAxis::Columns,
                0.5,
                SplitTree::leaf(pane(1)),
                SplitTree::leaf(pane(2)),
            ),
        );
        // Seeded with the first (depth-first) leaf.
        assert_eq!(tab.active_pane, pane(1));
        assert!(tab.focus(pane(2)));
        assert_eq!(tab.active_pane, pane(2));
        // A pane not in the tree is rejected and leaves focus unchanged.
        assert!(!tab.focus(pane(9)));
        assert_eq!(tab.active_pane, pane(2));
    }

    #[test]
    fn tree_membership_queries() {
        let tree = SplitTree::split(
            SplitAxis::Rows,
            0.5,
            SplitTree::leaf(pane(1)),
            SplitTree::leaf(pane(2)),
        );
        assert_eq!(tree.first_pane(), pane(1));
        assert!(tree.contains(pane(2)));
        assert!(!tree.contains(pane(3)));
    }

    #[test]
    fn window_without_active_tab_positions_nothing() {
        let window = Window {
            id: WindowId(1),
            tabs: vec![],
            active_tab: 0,
        };
        assert!(window.panes_positioned(AREA, CELL).is_empty());
    }

    #[test]
    fn layout_round_trips_through_postcard() {
        let tree = SplitTree::split(
            SplitAxis::Rows,
            0.3,
            SplitTree::leaf(pane(1)),
            SplitTree::leaf(pane(2)),
        );
        let window = Window::new(WindowId(7), vec![Tab::new(TabId(3), "main", tree)]);
        let bytes = postcard::to_allocvec(&window).unwrap();
        let decoded: Window = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, window);
    }
}
