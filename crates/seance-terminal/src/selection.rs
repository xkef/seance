//! Text selection state for a terminal pane.
//!
//! Tracks a rectangular or linear selection range in grid coordinates.
//! The terminal's screen content is queried to extract selected text.

/// A position in the terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPos {
    pub col: u16,
    pub row: u16,
}

/// Selection granularity (character, word, or line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionGranularity {
    Character,
    Word,
    Line,
}

/// Active text selection on a terminal grid.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Where the selection started (anchor point).
    anchor: GridPos,
    /// Current end of the selection (follows the cursor/mouse).
    head: GridPos,
    /// Selection granularity.
    granularity: SelectionGranularity,
}

impl Selection {
    /// Start a new character-level selection at the given grid position.
    pub fn new(pos: GridPos) -> Self {
        Self {
            anchor: pos,
            head: pos,
            granularity: SelectionGranularity::Character,
        }
    }

    /// Start a word-level selection (e.g. double-click).
    pub fn new_word(pos: GridPos) -> Self {
        Self {
            anchor: pos,
            head: pos,
            granularity: SelectionGranularity::Word,
        }
    }

    /// Start a line-level selection (e.g. triple-click).
    pub fn new_line(pos: GridPos) -> Self {
        Self {
            anchor: pos,
            head: pos,
            granularity: SelectionGranularity::Line,
        }
    }

    /// Update the head (moving end) of the selection.
    pub fn update(&mut self, pos: GridPos) {
        self.head = pos;
    }

    /// The anchor point (where the selection started).
    pub fn anchor(&self) -> GridPos {
        self.anchor
    }

    /// The head point (current end of the selection).
    pub fn head(&self) -> GridPos {
        self.head
    }

    pub fn granularity(&self) -> SelectionGranularity {
        self.granularity
    }

    /// Get the selection as a normalized (start, end) range where
    /// start is always before end in reading order.
    pub fn ordered_range(&self) -> (GridPos, GridPos) {
        let a = self.anchor;
        let b = self.head;
        if a.row < b.row || (a.row == b.row && a.col <= b.col) {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// Extract the selected text from screen lines.
    ///
    /// `lines` is the full screen content split by newline.
    /// `cols` is the grid width (used for line-mode selection).
    pub fn extract_text(&self, lines: &[&str], cols: u16) -> String {
        let (start, end) = self.ordered_range();
        let mut result = String::new();

        for row in start.row..=end.row {
            let Some(line) = lines.get(row as usize) else {
                continue;
            };

            let (col_start, col_end) = match self.granularity {
                SelectionGranularity::Line => (0u16, cols.saturating_sub(1)),
                _ => {
                    let cs = if row == start.row { start.col } else { 0 };
                    let ce = if row == end.row {
                        end.col
                    } else {
                        cols.saturating_sub(1)
                    };
                    (cs, ce)
                }
            };

            // Extract characters in the column range.
            let chars: Vec<char> = line.chars().collect();
            let from = (col_start as usize).min(chars.len());
            let to = ((col_end as usize) + 1).min(chars.len());
            let segment: String = chars[from..to].iter().collect();

            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(segment.trim_end());
        }

        result
    }
}

/// Expand a position to a word boundary given a line of text.
pub fn word_boundaries(line: &str, col: u16) -> (u16, u16) {
    let chars: Vec<char> = line.chars().collect();
    let pos = (col as usize).min(chars.len().saturating_sub(1));

    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';

    let mut start = pos;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = pos;
    while end + 1 < chars.len() && is_word_char(chars[end + 1]) {
        end += 1;
    }

    (start as u16, end as u16)
}
