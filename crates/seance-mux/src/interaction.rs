use seance_protocol::frame::{GridPos, Selection};

use crate::links::LinkModifiers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoverInput {
    pub cell: GridPos,
    pub modifiers: LinkModifiers,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneInteractionState {
    selection: Option<Selection>,
    hover: Option<HoverInput>,
}

impl PaneInteractionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn hover_input(&self) -> Option<HoverInput> {
        self.hover
    }

    pub fn set_hover_input(&mut self, cell: GridPos, modifiers: LinkModifiers) -> bool {
        let next = Some(HoverInput { cell, modifiers });
        if self.hover == next {
            false
        } else {
            self.hover = next;
            true
        }
    }

    pub fn clear_hover_input(&mut self) -> bool {
        if self.hover.is_none() {
            false
        } else {
            self.hover = None;
            true
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.selection.as_ref().map(Selection::ordered_range)
    }

    pub fn start_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new(GridPos { col, row }));
    }

    pub fn start_word_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new(GridPos { col, row }));
    }

    pub fn start_line_selection(&mut self, row: u16) {
        self.selection = Some(Selection::new_line(GridPos { col: 0, row }));
    }

    /// Replace the active selection with `selection`. Used when callers
    /// (e.g. `PaneView`) have resolved a multi-cell range against the
    /// current snapshot and want to install it directly.
    pub fn set_selection(&mut self, selection: Selection) {
        self.selection = Some(selection);
    }

    pub fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(selection) = &mut self.selection {
            selection.update(GridPos { col, row });
        }
    }

    pub fn select_all(&mut self, cols: u16, rows: u16) {
        let mut selection = Selection::new_line(GridPos { col: 0, row: 0 });
        selection.update(GridPos {
            col: cols.saturating_sub(1),
            row: rows.saturating_sub(1),
        });
        self.selection = Some(selection);
    }
}
