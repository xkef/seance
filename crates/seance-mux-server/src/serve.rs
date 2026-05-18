use std::fmt;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use seance_mux_client::{Domain, DomainEvent, MuxEvent, PaneError, PaneSpawnOptions};
use seance_protocol::mux::{
    ClientMessage, DomainInfo, PaneInfo, ProtocolErrorKind, ProtocolErrorPayload, ServerMessage,
    TabInfo, Topology, WindowInfo,
};
use seance_protocol::transport::{
    InProcessTransport, RequestId, SERVE_IDLE_BACKOFF_MS, Transport, TransportError,
    decode_client_frame_with_request, encode_server_frame, encode_server_frame_with_request,
};

use crate::LocalDomain;

/// Knobs handed to [`serve`].
pub struct ServeConfig {
    /// Called every time the server loop produces an outbound frame so the
    /// host can wake its event loop (winit proxy, IO reactor, etc).
    pub wake: Box<dyn Fn() + Send + Sync>,
}

impl ServeConfig {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            wake: Box::new(wake),
        }
    }
}

/// Reasons the [`serve`] loop returns control to its caller.
#[derive(Debug)]
pub enum ServeError {
    /// The transport closed; no more frames can flow either way.
    TransportClosed,
    /// A [`Domain`] method returned an unrecoverable error.
    Pane(PaneError),
    /// Decoding an inbound frame failed in a way the loop could not recover
    /// from. Recoverable per-frame errors are reported to the client via
    /// [`ServerMessage::Error`] instead.
    Codec(String),
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransportClosed => f.write_str("transport closed"),
            Self::Pane(err) => write!(f, "pane error: {err}"),
            Self::Codec(msg) => write!(f, "codec error: {msg}"),
        }
    }
}

impl std::error::Error for ServeError {}

impl From<PaneError> for ServeError {
    fn from(value: PaneError) -> Self {
        Self::Pane(value)
    }
}

impl From<TransportError> for ServeError {
    fn from(_: TransportError) -> Self {
        Self::TransportClosed
    }
}

/// Drive a [`Domain`] over a [`Transport`].
///
/// Single-threaded blocking loop. Each tick:
/// 1. Drains inbound client frames, dispatching each to `domain`. Replies
///    that correlate to a specific request (currently
///    [`ClientMessage::SpawnPane`] → [`ServerMessage::Topology`]) are tagged
///    with the originating [`RequestId`].
/// 2. Drains outbound events from `domain.drain_events`, encoding each as
///    a unilateral [`ServerMessage`] push.
/// 3. Calls `config.wake` for every frame sent so the host event loop knows
///    to refresh.
/// 4. Parks briefly when both directions are idle.
///
/// Exits when the transport is closed or a `Domain` method fails. Domain
/// errors that target a specific pane are reported back as
/// [`ServerMessage::Error`] without terminating the loop.
pub fn serve<D, T>(mut domain: D, transport: T, config: ServeConfig) -> Result<(), ServeError>
where
    D: Domain,
    T: Transport,
{
    loop {
        let mut did_work = false;

        loop {
            match transport.try_recv() {
                Ok(Some(frame)) => {
                    did_work = true;
                    let (request_id, message) = match decode_client_frame_with_request(&frame) {
                        Ok(value) => value,
                        Err(err) => {
                            let payload = ServerMessage::Error(ProtocolErrorPayload {
                                kind: ProtocolErrorKind::ProtocolCorrupt,
                                message: err.to_string(),
                                request_id: RequestId::PUSH,
                                pane: None,
                            });
                            send_unilateral(&transport, &config, payload)?;
                            continue;
                        }
                    };
                    dispatch_client_message(&mut domain, &transport, &config, request_id, message)?;
                }
                Ok(None) => break,
                Err(_) => return Err(ServeError::TransportClosed),
            }
        }

        let mut events = Vec::new();
        domain.drain_events(&mut |event| events.push(event))?;
        for event in events {
            did_work = true;
            for message in domain_event_to_messages(event) {
                send_unilateral(&transport, &config, message)?;
            }
        }

        if !did_work {
            thread::sleep(Duration::from_millis(SERVE_IDLE_BACKOFF_MS));
        }
    }
}

/// In-process bootstrap: spawn a [`LocalDomain`] on a dedicated server
/// thread, wire it to a fresh [`InProcessTransport`] pair, and return the
/// client end ready to hand to [`seance_mux_client::ProtocolDomain`].
///
/// `wake` is called by the server thread for every outbound frame and is
/// the seam to the host's event loop (typically `EventLoopProxy::send_event`
/// in a winit application).
pub fn spawn_local_server<W>(wake: W) -> (InProcessTransport, JoinHandle<()>)
where
    W: Fn() + Send + Sync + 'static,
{
    let (client_transport, server_transport) = InProcessTransport::pair();
    let wake_arc: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    let wake_for_domain = Arc::clone(&wake_arc);
    let domain = LocalDomain::new(move |event| match event {
        // The polling serve loop picks up events on its next tick; the wake
        // is only required so the frontend re-renders once the server has
        // pushed a frame. The domain-side wake remains a no-op here.
        MuxEvent::Wake => {
            let _ = &wake_for_domain;
        }
    });
    let wake_for_config = Arc::clone(&wake_arc);
    let config = ServeConfig::new(move || (wake_for_config)());
    let join = thread::spawn(move || {
        if let Err(err) = serve(domain, server_transport, config) {
            match err {
                ServeError::TransportClosed => {}
                other => tracing::warn!(error = %other, "server loop exited"),
            }
        }
    });
    (client_transport, join)
}

