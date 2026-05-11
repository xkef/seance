use std::sync::Arc;

use seance_frame::SnapshotFrameSource;
use seance_protocol::frame::{
    CursorShape, DirtySnapshot, FrameDelta, GridPos, HyperlinkRun, TerminalModes, VtSnapshot,
    apply_frame_delta,
};
use seance_protocol::identity::{PaneRef, ServerSeq};
use seance_protocol::mux::PaneUpdate;

use crate::PaneError;
use crate::interaction::{HoverInput, PaneInteractionState};
use crate::links::{DetectedLink, LinkDetector, LinkModifiers};

pub type PaneFrame<'a> = SnapshotFrameSource<'a>;

pub struct PaneView {
    pane: PaneRef,
    latest_snapshot: Option<Arc<VtSnapshot>>,
    interaction: PaneInteractionState,
    last_applied_seq: Option<ServerSeq>,
}

impl PaneView {
    pub fn new(pane: PaneRef) -> Self {
        Self {
            pane,
            latest_snapshot: None,
            interaction: PaneInteractionState::new(),
            last_applied_seq: None,
        }
    }

    pub fn pane_ref(&self) -> PaneRef {
        self.pane
    }

    pub fn last_applied_seq(&self) -> Option<ServerSeq> {
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

    pub fn pwd(&self) -> Option<&str> {
        self.latest_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pwd.as_deref())
    }

    pub fn hover_input(&self) -> Option<HoverInput> {
        self.interaction.hover_input()
    }

    pub fn set_hover_input(&mut self, pos: GridPos, modifiers: LinkModifiers) -> bool {
        self.interaction.set_hover_input(pos, modifiers)
    }

    pub fn clear_hover_input(&mut self) -> bool {
        self.interaction.clear_hover_input()
    }

    pub fn has_selection(&self) -> bool {
        self.interaction.has_selection()
    }

    pub fn clear_selection(&mut self) {
        self.interaction.clear_selection();
    }

    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.interaction.selection_range()
    }

    pub fn start_selection(&mut self, col: u16, row: u16) {
        self.interaction.start_selection(col, row);
    }

    pub fn start_word_selection(&mut self, col: u16, row: u16) {
        self.interaction.start_word_selection(col, row);
    }

    pub fn start_line_selection(&mut self, row: u16) {
        self.interaction.start_line_selection(row);
    }

    pub fn update_selection(&mut self, col: u16, row: u16) {
        self.interaction.update_selection(col, row);
    }

    pub fn select_all(&mut self, cols: u16, rows: u16) {
        self.interaction.select_all(cols, rows);
    }

    pub fn selection_text(&self) -> Option<String> {
        let selection = self.interaction.selection()?;
        let snapshot = self.latest_snapshot.as_ref()?;
        snapshot.selection_text(selection)
    }

    pub fn osc8_run_at(&self, col: u16, row: u16) -> Option<HyperlinkRun<'_>> {
        self.latest_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.osc8_run_at(col, row))
    }

    pub fn hovered_link(&self, detector: &LinkDetector) -> Option<DetectedLink> {
        let input = self.hover_input()?;
        self.link_at(input.cell, detector, input.modifiers)
    }

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
