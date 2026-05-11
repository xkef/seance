use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use seance_protocol::frame::{CursorShape, Resize, ThemeColors};
use seance_protocol::identity::{DomainId, DomainId as ProtocolDomainId, PaneRef};
use seance_protocol::mux::{ClientMessage, ProtocolErrorPayload, ServerMessage};
use seance_protocol::transport::{RequestId, Transport, decode_server_frame, encode_client_frame};

use crate::{Domain, DomainEvent, PaneError, PaneSpawnOptions, SpawnError};

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
