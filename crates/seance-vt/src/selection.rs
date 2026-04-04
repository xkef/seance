//! Text selection over the terminal grid.
//!
//! Character, word, or line granularity. The text is extracted by
//! [`crate::terminal::Terminal::selection_text`] from the live VT grid;
//! this module only tracks the anchor/head pair.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GridPos {
    pub col: u16,
    pub row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionGranularity {
    Character,
    Word,
    Line,
}

#[derive(Debug, Clone)]
pub struct Selection {
    anchor: GridPos,
    head: GridPos,
    granularity: SelectionGranularity,
}

impl Selection {
    pub fn new(pos: GridPos) -> Self {
        Self::at(pos, SelectionGranularity::Character)
    }

    pub fn new_word(pos: GridPos) -> Self {
        Self::at(pos, SelectionGranularity::Word)
    }

    pub fn new_line(pos: GridPos) -> Self {
        Self::at(pos, SelectionGranularity::Line)
    }

    fn at(pos: GridPos, granularity: SelectionGranularity) -> Self {
        Self {
            anchor: pos,
            head: pos,
            granularity,
        }
    }

    pub fn update(&mut self, pos: GridPos) {
        self.head = pos;
    }

    pub fn granularity(&self) -> SelectionGranularity {
        self.granularity
    }

    /// Return the selection range as `(start, end)` in reading order.
    pub fn ordered_range(&self) -> (GridPos, GridPos) {
        let (a, b) = (self.anchor, self.head);
        if (a.row, a.col) <= (b.row, b.col) {
            (a, b)
        } else {
            (b, a)
        }
    }
}
