#[derive(Debug, Clone, Copy)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

pub type PaneId = u32;

enum Node {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        ratio: f32,
        left: Box<Node>,
        right: Box<Node>,
    },
}

pub struct PaneLayout {
    pub pane_id: PaneId,
    pub viewport: Viewport,
    pub grid_size: GridSize,
}

pub struct LayoutTree {
    root: Node,
    next_id: PaneId,
    cell_width: f32,
    cell_height: f32,
}

impl LayoutTree {
    pub fn new(first_pane: PaneId, cell_width: f32, cell_height: f32) -> Self {
        Self {
            root: Node::Leaf(first_pane),
            next_id: first_pane + 1,
            cell_width,
            cell_height,
        }
    }

    pub fn set_cell_size(&mut self, width: f32, height: f32) {
        self.cell_width = width;
        self.cell_height = height;
    }

    pub fn compute_layouts(&self, total_width: u32, total_height: u32) -> Vec<PaneLayout> {
        let mut layouts = Vec::new();
        let vp = Viewport {
            x: 0,
            y: 0,
            width: total_width,
            height: total_height,
        };
        self.layout_node(&self.root, vp, &mut layouts);
        layouts
    }

    fn layout_node(&self, node: &Node, viewport: Viewport, out: &mut Vec<PaneLayout>) {
        match node {
            Node::Leaf(pane_id) => {
                let cols = (viewport.width as f32 / self.cell_width).floor() as u16;
                let rows = (viewport.height as f32 / self.cell_height).floor() as u16;
                out.push(PaneLayout {
                    pane_id: *pane_id,
                    viewport,
                    grid_size: GridSize {
                        cols: cols.max(1),
                        rows: rows.max(1),
                    },
                });
            }
            Node::Split {
                direction,
                ratio,
                left,
                right,
            } => {
                let (l, r) = self.split_viewport(viewport, *direction, *ratio);
                self.layout_node(left, l, out);
                self.layout_node(right, r, out);
            }
        }
    }

    fn split_viewport(
        &self,
        vp: Viewport,
        direction: SplitDirection,
        ratio: f32,
    ) -> (Viewport, Viewport) {
        let divider = 1u32;
        match direction {
            SplitDirection::Horizontal => {
                let left_w = snap_to_grid(vp.width.saturating_sub(divider), ratio, self.cell_width);
                let right_x = vp.x + left_w + divider;
                let right_w = vp.width.saturating_sub(left_w + divider);
                (
                    Viewport { width: left_w, ..vp },
                    Viewport {
                        x: right_x,
                        width: right_w,
                        ..vp
                    },
                )
            }
            SplitDirection::Vertical => {
                let top_h = snap_to_grid(vp.height.saturating_sub(divider), ratio, self.cell_height);
                let bottom_y = vp.y + top_h + divider;
                let bottom_h = vp.height.saturating_sub(top_h + divider);
                (
                    Viewport {
                        height: top_h,
                        ..vp
                    },
                    Viewport {
                        y: bottom_y,
                        height: bottom_h,
                        ..vp
                    },
                )
            }
        }
    }

    pub fn split(&mut self, target: PaneId, direction: SplitDirection) -> Option<PaneId> {
        let new_id = self.next_id;
        if Self::split_node(&mut self.root, target, direction, new_id) {
            self.next_id += 1;
            Some(new_id)
        } else {
            None
        }
    }

    fn split_node(
        node: &mut Node,
        target: PaneId,
        direction: SplitDirection,
        new_id: PaneId,
    ) -> bool {
        match node {
            Node::Leaf(id) if *id == target => {
                let old = std::mem::replace(node, Node::Leaf(0));
                *node = Node::Split {
                    direction,
                    ratio: 0.5,
                    left: Box::new(old),
                    right: Box::new(Node::Leaf(new_id)),
                };
                true
            }
            Node::Split { left, right, .. } => {
                Self::split_node(left, target, direction, new_id)
                    || Self::split_node(right, target, direction, new_id)
            }
            _ => false,
        }
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        Self::collect_ids(&self.root, &mut ids);
        ids
    }

    fn collect_ids(node: &Node, out: &mut Vec<PaneId>) {
        match node {
            Node::Leaf(id) => out.push(*id),
            Node::Split { left, right, .. } => {
                Self::collect_ids(left, out);
                Self::collect_ids(right, out);
            }
        }
    }
}

fn snap_to_grid(total: u32, ratio: f32, cell_size: f32) -> u32 {
    let raw = total as f32 * ratio;
    let cells = (raw / cell_size).floor();
    (cells * cell_size) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pane_fills_viewport() {
        let tree = LayoutTree::new(0, 8.0, 16.0);
        let layouts = tree.compute_layouts(800, 600);
        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0].grid_size.cols, 100);
        assert_eq!(layouts[0].grid_size.rows, 37);
    }

    #[test]
    fn horizontal_split_produces_two_panes() {
        let mut tree = LayoutTree::new(0, 8.0, 16.0);
        tree.split(0, SplitDirection::Horizontal).unwrap();
        let layouts = tree.compute_layouts(800, 600);
        assert_eq!(layouts.len(), 2);
        let total = layouts[0].viewport.width + layouts[1].viewport.width + 1;
        assert_eq!(total, 800);
    }

    #[test]
    fn split_nonexistent_returns_none() {
        let mut tree = LayoutTree::new(0, 8.0, 16.0);
        assert!(tree.split(99, SplitDirection::Horizontal).is_none());
    }

    #[test]
    fn snap_to_grid_rounds_down() {
        assert_eq!(snap_to_grid(800, 0.5, 8.0), 400);
        assert_eq!(snap_to_grid(801, 0.5, 8.0), 400);
    }
}