fn dispatch_client_message<D, T>(
    domain: &mut D,
    transport: &T,
    config: &ServeConfig,
    request_id: RequestId,
    message: ClientMessage,
) -> Result<(), ServeError>
where
    D: Domain,
    T: Transport,
{
    match message {
        ClientMessage::SpawnPane {
            domain: _,
            cols,
            rows,
            pixel_width,
            pixel_height,
            initial_cursor_shape,
            max_scrollback,
        } => {
            let options = PaneSpawnOptions {
                cols,
                rows,
                pixel_width,
                pixel_height,
                initial_cursor_shape,
                max_scrollback: usize::try_from(max_scrollback).unwrap_or(usize::MAX),
            };
            match domain.spawn_pane(options) {
                Ok(pane_ref) => {
                    let topology = Topology {
                        domains: vec![DomainInfo {
                            domain_id: pane_ref.domain,
                            name: "local".to_string(),
                        }],
                        windows: vec![WindowInfo {
                            window_id: Default::default(),
                            domain_id: pane_ref.domain,
                        }],
                        tabs: vec![TabInfo {
                            tab_id: Default::default(),
                            window_id: Default::default(),
                        }],
                        panes: vec![PaneInfo {
                            pane: pane_ref,
                            tab_id: Default::default(),
                            cols,
                            rows,
                            title: String::new(),
                        }],
                    };
                    send_response(
                        transport,
                        config,
                        request_id,
                        ServerMessage::Topology(topology),
                    )?;
                }
                Err(err) => send_response(
                    transport,
                    config,
                    request_id,
                    ServerMessage::Error(ProtocolErrorPayload {
                        kind: ProtocolErrorKind::ServerPaneError,
                        message: err.to_string(),
                        request_id,
                        pane: None,
                    }),
                )?,
            }
        }
        ClientMessage::PaneInput { pane, bytes } => {
            if let Err(err) = domain.write(pane, bytes.into()) {
                report_pane_error(transport, config, request_id, pane, err)?;
            }
        }
        ClientMessage::ResizePane { pane, resize } => {
            if let Err(err) = domain.resize(pane, resize) {
                report_pane_error(transport, config, request_id, pane, err)?;
            }
        }
        ClientMessage::ScrollPane { pane, delta } => {
            if let Err(err) = domain.scroll_lines(pane, delta) {
                report_pane_error(transport, config, request_id, pane, err)?;
            }
        }
        ClientMessage::SetPaneTheme { pane, colors } => {
            if let Err(err) = domain.set_theme_colors(pane, colors) {
                report_pane_error(transport, config, request_id, pane, err)?;
            }
        }
        ClientMessage::SetPaneCursorShape { pane, shape } => {
            if let Err(err) = domain.set_cursor_shape(pane, shape) {
                report_pane_error(transport, config, request_id, pane, err)?;
            }
        }
        ClientMessage::AckPresented { pane, generation } => {
            if let Err(err) = domain.ack_presented(pane, generation) {
                report_pane_error(transport, config, request_id, pane, err)?;
            }
        }
        ClientMessage::Ping { nonce } => {
            send_response(transport, config, request_id, ServerMessage::Pong { nonce })?;
        }
        ClientMessage::ClosePane { .. }
        | ClientMessage::Hello(_)
        | ClientMessage::Subscribe { .. }
        | ClientMessage::RequestSnapshot { .. }
        | ClientMessage::ImageCacheMiss { .. }
        | ClientMessage::AckApplied { .. }
        | ClientMessage::GetLines { .. } => {
            // Phase-2-era operations: the loop accepts and ignores them so
            // future clients don't get spurious errors during rollout.
        }
    }
    Ok(())
}

fn domain_event_to_messages(event: DomainEvent) -> Vec<ServerMessage> {
    match event {
        DomainEvent::PaneUpdate(update) => vec![ServerMessage::PaneUpdate(update)],
        DomainEvent::PaneExited { pane } => vec![ServerMessage::PaneExited {
            pane,
            exit_status: None,
        }],
        DomainEvent::ClipboardRequest { pane, request } => {
            vec![ServerMessage::PaneClipboardRequest { pane, request }]
        }
        DomainEvent::Error { pane, message } => vec![ServerMessage::Error(ProtocolErrorPayload {
            kind: ProtocolErrorKind::ServerPaneError,
            message,
            request_id: RequestId::PUSH,
            pane,
        })],
    }
}

fn send_unilateral<T: Transport>(
    transport: &T,
    config: &ServeConfig,
    message: ServerMessage,
) -> Result<(), ServeError> {
    let frame = encode_server_frame(message).map_err(|err| ServeError::Codec(err.to_string()))?;
    transport.send(frame)?;
    (config.wake)();
    Ok(())
}

fn send_response<T: Transport>(
    transport: &T,
    config: &ServeConfig,
    request_id: RequestId,
    message: ServerMessage,
) -> Result<(), ServeError> {
    let frame = encode_server_frame_with_request(message, request_id)
        .map_err(|err| ServeError::Codec(err.to_string()))?;
    transport.send(frame)?;
    (config.wake)();
    Ok(())
}

fn report_pane_error<T: Transport>(
    transport: &T,
    config: &ServeConfig,
    request_id: RequestId,
    pane: seance_protocol::identity::PaneRef,
    err: PaneError,
) -> Result<(), ServeError> {
    send_response(
        transport,
        config,
        request_id,
        ServerMessage::Error(ProtocolErrorPayload {
            kind: ProtocolErrorKind::ServerPaneError,
            message: err.to_string(),
            request_id,
            pane: Some(pane),
        }),
    )
}
