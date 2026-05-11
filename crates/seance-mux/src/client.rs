use std::collections::HashMap;

use bytes::Bytes;
use seance_protocol::frame::{CursorShape, GridPos, Resize, TerminalModes, ThemeColors};
use seance_protocol::identity::PaneRef;

use crate::{
    ClientRefresh, Domain, DomainEvent, PaneError, PaneFrame, PaneSpawnOptions, PaneView,
    SpawnError,
};

pub struct MuxClient<D> {
    domain: D,
    active: Option<PaneRef>,
    views: HashMap<PaneRef, PaneView>,
}

impl<D> MuxClient<D> {
    pub fn new(domain: D) -> Self {
        Self {
            domain,
            active: None,
            views: HashMap::new(),
        }
    }

    pub fn domain(&self) -> &D {
        &self.domain
    }

    pub fn domain_mut(&mut self) -> &mut D {
        &mut self.domain
    }

    pub fn active_pane_ref(&self) -> Option<PaneRef> {
        self.active
    }

    pub fn set_active_pane(&mut self, pane: PaneRef) -> Result<(), PaneError> {
        if self.views.contains_key(&pane) {
            self.active = Some(pane);
            Ok(())
        } else {
            Err(PaneError::new("unknown pane"))
        }
    }

    pub fn pane_view(&self, pane: PaneRef) -> Option<&PaneView> {
        self.views.get(&pane)
    }

    pub fn pane_view_mut(&mut self, pane: PaneRef) -> Option<&mut PaneView> {
        self.views.get_mut(&pane)
    }

    pub fn pane(&mut self, pane: PaneRef) -> PaneHandle<'_, D> {
        PaneHandle { client: self, pane }
    }

    pub fn active_pane(&mut self) -> Option<PaneHandle<'_, D>> {
        let pane = self.active?;
        Some(self.pane(pane))
    }
}

impl<D: Domain> MuxClient<D> {
    pub fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError> {
        let pane = self.domain.spawn_pane(options)?;
        self.views
            .entry(pane)
            .or_insert_with(|| PaneView::new(pane));
        if self.active.is_none() {
            self.active = Some(pane);
        }
        self.refresh_updates()?;
        Ok(pane)
    }

    pub fn refresh_updates(&mut self) -> Result<ClientRefresh, PaneError> {
        let mut events = Vec::new();
        self.domain.drain_events(&mut |event| events.push(event))?;

        let mut refresh = ClientRefresh::default();
        for event in events {
            match event {
                DomainEvent::PaneUpdate(update) => {
                    refresh
                        .image_events
                        .extend(update.image_events.iter().cloned());
                    if update.frame.is_some() {
                        refresh.frame_dirty = true;
                    }
                    self.views
                        .entry(update.pane)
                        .or_insert_with(|| PaneView::new(update.pane))
                        .apply_update(&update)?;
                }
                DomainEvent::PaneExited { pane } => {
                    refresh.exited.push(pane);
                }
                DomainEvent::Error { message, .. } => {
                    refresh.errors.push(message);
                }
            }
        }
        Ok(refresh)
    }
}

pub struct PaneHandle<'a, D> {
    client: &'a mut MuxClient<D>,
    pane: PaneRef,
}

impl<D> PaneHandle<'_, D> {
    pub fn pane_ref(&self) -> PaneRef {
        self.pane
    }

    pub fn frame_source(&self) -> Option<PaneFrame<'_>> {
        self.view().and_then(PaneView::frame_source)
    }

    pub fn generation(&self) -> Option<u64> {
        self.view().and_then(PaneView::generation)
    }

    pub fn cursor_shape(&self) -> Option<CursorShape> {
        self.view().and_then(PaneView::cursor_shape)
    }

    pub fn modes(&self) -> TerminalModes {
        self.view()
            .map_or(TerminalModes::default(), PaneView::modes)
    }

    pub fn has_selection(&self) -> bool {
        self.view().is_some_and(PaneView::has_selection)
    }

    pub fn clear_selection(&mut self) {
        if let Some(view) = self.view_mut() {
            view.clear_selection();
        }
    }

    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.view().and_then(PaneView::selection_range)
    }

    pub fn start_selection(&mut self, col: u16, row: u16) {
        if let Some(view) = self.view_mut() {
            view.start_selection(col, row);
        }
    }

    pub fn start_word_selection(&mut self, col: u16, row: u16) {
        if let Some(view) = self.view_mut() {
            view.start_word_selection(col, row);
        }
    }

    pub fn start_line_selection(&mut self, row: u16) {
        if let Some(view) = self.view_mut() {
            view.start_line_selection(row);
        }
    }

    pub fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(view) = self.view_mut() {
            view.update_selection(col, row);
        }
    }

    pub fn select_all(&mut self, cols: u16, rows: u16) {
        if let Some(view) = self.view_mut() {
            view.select_all(cols, rows);
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        self.view().and_then(PaneView::selection_text)
    }

    fn view(&self) -> Option<&PaneView> {
        self.client.views.get(&self.pane)
    }

    fn view_mut(&mut self) -> Option<&mut PaneView> {
        self.client.views.get_mut(&self.pane)
    }
}

impl<D: Domain> PaneHandle<'_, D> {
    pub fn write(&mut self, bytes: Bytes) -> Result<(), PaneError> {
        self.client.domain.write(self.pane, bytes)
    }

    pub fn resize(&mut self, resize: Resize) -> Result<(), PaneError> {
        self.client.domain.resize(self.pane, resize)
    }

    pub fn scroll_lines(&mut self, delta: i32) -> Result<(), PaneError> {
        self.client.domain.scroll_lines(self.pane, delta)
    }

    pub fn set_theme_colors(&mut self, colors: ThemeColors) -> Result<(), PaneError> {
        self.client.domain.set_theme_colors(self.pane, colors)
    }

    pub fn set_cursor_shape(&mut self, shape: CursorShape) -> Result<(), PaneError> {
        self.client.domain.set_cursor_shape(self.pane, shape)
    }

    pub fn ack_presented(&mut self, generation: u64) -> Result<(), PaneError> {
        self.client.domain.ack_presented(self.pane, generation)
    }
}
