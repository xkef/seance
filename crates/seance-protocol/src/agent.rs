//! Stable, versioned pane snapshot for external (agent-plane) consumers.
//!
//! [`VtSnapshot`] is seance's internal per-frame representation: its cell
//! encoding (a shared text buffer indexed by `text_start`/`text_len`) is an
//! implementation detail that may change as the renderer evolves. Agent-plane
//! tooling — the `wait_for` content locator (M10) and M11 trace assertions —
//! needs a snapshot it can serialize, store, and diff across seance versions
//! without tracking those internals.
//!
//! [`PaneSnapshot`] is that frozen projection. It is versioned by
//! [`SNAPSHOT_SCHEMA_VERSION`]: fields are only ever added, never removed or
//! repurposed, and every bump is a superset of the previous version. Consumers
//! check `schema_version` to decide how much of the payload they understand.

use serde::{Deserialize, Serialize};

use crate::frame::{CursorShape, TerminalModes, VtSnapshot};

/// Version of the [`PaneSnapshot`] wire schema. Bumped whenever a field is
/// added; the layout is append-only so a newer producer stays readable by an
/// older consumer up to the fields it knows.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Cursor state carried by a [`PaneSnapshot`], flattened out of the internal
/// [`CursorInfo`](crate::frame::CursorInfo) so external consumers depend on a
/// stable shape rather than the renderer's cursor struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
    /// `None` when the pane has not reported a shape (DECSCUSR default).
    pub shape: Option<CursorShape>,
}

/// A frozen, versioned view of a pane's visible screen. Richer than tmux
/// `capture-pane`: it carries the cursor, terminal modes, and working
/// directory alongside the text grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub schema_version: u32,
    pub cols: u16,
    pub rows: u16,
    /// Frame generation the snapshot was taken from — monotonic per pane.
    pub generation: u64,
    pub cursor: SnapshotCursor,
    pub modes: TerminalModes,
    pub pwd: Option<String>,
    /// One entry per grid row, top to bottom. Each string is the row's
    /// visible text with trailing blank cells trimmed; blank cells before the
    /// last non-blank cell render as a single space so column offsets line up.
    pub rows_text: Vec<String>,
}

impl PaneSnapshot {
    /// Project an internal [`VtSnapshot`] into the stable schema.
    pub fn from_vt(snapshot: &VtSnapshot) -> Self {
        let rows_text = (0..snapshot.rows)
            .map(|row| row_text(snapshot, row))
            .collect();

        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            cols: snapshot.cols,
            rows: snapshot.rows,
            generation: snapshot.generation,
            cursor: SnapshotCursor {
                row: snapshot.cursor.pos.row,
                col: snapshot.cursor.pos.col,
                visible: snapshot.cursor.visible,
                shape: snapshot.cursor.shape,
            },
            modes: snapshot.modes,
            pwd: snapshot.pwd.clone(),
            rows_text,
        }
    }
}

fn row_text(snapshot: &VtSnapshot, row: u16) -> String {
    let mut out = String::new();
    for col in 0..snapshot.cols {
        match snapshot.cell_at(row, col) {
            Some(cell) => {
                let text = snapshot.cell_text(cell);
                if text.is_empty() {
                    out.push(' ');
                } else {
                    out.push_str(text);
                }
            }
            None => out.push(' '),
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{CellAttrs, CellColor, CursorInfo, GridPos, VtSnapshot};

    fn snapshot_with(cols: u16, rows: u16, cells: &[&str]) -> VtSnapshot {
        let mut snap = VtSnapshot::empty(cols, rows);
        snap.generation = 7;
        for text in cells {
            snap.push_cell(
                text,
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            );
        }
        snap
    }

    #[test]
    fn from_vt_carries_dims_generation_and_schema_version() {
        let snap = snapshot_with(3, 1, &["a", "b", "c"]);
        let stable = PaneSnapshot::from_vt(&snap);
        assert_eq!(stable.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!((stable.cols, stable.rows), (3, 1));
        assert_eq!(stable.generation, 7);
    }

    #[test]
    fn rows_text_trims_trailing_blanks_but_keeps_interior_spaces() {
        // "hi" then a blank cell then "x", padded out to 6 columns.
        let snap = snapshot_with(6, 1, &["h", "i", "", "x", "", ""]);
        let stable = PaneSnapshot::from_vt(&snap);
        assert_eq!(stable.rows_text, vec!["hi x".to_string()]);
    }

    #[test]
    fn rows_text_has_one_entry_per_row_including_blank_rows() {
        let snap = snapshot_with(2, 3, &["a", "b"]);
        let stable = PaneSnapshot::from_vt(&snap);
        assert_eq!(
            stable.rows_text,
            vec!["ab".to_string(), String::new(), String::new()]
        );
    }

    #[test]
    fn cursor_is_flattened_from_cursor_info() {
        let mut snap = snapshot_with(4, 2, &["a"]);
        snap.cursor = CursorInfo {
            pos: GridPos { col: 2, row: 1 },
            visible: true,
            wide: false,
            shape: Some(CursorShape::Bar),
        };
        let stable = PaneSnapshot::from_vt(&snap);
        assert_eq!(
            stable.cursor,
            SnapshotCursor {
                row: 1,
                col: 2,
                visible: true,
                shape: Some(CursorShape::Bar),
            }
        );
    }

    #[test]
    fn postcard_round_trips() {
        let mut snap = snapshot_with(3, 2, &["o", "k"]);
        snap.pwd = Some("/home/user".to_string());
        let stable = PaneSnapshot::from_vt(&snap);

        let bytes = postcard::to_allocvec(&stable).unwrap();
        let decoded: PaneSnapshot = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, stable);
    }
}
