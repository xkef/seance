use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use bytes::Bytes;
use seance_protocol::{
    ClientMessage, CodecError, DirtySnapshot, DomainId as ProtocolDomainId, FrameDelta,
    ImageCacheEvent, PaneEpoch, PaneId, PaneUpdate, ProtocolErrorPayload, RequestId, ServerMessage,
    ServerSeq, Transport, TransportError, VtSnapshot, apply_frame_delta, decode_server_frame,
    encode_client_frame,
};
use seance_vt::{VtEvent, VtSessionHandle, spawn_vt_session};

pub use seance_frame::SnapshotFrameSource;
pub use seance_protocol::{
    CellAttrs, CellColor, CursorInfo, CursorShape, DomainId, GridPos, ImageId, ImageKey,
    InProcessTransport, LineContent, LineRange, PaneRef, PlacementSnapshot, Resize, Selection,
    SelectionGranularity, TerminalModes, ThemeColors, TransportFrame,
};

pub type PaneFrame<'a> = SnapshotFrameSource<'a>;

type EventSink = Arc<Mutex<Box<dyn Fn(MuxEvent) + Send>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    Pane { pane: PaneRef, event: PaneEvent },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneEvent {
    FrameDirty,
    ImageCache(ImageCacheEvent),
    Exited,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    PaneUpdate(PaneUpdate),
    PaneExited {
        pane: PaneRef,
    },
    Error {
        pane: Option<PaneRef>,
        message: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientRefresh {
    pub frame_dirty: bool,
    pub image_events: Vec<ImageCacheEvent>,
    pub exited: Vec<PaneRef>,
    pub errors: Vec<String>,
}

impl ClientRefresh {
    pub fn is_empty(&self) -> bool {
        !self.frame_dirty
            && self.image_events.is_empty()
            && self.exited.is_empty()
            && self.errors.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSpawnOptions {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub initial_cursor_shape: CursorShape,
}

impl Default for PaneSpawnOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
            initial_cursor_shape: CursorShape::Block,
        }
    }
}

impl From<PaneSpawnOptions> for seance_vt::VtSessionOptions {
    fn from(value: PaneSpawnOptions) -> Self {
        Self {
            cols: value.cols,
            rows: value.rows,
            pixel_width: value.pixel_width,
            pixel_height: value.pixel_height,
            initial_cursor_shape: value.initial_cursor_shape,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnError {
    message: String,
}

impl SpawnError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SpawnError {}

impl From<seance_vt::SpawnError> for SpawnError {
    fn from(value: seance_vt::SpawnError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<PaneError> for SpawnError {
    fn from(value: PaneError) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneError {
    message: String,
}

impl PaneError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PaneError {}

impl From<seance_vt::VtSessionError> for PaneError {
    fn from(value: seance_vt::VtSessionError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<TransportError> for PaneError {
    fn from(value: TransportError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<CodecError> for PaneError {
    fn from(value: CodecError) -> Self {
        Self::new(value.to_string())
    }
}

pub trait Domain {
    fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError>;

    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError>;

    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError>;

    fn resize(&mut self, pane: PaneRef, resize: Resize) -> Result<(), PaneError>;

    fn scroll_lines(&mut self, pane: PaneRef, delta: i32) -> Result<(), PaneError>;

    fn set_theme_colors(&mut self, pane: PaneRef, colors: ThemeColors) -> Result<(), PaneError>;

    fn set_cursor_shape(&mut self, pane: PaneRef, shape: CursorShape) -> Result<(), PaneError>;

    fn ack_presented(&mut self, pane: PaneRef, generation: u64) -> Result<(), PaneError>;
}

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

pub struct PaneView {
    pane: PaneRef,
    latest_snapshot: Option<Arc<VtSnapshot>>,
    selection: Option<Selection>,
    last_applied_seq: Option<ServerSeq>,
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

    pub fn last_applied_seq(&self) -> Option<ServerSeq> {
        self.last_applied_seq
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
            .map(|snapshot| SnapshotFrameSource::new(snapshot))
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

pub struct LocalDomain {
    domain: ProtocolDomainId,
    next_pane_id: u64,
    panes: HashMap<PaneRef, LocalPane>,
    pending_tx: mpsc::Sender<LocalDomainEvent>,
    pending_rx: mpsc::Receiver<LocalDomainEvent>,
    event_sink: EventSink,
}

impl LocalDomain {
    pub fn new<F>(event_sink: F) -> Self
    where
        F: Fn(MuxEvent) + Send + 'static,
    {
        let (pending_tx, pending_rx) = mpsc::channel();
        Self {
            domain: ProtocolDomainId(1),
            next_pane_id: 1,
            panes: HashMap::new(),
            pending_tx,
            pending_rx,
            event_sink: Arc::new(Mutex::new(Box::new(event_sink))),
        }
    }

    pub fn history(&self, pane: PaneRef) -> Option<&PaneFrameHistory> {
        self.panes.get(&pane).map(|pane| &pane.history)
    }

    fn pane_mut(&mut self, pane: PaneRef) -> Result<&mut LocalPane, PaneError> {
        self.panes
            .get_mut(&pane)
            .ok_or_else(|| PaneError::new("message routed to a different pane"))
    }
}

impl Domain for LocalDomain {
    fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError> {
        let _domain = self.domain;
        let pane_ref = PaneRef {
            pane_id: PaneId(self.next_pane_id),
            epoch: PaneEpoch(1),
        };
        self.next_pane_id += 1;

        let pending_tx = self.pending_tx.clone();
        let event_sink = Arc::clone(&self.event_sink);
        let vt = spawn_vt_session(options.into(), move |event| {
            let (local_event, pane_event) = match event {
                VtEvent::ContentDirty => (
                    LocalDomainEvent::ContentDirty { pane: pane_ref },
                    PaneEvent::FrameDirty,
                ),
                VtEvent::Exited => (
                    LocalDomainEvent::Exited { pane: pane_ref },
                    PaneEvent::Exited,
                ),
            };
            let _ = pending_tx.send(local_event);
            emit_mux_event(
                &event_sink,
                MuxEvent::Pane {
                    pane: pane_ref,
                    event: pane_event,
                },
            );
        })?;

        self.panes.insert(pane_ref, LocalPane::new(pane_ref, vt));
        Ok(pane_ref)
    }

    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError> {
        while let Ok(event) = self.pending_rx.try_recv() {
            match event {
                LocalDomainEvent::ContentDirty { .. } => {}
                LocalDomainEvent::Exited { pane } => {
                    if let Some(local) = self.panes.get_mut(&pane) {
                        local.exited = true;
                    }
                    sink(DomainEvent::PaneExited { pane });
                }
            }
        }

        let pane_refs = self.panes.keys().copied().collect::<Vec<_>>();
        for pane_ref in pane_refs {
            let Some(local) = self.panes.get_mut(&pane_ref) else {
                continue;
            };
            if local.exited {
                continue;
            }
            local.vt.clear_content_dirty_pending();
            let Some(snapshot) = local.vt.latest_snapshot() else {
                continue;
            };
            if local
                .server_snapshot
                .as_ref()
                .is_some_and(|latest| latest.generation == snapshot.generation)
            {
                continue;
            }

            let delta = FrameDelta::from_snapshot(local.server_snapshot.as_deref(), &snapshot);
            let update = PaneUpdate {
                pane: pane_ref,
                seq: local.alloc_seq(),
                image_events: Vec::new(),
                frame: Some(delta),
            };
            local.history.push(update.clone());
            local.server_snapshot = Some(snapshot);
            sink(DomainEvent::PaneUpdate(update));
        }
        Ok(())
    }

    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError> {
        self.pane_mut(pane)?.vt.write(bytes).map_err(Into::into)
    }

    fn resize(&mut self, pane: PaneRef, resize: Resize) -> Result<(), PaneError> {
        self.pane_mut(pane)?.vt.resize(resize).map_err(Into::into)
    }

    fn scroll_lines(&mut self, pane: PaneRef, delta: i32) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .scroll_lines(delta)
            .map_err(Into::into)
    }

    fn set_theme_colors(&mut self, pane: PaneRef, colors: ThemeColors) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .set_theme_colors(colors)
            .map_err(Into::into)
    }

    fn set_cursor_shape(&mut self, pane: PaneRef, shape: CursorShape) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .set_cursor_shape(shape)
            .map_err(Into::into)
    }

    fn ack_presented(&mut self, pane: PaneRef, generation: u64) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .ack_rendered(generation)
            .map_err(Into::into)
    }
}

fn emit_mux_event(sink: &EventSink, event: MuxEvent) {
    if let Ok(sink) = sink.lock() {
        sink(event);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalDomainEvent {
    ContentDirty { pane: PaneRef },
    Exited { pane: PaneRef },
}

struct LocalPane {
    vt: VtSessionHandle,
    server_snapshot: Option<Arc<VtSnapshot>>,
    history: PaneFrameHistory,
    next_seq: u64,
    exited: bool,
}

impl LocalPane {
    fn new(pane_ref: PaneRef, vt: VtSessionHandle) -> Self {
        Self {
            vt,
            server_snapshot: None,
            history: PaneFrameHistory::new(pane_ref, seance_protocol::MAX_RETAINED_PANE_UPDATES),
            next_seq: 1,
            exited: false,
        }
    }

    fn alloc_seq(&mut self) -> ServerSeq {
        let seq = ServerSeq(self.next_seq);
        self.next_seq += 1;
        seq
    }
}

pub struct ProtocolDomain<T> {
    transport: T,
    domain: ProtocolDomainId,
    next_request_id: AtomicU64,
}

impl<T> ProtocolDomain<T> {
    pub fn new(transport: T) -> Self {
        Self::with_domain(transport, DomainId(1))
    }

    pub fn with_domain(transport: T, domain: DomainId) -> Self {
        Self {
            transport,
            domain,
            next_request_id: AtomicU64::new(1),
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    fn next_request_id(&self) -> RequestId {
        RequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed))
    }
}

impl<T: Transport> ProtocolDomain<T> {
    fn send_client_message(&mut self, message: ClientMessage) -> Result<(), PaneError> {
        let frame = encode_client_frame(message, self.next_request_id())?;
        self.transport.send(frame).map_err(Into::into)
    }
}

impl<T: Transport> Domain for ProtocolDomain<T> {
    fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError> {
        self.send_client_message(ClientMessage::SpawnPane {
            domain: self.domain,
            cols: options.cols,
            rows: options.rows,
        })?;
        Err(SpawnError::new(
            "protocol pane spawn awaits server topology",
        ))
    }

    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError> {
        while let Some(frame) = self.transport.try_recv()? {
            match decode_server_frame(&frame)? {
                ServerMessage::PaneUpdate(update) => sink(DomainEvent::PaneUpdate(update)),
                ServerMessage::PaneExited { pane, .. } => sink(DomainEvent::PaneExited { pane }),
                ServerMessage::ResyncRequired { pane, reason } => sink(DomainEvent::Error {
                    pane: Some(pane),
                    message: reason,
                }),
                ServerMessage::Error(ProtocolErrorPayload { pane, message, .. }) => {
                    sink(DomainEvent::Error { pane, message });
                }
                ServerMessage::Hello(_)
                | ServerMessage::Topology(_)
                | ServerMessage::Pong { .. }
                | ServerMessage::Lines(_) => {}
            }
        }
        Ok(())
    }

    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::PaneInput {
            pane,
            bytes: bytes.to_vec(),
        })
    }

    fn resize(&mut self, pane: PaneRef, resize: Resize) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::ResizePane { pane, resize })
    }

    fn scroll_lines(&mut self, pane: PaneRef, delta: i32) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::ScrollPane { pane, delta })
    }

    fn set_theme_colors(&mut self, pane: PaneRef, colors: ThemeColors) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::SetPaneTheme { pane, colors })
    }

    fn set_cursor_shape(&mut self, pane: PaneRef, shape: CursorShape) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::SetPaneCursorShape { pane, shape })
    }

    fn ack_presented(&mut self, pane: PaneRef, generation: u64) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::AckPresented { pane, generation })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayBatch {
    Replay(Vec<PaneUpdate>),
    Resync { full: PaneUpdate },
}

#[derive(Debug, Clone)]
pub struct PaneFrameHistory {
    pane: PaneRef,
    max_updates: usize,
    updates: VecDeque<PaneUpdate>,
    latest_full: Option<PaneUpdate>,
}

impl PaneFrameHistory {
    pub fn new(pane: PaneRef, max_updates: usize) -> Self {
        Self {
            pane,
            max_updates: max_updates.max(1),
            updates: VecDeque::new(),
            latest_full: None,
        }
    }

    pub fn push(&mut self, update: PaneUpdate) {
        if update
            .frame
            .as_ref()
            .is_some_and(|frame| matches!(frame, FrameDelta::Full { .. }))
        {
            self.latest_full = Some(update.clone());
        }
        self.updates.push_back(update);
        while self.updates.len() > self.max_updates {
            self.updates.pop_front();
        }
    }

    pub fn first_seq(&self) -> Option<ServerSeq> {
        self.updates.front().map(|update| update.seq)
    }

    pub fn latest_seq(&self) -> Option<ServerSeq> {
        self.updates.back().map(|update| update.seq)
    }

    pub fn replay_since(&self, last_seen: Option<ServerSeq>) -> Option<ReplayBatch> {
        match last_seen {
            None => self
                .latest_full
                .clone()
                .map(|full| ReplayBatch::Resync { full }),
            Some(seq) => {
                if self.updates.is_empty() {
                    return self
                        .latest_full
                        .clone()
                        .map(|full| ReplayBatch::Resync { full });
                }
                let first = self.first_seq()?;
                let latest = self.latest_seq()?;
                if seq.0 >= latest.0 {
                    return Some(ReplayBatch::Replay(Vec::new()));
                }
                if seq.0 < first.0.saturating_sub(1) {
                    return self
                        .latest_full
                        .clone()
                        .map(|full| ReplayBatch::Resync { full });
                }
                Some(ReplayBatch::Replay(
                    self.updates
                        .iter()
                        .filter(|update| update.seq.0 > seq.0)
                        .cloned()
                        .collect(),
                ))
            }
        }
    }

    pub fn pane(&self) -> PaneRef {
        self.pane
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seance_protocol::{
        CellAttrs, CellColor, CursorInfo, MessageKind, ServerMessage, TerminalModes,
        decode_client_frame, encode_server_frame,
    };

    fn pane_ref() -> PaneRef {
        PaneRef {
            pane_id: PaneId(1),
            epoch: PaneEpoch(1),
        }
    }

    fn snapshot(generation: u64, text: &str) -> VtSnapshot {
        let mut snapshot = VtSnapshot::empty(1, 1);
        snapshot.generation = generation;
        snapshot.push_cell(
            text,
            CellColor::Default,
            CellColor::Default,
            CellAttrs::default(),
        );
        snapshot
    }

    #[test]
    fn pane_view_applies_full_and_partial_updates() {
        let pane = pane_ref();
        let mut view = PaneView::new(pane);
        let first = PaneUpdate {
            pane,
            seq: ServerSeq(1),
            image_events: Vec::new(),
            frame: Some(FrameDelta::Full {
                generation: 1,
                snapshot: snapshot(1, "a"),
            }),
        };
        view.apply_update(&first).unwrap();
        assert_eq!(view.generation(), Some(1));

        let mut next = snapshot(2, "b");
        next.dirty = DirtySnapshot::Partial(vec![0]);
        let partial = PaneUpdate {
            pane,
            seq: ServerSeq(2),
            image_events: Vec::new(),
            frame: Some(FrameDelta::from_snapshot(
                view.latest_snapshot.as_deref(),
                &next,
            )),
        };
        view.apply_update(&partial).unwrap();

        let snapshot = view.latest_snapshot.as_ref().unwrap();
        assert_eq!(snapshot.cell_text(&snapshot.cells[0]), "b");
        assert_eq!(view.last_applied_seq(), Some(ServerSeq(2)));
    }

    #[derive(Default)]
    struct ScriptedDomain {
        events: VecDeque<DomainEvent>,
        writes: Vec<(PaneRef, Bytes)>,
    }

    impl Domain for ScriptedDomain {
        fn spawn_pane(&mut self, _options: PaneSpawnOptions) -> Result<PaneRef, SpawnError> {
            Ok(pane_ref())
        }

        fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError> {
            while let Some(event) = self.events.pop_front() {
                sink(event);
            }
            Ok(())
        }

        fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError> {
            self.writes.push((pane, bytes));
            Ok(())
        }

        fn resize(&mut self, _pane: PaneRef, _resize: Resize) -> Result<(), PaneError> {
            Ok(())
        }

        fn scroll_lines(&mut self, _pane: PaneRef, _delta: i32) -> Result<(), PaneError> {
            Ok(())
        }

        fn set_theme_colors(
            &mut self,
            _pane: PaneRef,
            _colors: ThemeColors,
        ) -> Result<(), PaneError> {
            Ok(())
        }

        fn set_cursor_shape(
            &mut self,
            _pane: PaneRef,
            _shape: CursorShape,
        ) -> Result<(), PaneError> {
            Ok(())
        }

        fn ack_presented(&mut self, _pane: PaneRef, _generation: u64) -> Result<(), PaneError> {
            Ok(())
        }
    }

    #[test]
    fn mux_client_drains_domain_updates_into_pane_view() {
        let pane = pane_ref();
        let update = PaneUpdate {
            pane,
            seq: ServerSeq(1),
            image_events: Vec::new(),
            frame: Some(FrameDelta::Full {
                generation: 1,
                snapshot: snapshot(1, "x"),
            }),
        };
        let mut client = MuxClient::new(ScriptedDomain {
            events: VecDeque::from([DomainEvent::PaneUpdate(update)]),
            ..ScriptedDomain::default()
        });
        client.views.insert(pane, PaneView::new(pane));
        client.active = Some(pane);

        let refresh = client.refresh_updates().unwrap();

        assert!(refresh.frame_dirty);
        assert_eq!(client.pane_view(pane).unwrap().generation(), Some(1));
    }

    #[test]
    fn pane_handle_routes_commands_through_domain() {
        let pane = pane_ref();
        let mut client = MuxClient::new(ScriptedDomain::default());
        client.views.insert(pane, PaneView::new(pane));
        client.active = Some(pane);

        client.pane(pane).write(Bytes::from_static(b"abc")).unwrap();

        assert_eq!(
            client.domain.writes,
            vec![(pane, Bytes::from_static(b"abc"))]
        );
    }

    #[test]
    fn protocol_domain_encodes_commands_and_decodes_server_updates() {
        let (client_transport, server_transport) = InProcessTransport::pair();
        let pane = pane_ref();
        let mut domain = ProtocolDomain::new(client_transport);

        domain.write(pane, Bytes::from_static(b"abc")).unwrap();
        let frame = server_transport.try_recv().unwrap().unwrap();
        assert_eq!(frame.stream_id, seance_protocol::StreamId::INPUT);
        assert_eq!(
            decode_client_frame(&frame).unwrap(),
            ClientMessage::PaneInput {
                pane,
                bytes: b"abc".to_vec()
            }
        );

        server_transport
            .send(
                encode_server_frame(ServerMessage::PaneUpdate(PaneUpdate {
                    pane,
                    seq: ServerSeq(7),
                    image_events: Vec::new(),
                    frame: Some(FrameDelta::Full {
                        generation: 1,
                        snapshot: snapshot(1, "z"),
                    }),
                }))
                .unwrap(),
            )
            .unwrap();

        let mut events = Vec::new();
        domain
            .drain_events(&mut |event| events.push(event))
            .unwrap();
        assert!(
            matches!(events.as_slice(), [DomainEvent::PaneUpdate(update)] if update.seq == ServerSeq(7))
        );
    }

    #[test]
    fn frame_history_replays_retained_updates() {
        let pane = pane_ref();
        let mut history = PaneFrameHistory::new(pane, 4);
        history.push(PaneUpdate {
            pane,
            seq: ServerSeq(1),
            image_events: Vec::new(),
            frame: Some(FrameDelta::Full {
                generation: 1,
                snapshot: snapshot(1, "a"),
            }),
        });
        history.push(PaneUpdate {
            pane,
            seq: ServerSeq(2),
            image_events: Vec::new(),
            frame: Some(FrameDelta::Partial {
                base_generation: 1,
                generation: 2,
                cols: 1,
                rows: 1,
                cursor: CursorInfo::default(),
                modes: TerminalModes::default(),
                placements: Vec::new(),
                dirty_rows: vec![
                    seance_protocol::RowDelta::from_snapshot_row(&snapshot(2, "b"), 0).unwrap(),
                ],
            }),
        });

        let replay = history.replay_since(Some(ServerSeq(1))).unwrap();
        assert!(
            matches!(replay, ReplayBatch::Replay(updates) if updates.len() == 1 && updates[0].seq == ServerSeq(2))
        );
    }

    #[test]
    fn frame_history_resyncs_when_update_fell_out_of_ring() {
        let pane = pane_ref();
        let mut history = PaneFrameHistory::new(pane, 2);
        for seq in 1..=4 {
            history.push(PaneUpdate {
                pane,
                seq: ServerSeq(seq),
                image_events: Vec::new(),
                frame: Some(FrameDelta::Full {
                    generation: seq,
                    snapshot: snapshot(seq, "x"),
                }),
            });
        }

        let replay = history.replay_since(Some(ServerSeq(1))).unwrap();
        assert!(matches!(replay, ReplayBatch::Resync { full } if full.seq == ServerSeq(4)));
    }

    #[test]
    fn protocol_spawn_uses_spawn_message_kind() {
        let (client_transport, server_transport) = InProcessTransport::pair();
        let mut domain = ProtocolDomain::new(client_transport);

        let err = domain.spawn_pane(PaneSpawnOptions::default()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "protocol pane spawn awaits server topology"
        );

        let frame = server_transport.try_recv().unwrap().unwrap();
        let message = decode_client_frame(&frame).unwrap();
        assert!(matches!(message, ClientMessage::SpawnPane { .. }));
        assert_eq!(MessageKind::ClientSpawnPane, message.kind());
    }
}
