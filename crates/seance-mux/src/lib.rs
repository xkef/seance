use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};

use bytes::Bytes;
use seance_protocol::{
    ClientMessage, CodecError, DirtySnapshot, DomainId, FrameDelta, MessageKind, PaneEpoch, PaneId,
    PaneRef, PaneUpdate, RequestId, ServerMessage, ServerSeq, StreamId, VtSnapshot,
    apply_frame_delta, decode_envelope, decode_typed_payload, encode_envelope,
};
use seance_vt::{VtEvent, VtSessionHandle, spawn_vt_session};

pub use seance_frame::SnapshotFrameSource;
pub use seance_protocol::{
    CellAttrs, CellColor, CursorInfo, CursorShape, GridPos, ImageCacheEvent, ImageId, ImageKey,
    PlacementSnapshot, Resize, Selection, SelectionGranularity, TerminalModes, ThemeColors,
};

pub type PaneFrame<'a> = SnapshotFrameSource<'a>;

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

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SpawnError {}

impl From<seance_vt::SpawnError> for SpawnError {
    fn from(value: seance_vt::SpawnError) -> Self {
        Self {
            message: value.to_string(),
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFrame {
    pub stream_id: StreamId,
    pub bytes: Bytes,
}

pub trait Transport {
    fn send(&self, frame: TransportFrame) -> Result<(), TransportError>;

    fn try_recv(&self) -> Result<Option<TransportFrame>, TransportError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Closed,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("transport is closed"),
        }
    }
}

impl std::error::Error for TransportError {}

pub struct InProcessTransport {
    tx: mpsc::Sender<TransportFrame>,
    rx: mpsc::Receiver<TransportFrame>,
}

impl InProcessTransport {
    pub fn pair() -> (Self, Self) {
        let (client_tx, server_rx) = mpsc::channel();
        let (server_tx, client_rx) = mpsc::channel();
        (
            Self {
                tx: client_tx,
                rx: client_rx,
            },
            Self {
                tx: server_tx,
                rx: server_rx,
            },
        )
    }
}

impl Transport for InProcessTransport {
    fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
        self.tx.send(frame).map_err(|_| TransportError::Closed)
    }

    fn try_recv(&self) -> Result<Option<TransportFrame>, TransportError> {
        match self.rx.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(TransportError::Closed),
        }
    }
}

#[derive(Debug)]
pub struct LocalMux {
    domain: DomainId,
    next_pane_id: u64,
}

impl Default for LocalMux {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalMux {
    pub fn new() -> Self {
        Self {
            domain: DomainId(1),
            next_pane_id: 1,
        }
    }

    pub fn spawn_pane<F>(
        &mut self,
        options: PaneSpawnOptions,
        event_sink: F,
    ) -> Result<Pane, SpawnError>
    where
        F: Fn(MuxEvent) + Send + 'static,
    {
        let _domain = self.domain;
        let pane_ref = PaneRef {
            pane_id: PaneId(self.next_pane_id),
            epoch: PaneEpoch(1),
        };
        self.next_pane_id += 1;

        let vt = spawn_vt_session(options.into(), move |event| {
            let pane_event = match event {
                VtEvent::ContentDirty => PaneEvent::FrameDirty,
                VtEvent::Exited => PaneEvent::Exited,
            };
            event_sink(MuxEvent::Pane {
                pane: pane_ref,
                event: pane_event,
            });
        })?;

        Ok(Pane::new(pane_ref, vt))
    }
}

pub struct Pane {
    pane_ref: PaneRef,
    vt: VtSessionHandle,
    client_transport: InProcessTransport,
    server_transport: InProcessTransport,
    latest_snapshot: Option<Arc<VtSnapshot>>,
    server_snapshot: Option<Arc<VtSnapshot>>,
    selection: Option<Selection>,
    history: PaneFrameHistory,
    next_seq: u64,
    next_request_id: AtomicU64,
}

impl Pane {
    fn new(pane_ref: PaneRef, vt: VtSessionHandle) -> Self {
        let (client_transport, server_transport) = InProcessTransport::pair();
        let mut pane = Self {
            pane_ref,
            vt,
            client_transport,
            server_transport,
            latest_snapshot: None,
            server_snapshot: None,
            selection: None,
            history: PaneFrameHistory::new(pane_ref, seance_protocol::MAX_RETAINED_PANE_UPDATES),
            next_seq: 1,
            next_request_id: AtomicU64::new(1),
        };
        pane.refresh_updates();
        pane
    }

