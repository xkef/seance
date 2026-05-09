use std::sync::Arc;

use seance_frame::SnapshotFrameSource;
use seance_protocol::{
    CursorShape, DirtySnapshot, FrameDelta, GridPos, PaneRef, PaneUpdate, Selection, TerminalModes,
    VtSnapshot, apply_frame_delta,
};

use crate::PaneError;

pub type PaneFrame<'a> = SnapshotFrameSource<'a>;

pub struct PaneView {
    pane: PaneRef,
    latest_snapshot: Option<Arc<VtSnapshot>>,
    selection: Option<Selection>,
    last_applied_seq: Option<seance_protocol::ServerSeq>,
}

impl PaneView {
    pub fn new(pane: PaneRef) -> Self {
        Self {
            pane,
            latest_snapshot: None,
            selection: None,
            last_applied_seq: None,
        }
    }

    pub fn pane_ref(&self) -> PaneRef {
        self.pane
    }

    pub fn last_applied_seq(&self) -> Option<seance_protocol::ServerSeq> {
        self.last_applied_seq
    }

    #[cfg(test)]
    pub(crate) fn latest_snapshot_for_tests(&self) -> Option<&VtSnapshot> {
        self.latest_snapshot.as_deref()
    }

    pub fn apply_update(&mut self, update: &PaneUpdate) -> Result<(), PaneError> {
        self.ensure_pane(update.pane)?;
        if let Some(frame) = &update.frame {
            let mut materialized = apply_frame_delta(self.latest_snapshot.as_deref(), frame)
                .map_err(|err| PaneError::new(err.to_string()))?;
            if matches!(frame, FrameDelta::Full { .. }) {
                materialized.dirty = DirtySnapshot::Full;
            }
            self.latest_snapshot = Some(Arc::new(materialized));
        }
        self.last_applied_seq = Some(update.seq);
        Ok(())
    }

    pub fn frame_source(&self) -> Option<PaneFrame<'_>> {
        self.latest_snapshot
            .as_ref()
            .map(|snapshot| SnapshotFrameSource::for_pane(snapshot, self.pane))
    }

    pub fn generation(&self) -> Option<u64> {
        self.latest_snapshot
            .as_ref()
            .map(|snapshot| snapshot.generation)
    }

    pub fn cursor_shape(&self) -> Option<CursorShape> {
        self.latest_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.cursor.shape)
    }

    pub fn modes(&self) -> TerminalModes {
        self.latest_snapshot
            .as_ref()
            .map_or(TerminalModes::default(), |snapshot| snapshot.modes)
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.selection.as_ref().map(Selection::ordered_range)
    }

    pub fn start_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new(GridPos { col, row }));
    }

    pub fn start_word_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new_word(GridPos { col, row }));
    }

    pub fn start_line_selection(&mut self, row: u16) {
        self.selection = Some(Selection::new_line(GridPos { col: 0, row }));
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

    pub fn selection_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let snapshot = self.latest_snapshot.as_ref()?;
        snapshot.selection_text(selection)
    }

    fn ensure_pane(&self, pane: PaneRef) -> Result<(), PaneError> {
        if pane == self.pane {
            Ok(())
        } else {
            Err(PaneError::new("message routed to a different pane"))
        }
    }
}
