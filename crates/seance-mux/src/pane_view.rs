use std::sync::Arc;

use seance_frame::SnapshotFrameSource;
use seance_protocol::frame::{
    CursorShape, DirtySnapshot, FrameDelta, GridPos, HyperlinkRun, Selection, TerminalModes,
    VtSnapshot, apply_frame_delta,
};
use seance_protocol::identity::{PaneRef, ServerSeq};
use seance_protocol::mux::PaneUpdate;

use crate::PaneError;
use crate::links::{DetectedLink, LinkDetector, LinkModifiers};

/// Borrowed frame source for one pane — alias of
/// [`SnapshotFrameSource`] so the renderer's `FrameSource` impl
/// applies.
pub type PaneFrame<'a> = SnapshotFrameSource<'a>;

/// Client-side per-pane state: the latest materialised snapshot, the
/// active selection (if any), and the last applied
/// [`seance_protocol::identity::ServerSeq`] used to gate further
/// updates.
pub struct PaneView {
    pane: PaneRef,
    latest_snapshot: Option<Arc<VtSnapshot>>,
    selection: Option<Selection>,
    last_applied_seq: Option<ServerSeq>,
}

impl PaneView {
    /// New view for `pane`, with no snapshot or selection.
    pub fn new(pane: PaneRef) -> Self {
        Self {
            pane,
            latest_snapshot: None,
            selection: None,
            last_applied_seq: None,
        }
    }

    /// Identity of the pane this view tracks.
    pub fn pane_ref(&self) -> PaneRef {
        self.pane
    }

    /// `seq` of the most recent [`PaneUpdate`] applied via
    /// [`Self::apply_update`].
    pub fn last_applied_seq(&self) -> Option<ServerSeq> {
        self.last_applied_seq
    }

    #[cfg(test)]
    pub(crate) fn latest_snapshot_for_tests(&self) -> Option<&VtSnapshot> {
        self.latest_snapshot.as_deref()
    }

    /// Apply a server-side [`PaneUpdate`]. Errors when the update is
    /// addressed to a different pane or its delta cannot be applied to
    /// the cached snapshot.
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

    /// Frame source over the cached snapshot, or `None` if no snapshot
    /// has been applied yet.
    pub fn frame_source(&self) -> Option<PaneFrame<'_>> {
        self.latest_snapshot
            .as_ref()
            .map(|snapshot| SnapshotFrameSource::for_pane(snapshot, self.pane))
    }

    /// Generation of the cached snapshot.
    pub fn generation(&self) -> Option<u64> {
        self.latest_snapshot
            .as_ref()
            .map(|snapshot| snapshot.generation)
    }

    /// Cursor shape override from the cached snapshot.
    pub fn cursor_shape(&self) -> Option<CursorShape> {
        self.latest_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.cursor.shape)
    }

    /// Terminal modes from the cached snapshot, or defaults if none
    /// has been applied yet.
    pub fn modes(&self) -> TerminalModes {
        self.latest_snapshot
            .as_ref()
            .map_or(TerminalModes::default(), |snapshot| snapshot.modes)
    }

    /// Working directory tracked from OSC 7 on the cached snapshot,
    /// when the shell emits one.
    pub fn pwd(&self) -> Option<&str> {
        self.latest_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pwd.as_deref())
    }

    /// Whether a selection is currently active.
    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// Drop the active selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Active selection ordered as `(start, end)`.
    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.selection.as_ref().map(Selection::ordered_range)
    }

    /// Begin a character-granularity selection at `(col, row)`.
    pub fn start_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new(GridPos { col, row }));
    }

    /// Begin a word-granularity selection at `(col, row)`.
    pub fn start_word_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new_word(GridPos { col, row }));
    }

    /// Begin a line-granularity selection at `row`.
    pub fn start_line_selection(&mut self, row: u16) {
        self.selection = Some(Selection::new_line(GridPos { col: 0, row }));
    }

    /// Move the active selection's head to `(col, row)`.
    pub fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(selection) = &mut self.selection {
            selection.update(GridPos { col, row });
        }
    }

    /// Select every cell of a `cols × rows` grid.
    pub fn select_all(&mut self, cols: u16, rows: u16) {
        let mut selection = Selection::new_line(GridPos { col: 0, row: 0 });
        selection.update(GridPos {
            col: cols.saturating_sub(1),
            row: rows.saturating_sub(1),
        });
        self.selection = Some(selection);
    }

    /// Concatenated text under the active selection, if any.
    pub fn selection_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let snapshot = self.latest_snapshot.as_ref()?;
        snapshot.selection_text(selection)
    }

    /// OSC 8 hyperlink run at `(col, row)`, if any.
    pub fn osc8_run_at(&self, col: u16, row: u16) -> Option<HyperlinkRun<'_>> {
        self.latest_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.osc8_run_at(col, row))
    }

    /// Activatable link at `pos` under `modifiers`, combining the OSC
    /// 8 run lookup and the regex-based detection in `detector`.
    pub fn link_at(
        &self,
        pos: GridPos,
        detector: &LinkDetector,
        modifiers: LinkModifiers,
    ) -> Option<DetectedLink> {
        self.latest_snapshot
            .as_ref()
            .and_then(|snapshot| detector.link_at(snapshot, pos, modifiers))
    }

    fn ensure_pane(&self, pane: PaneRef) -> Result<(), PaneError> {
        if pane == self.pane {
            Ok(())
        } else {
            Err(PaneError::new("message routed to a different pane"))
        }
    }
}