    pub fn pane_ref(&self) -> PaneRef {
        self.pane_ref
    }

    pub fn refresh_updates(&mut self) {
        self.vt.clear_content_dirty_pending();
        let Some(snapshot) = self.vt.latest_snapshot() else {
            return;
        };
        if self
            .server_snapshot
            .as_ref()
            .is_some_and(|latest| latest.generation == snapshot.generation)
        {
            return;
        }

        let delta = FrameDelta::from_snapshot(self.server_snapshot.as_deref(), &snapshot);
        let update = PaneUpdate {
            pane: self.pane_ref,
            seq: self.alloc_seq(),
            image_events: Vec::new(),
            frame: Some(delta),
        };
        if let Err(err) = self.publish_pane_update(update, Arc::clone(&snapshot)) {
            log::warn!("failed to publish pane update through local transport: {err}");
        }
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

    pub fn write(&self, bytes: Bytes) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::PaneInput {
            pane: self.pane_ref,
            bytes: bytes.to_vec(),
        })
    }

    pub fn resize(&self, resize: Resize) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::ResizePane {
            pane: self.pane_ref,
            resize,
        })
    }

    pub fn scroll_lines(&self, delta: i32) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::ScrollPane {
            pane: self.pane_ref,
            delta,
        })
    }

    pub fn set_theme_colors(&self, colors: ThemeColors) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::SetPaneTheme {
            pane: self.pane_ref,
            colors,
        })
    }

    pub fn set_cursor_shape(&self, shape: CursorShape) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::SetPaneCursorShape {
            pane: self.pane_ref,
            shape,
        })
    }

    pub fn ack_presented(&self, generation: u64) -> Result<(), PaneError> {
        self.send_client_message(ClientMessage::AckPresented {
            pane: self.pane_ref,
            generation,
        })
    }

    pub fn history(&self) -> &PaneFrameHistory {
        &self.history
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

    fn send_client_message(&self, message: ClientMessage) -> Result<(), PaneError> {
        let frame = encode_client_frame(message, self.next_request_id())?;
        self.client_transport.send(frame)?;
        self.dispatch_client_requests()
    }

    fn dispatch_client_requests(&self) -> Result<(), PaneError> {
        while let Some(frame) = self.server_transport.try_recv()? {
            let message = decode_client_frame(&frame)?;
            self.apply_client_message(message)?;
        }
        Ok(())
    }

    fn apply_client_message(&self, message: ClientMessage) -> Result<(), PaneError> {
        match message {
            ClientMessage::ResizePane { pane, resize } => {
                self.ensure_pane(pane)?;
                self.vt.resize(resize)?;
            }
            ClientMessage::ScrollPane { pane, delta } => {
                self.ensure_pane(pane)?;
                self.vt.scroll_lines(delta)?;
            }
            ClientMessage::SetPaneTheme { pane, colors } => {
                self.ensure_pane(pane)?;
                self.vt.set_theme_colors(colors)?;
            }
            ClientMessage::SetPaneCursorShape { pane, shape } => {
                self.ensure_pane(pane)?;
                self.vt.set_cursor_shape(shape)?;
            }
            ClientMessage::PaneInput { pane, bytes } => {
                self.ensure_pane(pane)?;
                self.vt.write(Bytes::from(bytes))?;
            }
            ClientMessage::AckPresented { pane, generation } => {
                self.ensure_pane(pane)?;
                self.vt.ack_rendered(generation)?;
            }
            ClientMessage::AckApplied { pane, .. }
            | ClientMessage::ClosePane { pane }
            | ClientMessage::RequestSnapshot { pane }
            | ClientMessage::GetLines { pane, .. } => {
                self.ensure_pane(pane)?;
            }
            ClientMessage::Hello(_)
            | ClientMessage::Subscribe { .. }
            | ClientMessage::SpawnPane { .. }
            | ClientMessage::ImageCacheMiss { .. }
            | ClientMessage::Ping { .. } => {}
        }
        Ok(())
    }

    fn publish_pane_update(
        &mut self,
        update: PaneUpdate,
        source_snapshot: Arc<VtSnapshot>,
    ) -> Result<(), PaneError> {
        let frame = encode_server_frame(ServerMessage::PaneUpdate(update.clone()))?;
        self.history.push(update);
        self.server_snapshot = Some(source_snapshot);
        self.server_transport.send(frame)?;
        self.drain_server_messages()
    }

    fn drain_server_messages(&mut self) -> Result<(), PaneError> {
        while let Some(frame) = self.client_transport.try_recv()? {
            let message = decode_server_frame(&frame)?;
            self.apply_server_message(message)?;
        }
        Ok(())
    }

    fn apply_server_message(&mut self, message: ServerMessage) -> Result<(), PaneError> {
        match message {
            ServerMessage::PaneUpdate(update) => {
                self.ensure_pane(update.pane)?;
                for event in &update.image_events {
                    log::debug!("received image cache event through local transport: {event:?}");
                }
                if let Some(frame) = &update.frame {
                    let materialized = match apply_frame_delta(
                        self.latest_snapshot.as_deref(),
                        frame,
                    ) {
                        Ok(snapshot) => snapshot,
                        Err(err) => {
                            log::warn!(
                                "failed to materialize pane frame delta, falling back to full: {err}"
                            );
                            match frame {
                                FrameDelta::Full { snapshot, .. } => {
                                    let mut snapshot = snapshot.clone();
                                    snapshot.dirty = DirtySnapshot::Full;
                                    snapshot
                                }
                                FrameDelta::Partial { .. } => {
                                    return Err(PaneError::new(err.to_string()));
                                }
                            }
                        }
                    };
                    self.latest_snapshot = Some(Arc::new(materialized));
                }
            }
            ServerMessage::PaneExited { pane, .. } | ServerMessage::ResyncRequired { pane, .. } => {
                self.ensure_pane(pane)?;
            }
            ServerMessage::Hello(_)
            | ServerMessage::Error(_)
            | ServerMessage::Topology(_)
            | ServerMessage::Pong { .. }
            | ServerMessage::Lines(_) => {}
        }
        Ok(())
    }

    fn ensure_pane(&self, pane: PaneRef) -> Result<(), PaneError> {
        if pane == self.pane_ref {
            Ok(())
        } else {
            Err(PaneError::new("message routed to a different pane"))
        }
    }

    fn next_request_id(&self) -> RequestId {
        RequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed))
    }

    fn alloc_seq(&mut self) -> ServerSeq {
        let seq = ServerSeq(self.next_seq);
        self.next_seq += 1;
        seq
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

fn encode_client_frame(
    message: ClientMessage,
    request_id: RequestId,
) -> Result<TransportFrame, PaneError> {
    let kind = message.kind();
    let bytes = encode_envelope(kind, request_id, ServerSeq(0), &message)?;
    Ok(TransportFrame {
        stream_id: client_stream(&message),
        bytes: Bytes::from(bytes),
    })
}

fn encode_server_frame(message: ServerMessage) -> Result<TransportFrame, PaneError> {
    let kind = message.kind();
    let seq = server_seq(&message);
    let bytes = encode_envelope(kind, RequestId::PUSH, seq, &message)?;
    Ok(TransportFrame {
        stream_id: server_stream(&message),
        bytes: Bytes::from(bytes),
    })
}

fn decode_client_frame(frame: &TransportFrame) -> Result<ClientMessage, PaneError> {
    let (envelope, _consumed) =
        decode_envelope(&frame.bytes, seance_protocol::MAX_DECODED_MESSAGE_BYTES)?;
    let kind = envelope.known_kind()?;
    ensure_client_kind(kind)?;
    let message: ClientMessage = decode_typed_payload(&envelope, kind)?;
    if message.kind() != kind {
        return Err(PaneError::new(
            "client envelope kind does not match payload",
        ));
    }
    Ok(message)
}

fn decode_server_frame(frame: &TransportFrame) -> Result<ServerMessage, PaneError> {
    let (envelope, _consumed) =
        decode_envelope(&frame.bytes, seance_protocol::MAX_DECODED_MESSAGE_BYTES)?;
    let kind = envelope.known_kind()?;
    ensure_server_kind(kind)?;
    let message: ServerMessage = decode_typed_payload(&envelope, kind)?;
    if message.kind() != kind {
        return Err(PaneError::new(
            "server envelope kind does not match payload",
        ));
    }
    Ok(message)
}

fn ensure_client_kind(kind: MessageKind) -> Result<(), PaneError> {
    match kind {
        MessageKind::ClientHello
        | MessageKind::ClientSubscribe
        | MessageKind::ClientSpawnPane
        | MessageKind::ClientClosePane
        | MessageKind::ClientResizePane
        | MessageKind::ClientPaneInput
        | MessageKind::ClientRequestSnapshot
        | MessageKind::ClientImageCacheMiss
        | MessageKind::ClientAckApplied
        | MessageKind::ClientAckPresented
        | MessageKind::ClientPing
        | MessageKind::ClientGetLines
        | MessageKind::ClientScrollPane
        | MessageKind::ClientSetPaneTheme
        | MessageKind::ClientSetPaneCursorShape => Ok(()),
        _ => Err(PaneError::new("expected client message kind")),
    }
}

fn ensure_server_kind(kind: MessageKind) -> Result<(), PaneError> {
    match kind {
        MessageKind::ServerHello
        | MessageKind::ServerError
        | MessageKind::ServerTopology
        | MessageKind::ServerPaneUpdate
        | MessageKind::ServerPaneExited
        | MessageKind::ServerResyncRequired
        | MessageKind::ServerPong
        | MessageKind::ServerLines => Ok(()),
        _ => Err(PaneError::new("expected server message kind")),
    }
}

fn client_stream(message: &ClientMessage) -> StreamId {
    match message {
        ClientMessage::PaneInput { .. } => StreamId::INPUT,
        ClientMessage::ImageCacheMiss { .. } => StreamId::IMAGES,
        _ => StreamId::CONTROL,
    }
}

fn server_stream(message: &ServerMessage) -> StreamId {
    match message {
        ServerMessage::PaneUpdate(update) if !update.image_events.is_empty() => StreamId::IMAGES,
        ServerMessage::PaneUpdate(_) | ServerMessage::Lines(_) => StreamId::OUTPUT,
        _ => StreamId::CONTROL,
    }
}

fn server_seq(message: &ServerMessage) -> ServerSeq {
    match message {
        ServerMessage::PaneUpdate(update) => update.seq,
        ServerMessage::Lines(lines) => lines.seq,
        _ => ServerSeq(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seance_protocol::{CellAttrs, CellColor};

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
    fn in_process_transport_round_trips_serialized_client_message() {
        let (client, server) = InProcessTransport::pair();
        let message = ClientMessage::PaneInput {
            pane: pane_ref(),
            bytes: b"abc".to_vec(),
        };
        client
            .send(encode_client_frame(message.clone(), RequestId(1)).unwrap())
            .unwrap();

        let frame = server.try_recv().unwrap().unwrap();
        assert_eq!(frame.stream_id, StreamId::INPUT);
        assert_eq!(decode_client_frame(&frame).unwrap(), message);
        assert!(server.try_recv().unwrap().is_none());
    }

    #[test]
    fn in_process_transport_round_trips_serialized_pane_update() {
        let (client, server) = InProcessTransport::pair();
        let pane = pane_ref();
        let update = PaneUpdate {
            pane,
            seq: ServerSeq(7),
            image_events: Vec::new(),
            frame: Some(FrameDelta::Full {
                generation: 1,
                snapshot: snapshot(1, "x"),
            }),
        };
        server
            .send(encode_server_frame(ServerMessage::PaneUpdate(update.clone())).unwrap())
            .unwrap();

        let frame = client.try_recv().unwrap().unwrap();
        assert_eq!(frame.stream_id, StreamId::OUTPUT);
        assert_eq!(
            decode_server_frame(&frame).unwrap(),
            ServerMessage::PaneUpdate(update)
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
}
