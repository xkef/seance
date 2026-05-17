use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use seance_protocol::frame::{CursorShape, Resize, ThemeColors};
use seance_protocol::identity::{DomainId, DomainId as ProtocolDomainId, PaneRef};
use seance_protocol::mux::{ClientMessage, ProtocolErrorPayload, ServerMessage};
use seance_protocol::transport::{
    RequestId, Transport, decode_server_frame_with_request, encode_client_frame,
};

use crate::{Domain, DomainEvent, PaneError, PaneSpawnOptions, SpawnError};

const SPAWN_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const SPAWN_POLL_INTERVAL: Duration = Duration::from_micros(200);

/// Client-side [`Domain`] impl that talks to a server over an arbitrary
/// [`Transport`].
///
/// `spawn_pane` is the only operation that requires a round-trip — every
/// other method enqueues a client frame and returns. Server pushes drain
/// through `drain_events`. Other server frames that arrive while
/// `spawn_pane` is awaiting its response are buffered and re-emitted on the
/// next `drain_events` call so the host never loses an event.
pub struct ProtocolDomain<T> {
    transport: T,
    domain: ProtocolDomainId,
    next_request_id: AtomicU64,
    pending: VecDeque<DomainEvent>,
}

impl<T> ProtocolDomain<T> {
    /// Construct with the default `DomainId(1)`; matches `LocalDomain::new`.
    pub fn new(transport: T) -> Self {
        Self::with_domain(transport, DomainId(1))
    }

    /// Construct with an explicit `DomainId`. Use when the host owns more
    /// than one Domain instance and per-domain id namespacing matters.
    pub fn with_domain(transport: T, domain: DomainId) -> Self {
        Self {
            transport,
            domain,
            next_request_id: AtomicU64::new(1),
            pending: VecDeque::new(),
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
    fn send_client_message_with_request(
        &mut self,
        message: ClientMessage,
        request_id: RequestId,
    ) -> Result<(), PaneError> {
        let frame = encode_client_frame(message, request_id)?;
        self.transport.send(frame).map_err(Into::into)
    }

    fn send_client_message(&mut self, message: ClientMessage) -> Result<(), PaneError> {
        self.send_client_message_with_request(message, self.next_request_id())
    }

    /// Block until a [`ServerMessage`] tagged with `request_id` arrives, then
    /// return it. Intervening server pushes are queued onto `self.pending`
    /// so the next `drain_events` re-emits them in order.
    fn await_response(&mut self, request_id: RequestId) -> Result<ServerMessage, PaneError> {
        let deadline = Instant::now() + SPAWN_RESPONSE_TIMEOUT;
        loop {
            match self.transport.try_recv()? {
                Some(frame) => {
                    let (frame_request_id, message) = decode_server_frame_with_request(&frame)?;
                    if frame_request_id == request_id {
                        return Ok(message);
                    }
                    for event in server_message_to_events(message) {
                        self.pending.push_back(event);
                    }
                }
                None => {
                    if Instant::now() >= deadline {
                        return Err(PaneError::new("timed out waiting for server response"));
                    }
                    thread::sleep(SPAWN_POLL_INTERVAL);
                }
            }
        }
    }
}

impl<T: Transport> Domain for ProtocolDomain<T> {
    fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError> {
        let request_id = self.next_request_id();
        self.send_client_message_with_request(
            ClientMessage::SpawnPane {
                domain: self.domain,
                cols: options.cols,
                rows: options.rows,
            },
            request_id,
        )?;
        match self.await_response(request_id)? {
            ServerMessage::Topology(topology) => topology
                .panes
                .first()
                .map(|info| info.pane)
                .ok_or_else(|| SpawnError::new("server topology reply contained no panes")),
            ServerMessage::Error(ProtocolErrorPayload { message, .. }) => {
                Err(SpawnError::new(message))
            }
            other => Err(SpawnError::new(format!(
                "unexpected response to spawn: {:?}",
                other.kind()
            ))),
        }
    }

    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError> {
        while let Some(event) = self.pending.pop_front() {
            sink(event);
        }
        while let Some(frame) = self.transport.try_recv()? {
            let (_request_id, message) = decode_server_frame_with_request(&frame)?;
            for event in server_message_to_events(message) {
                sink(event);
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

fn server_message_to_events(message: ServerMessage) -> Vec<DomainEvent> {
    match message {
        ServerMessage::PaneUpdate(update) => vec![DomainEvent::PaneUpdate(update)],
        ServerMessage::PaneExited { pane, .. } => vec![DomainEvent::PaneExited { pane }],
        ServerMessage::PaneClipboardRequest { pane, request } => {
            vec![DomainEvent::ClipboardRequest { pane, request }]
        }
        ServerMessage::ResyncRequired { pane, reason } => vec![DomainEvent::Error {
            pane: Some(pane),
            message: reason,
        }],
        ServerMessage::Error(ProtocolErrorPayload { pane, message, .. }) => {
            vec![DomainEvent::Error { pane, message }]
        }
        ServerMessage::Hello(_)
        | ServerMessage::Topology(_)
        | ServerMessage::Pong { .. }
        | ServerMessage::Lines(_) => Vec::new(),
    }
}
