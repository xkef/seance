//! Top-level [`ClientMessage`] / [`ServerMessage`] and server topology
//! types.

use serde::{Deserialize, Serialize};

use crate::frame::{CursorShape, FrameDelta, LineRange, Resize, RowDelta, ThemeColors};
use crate::identity::{
    DomainId, ImageKey, PaneRef, ServerId, ServerSeq, SessionId, TabId, WindowId,
};
use crate::image_cache::ImageCacheEvent;
use crate::limits::MAX_DECODED_MESSAGE_BYTES;
use crate::transport::{CodecError, RequestId};

/// Protocol version this build emits in [`Hello`] / [`ServerHello`].
pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);
/// Oldest peer protocol version this build accepts during handshake.
pub const MIN_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

/// Protocol revision negotiated during handshake.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ProtocolVersion(#[allow(missing_docs)] pub u16);

/// Optional features either side may negotiate during [`Hello`] /
/// [`ServerHello`]. Both peers must advertise a capability for it to be
/// active.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Capability {
    /// Zstd compression on the length-prefixed framing.
    Zstd,
    /// Server may emit [`FrameDelta::Partial`] instead of resending full
    /// snapshots on every update.
    FrameDelta,
    /// Server-driven image cache via [`ImageCacheEvent`].
    ImageCache,
    /// Image payloads delivered in
    /// [`crate::image_cache::ImagePutChunk`] segments rather than a
    /// single [`crate::image_cache::ImagePayload`].
    ImageChunks,
    /// Resume an existing session after transport reconnect using
    /// [`Hello::last_seen_seq`].
    Resume,
}

/// Client-side handshake. Advertises the protocol-version range and
/// capabilities the client supports, plus a resume hint when
/// reconnecting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    #[allow(missing_docs)]
    pub min_version: ProtocolVersion,
    #[allow(missing_docs)]
    pub max_version: ProtocolVersion,
    #[allow(missing_docs)]
    pub capabilities: Vec<Capability>,
    #[allow(missing_docs)]
    pub max_message_bytes: u32,
    #[allow(missing_docs)]
    pub max_image_bytes: u64,
    /// Last [`ServerSeq`] the client previously observed; `Some`
    /// requests resume of an existing session, `None` starts fresh.
    pub last_seen_seq: Option<ServerSeq>,
}

impl Default for Hello {
    fn default() -> Self {
        Self {
            min_version: MIN_PROTOCOL_VERSION,
            max_version: CURRENT_PROTOCOL_VERSION,
            capabilities: vec![Capability::FrameDelta, Capability::ImageCache],
            max_message_bytes: u32::try_from(MAX_DECODED_MESSAGE_BYTES).unwrap_or(u32::MAX),
            max_image_bytes: 64 * 1024 * 1024,
            last_seen_seq: None,
        }
    }
}

/// Server-side handshake reply. The chosen `version` is the highest
/// version both peers support, and `capabilities` is their intersection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    #[allow(missing_docs)]
    pub version: ProtocolVersion,
    #[allow(missing_docs)]
    pub capabilities: Vec<Capability>,
    #[allow(missing_docs)]
    pub server_id: ServerId,
    #[allow(missing_docs)]
    pub session_id: SessionId,
}

/// Categorical reason a server-emitted [`ServerMessage::Error`] was
/// raised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolErrorKind {
    /// No overlap between client and server protocol-version ranges.
    VersionMismatch,
    /// Client requested a capability the server does not implement.
    UnsupportedCapability,
    /// Envelope kind tag was unknown to the server.
    UnknownMessage,
    /// Message addressed a pane the server does not own.
    BadRoute,
    /// Pane reference was stale (epoch mismatch).
    StalePane,
    /// Server cannot serve a partial delta; client must request a full
    /// snapshot.
    NeedFull,
    /// Outbound frame would exceed the negotiated frame budget.
    FrameTooLarge,
    /// Image upload would exceed the negotiated image budget.
    ImageTooLarge,
    /// Wire bytes failed framing or codec validation.
    ProtocolCorrupt,
    /// Client referenced a pane whose process has exited.
    PaneExited,
    /// Underlying transport reached end-of-file.
    TransportEof,
    /// Client detached cleanly; reattach is allowed.
    Detached,
    /// Pane-local error originating on the server.
    ServerPaneError,
}

/// Body of [`ServerMessage::Error`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolErrorPayload {
    #[allow(missing_docs)]
    pub kind: ProtocolErrorKind,
    /// Human-readable explanation for logging; not for parsing.
    pub message: String,
    /// `RequestId` of the failed request, or [`RequestId::PUSH`] for
    /// server-initiated errors.
    pub request_id: RequestId,
    /// Pane the error pertains to, when applicable.
    pub pane: Option<PaneRef>,
}

/// Top-level client-to-server message. Each variant maps 1:1 to a
/// [`MessageKind`] tag on the wire.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Client handshake — must be the first message on a fresh
    /// transport.
    Hello(#[allow(missing_docs)] Hello),
    /// Subscribe to push updates for `pane`, or all panes when `None`.
    Subscribe {
        #[allow(missing_docs)]
        pane: Option<PaneRef>,
    },
    /// Spawn a new pane inside `domain` at the given grid size.
    SpawnPane {
        #[allow(missing_docs)]
        domain: DomainId,
        #[allow(missing_docs)]
        cols: u16,
        #[allow(missing_docs)]
        rows: u16,
    },
    /// Request that `pane` exit and be reaped.
    ClosePane {
        #[allow(missing_docs)]
        pane: PaneRef,
    },
    /// Resize `pane` to the geometry in `resize`.
    ResizePane {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        resize: Resize,
    },
    /// PTY input bytes destined for `pane`.
    PaneInput {
        #[allow(missing_docs)]
        pane: PaneRef,
        /// Raw bytes; capped per message by
        /// [`crate::limits::MAX_PTY_INPUT_BYTES`].
        bytes: Vec<u8>,
    },
    /// Force the server to emit a [`FrameDelta::Full`] for `pane`.
    RequestSnapshot {
        #[allow(missing_docs)]
        pane: PaneRef,
    },
    /// The client does not have `key` cached; ask the server to resend
    /// it.
    ImageCacheMiss {
        #[allow(missing_docs)]
        key: ImageKey,
    },
    /// Acknowledge that the client has applied frames up to `seq`.
    /// Used for resume bookkeeping and outbound buffer reclamation.
    AckApplied {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        seq: ServerSeq,
    },
    /// Acknowledge that frame `generation` has been presented.
    AckPresented {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        generation: u64,
    },
    /// Round-trip latency / liveness probe. Server replies with
    /// [`ServerMessage::Pong`] echoing `nonce`.
    Ping {
        #[allow(missing_docs)]
        nonce: u64,
    },
    /// Fetch a contiguous range of scrollback lines for `pane`.
    GetLines {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        range: LineRange,
        #[allow(missing_docs)]
        since_seq: Option<ServerSeq>,
    },
    /// Scroll `pane`'s viewport by `delta` rows (negative scrolls back).
    ScrollPane {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        delta: i32,
    },
    /// Replace `pane`'s theme.
    SetPaneTheme {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        colors: ThemeColors,
    },
    /// Override `pane`'s cursor shape.
    SetPaneCursorShape {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        shape: CursorShape,
    },
}

impl ClientMessage {
    /// The numeric [`MessageKind`] tag this variant encodes as.
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Hello(_) => MessageKind::ClientHello,
            Self::Subscribe { .. } => MessageKind::ClientSubscribe,
            Self::SpawnPane { .. } => MessageKind::ClientSpawnPane,
            Self::ClosePane { .. } => MessageKind::ClientClosePane,
            Self::ResizePane { .. } => MessageKind::ClientResizePane,
            Self::ScrollPane { .. } => MessageKind::ClientScrollPane,
            Self::SetPaneTheme { .. } => MessageKind::ClientSetPaneTheme,
            Self::SetPaneCursorShape { .. } => MessageKind::ClientSetPaneCursorShape,
            Self::PaneInput { .. } => MessageKind::ClientPaneInput,
            Self::RequestSnapshot { .. } => MessageKind::ClientRequestSnapshot,
            Self::ImageCacheMiss { .. } => MessageKind::ClientImageCacheMiss,
            Self::AckApplied { .. } => MessageKind::ClientAckApplied,
            Self::AckPresented { .. } => MessageKind::ClientAckPresented,
            Self::Ping { .. } => MessageKind::ClientPing,
            Self::GetLines { .. } => MessageKind::ClientGetLines,
        }
    }
}

/// Top-level server-to-client message. Each variant maps 1:1 to a
/// [`MessageKind`] tag on the wire.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Reply to [`ClientMessage::Hello`] — chosen version + capability
    /// set.
    Hello(#[allow(missing_docs)] ServerHello),
    /// Out-of-band error report.
    Error(#[allow(missing_docs)] ProtocolErrorPayload),
    /// Current domain/window/tab/pane layout.
    Topology(#[allow(missing_docs)] Topology),
    /// Per-pane sequenced update bundling image events and a frame
    /// delta.
    PaneUpdate(#[allow(missing_docs)] PaneUpdate),
    /// `pane`'s process exited; `exit_status` is the OS exit code if
    /// the server captured one.
    PaneExited {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        exit_status: Option<i32>,
    },
    /// Server's view of `pane` diverged from the client's; the client
    /// must drop its cache and re-request a full snapshot.
    ResyncRequired {
        #[allow(missing_docs)]
        pane: PaneRef,
        #[allow(missing_docs)]
        reason: String,
    },
    /// Reply to [`ClientMessage::Ping`], echoing `nonce`.
    Pong {
        #[allow(missing_docs)]
        nonce: u64,
    },
    /// Reply to [`ClientMessage::GetLines`].
    Lines(#[allow(missing_docs)] LineContent),
}

impl ServerMessage {
    /// The numeric [`MessageKind`] tag this variant encodes as.
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Hello(_) => MessageKind::ServerHello,
            Self::Error(_) => MessageKind::ServerError,
            Self::Topology(_) => MessageKind::ServerTopology,
            Self::PaneUpdate(_) => MessageKind::ServerPaneUpdate,
            Self::PaneExited { .. } => MessageKind::ServerPaneExited,
            Self::ResyncRequired { .. } => MessageKind::ServerResyncRequired,
            Self::Pong { .. } => MessageKind::ServerPong,
            Self::Lines(_) => MessageKind::ServerLines,
        }
    }
}

/// Body of [`ServerMessage::Lines`]: a contiguous run of scrollback
/// rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineContent {
    #[allow(missing_docs)]
    pub pane: PaneRef,
    #[allow(missing_docs)]
    pub seq: ServerSeq,
    #[allow(missing_docs)]
    pub generation: u64,
    #[allow(missing_docs)]
    pub cols: u16,
    #[allow(missing_docs)]
    pub range: LineRange,
    /// Rows in `range`, each self-contained.
    pub rows: Vec<RowDelta>,
}

/// Server-side view of the multiplexer hierarchy: domains contain
/// windows, windows contain tabs, tabs contain panes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    #[allow(missing_docs)]
    pub domains: Vec<DomainInfo>,
    #[allow(missing_docs)]
    pub windows: Vec<WindowInfo>,
    #[allow(missing_docs)]
    pub tabs: Vec<TabInfo>,
    #[allow(missing_docs)]
    pub panes: Vec<PaneInfo>,
}

/// Server-side description of one multiplexer domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainInfo {
    #[allow(missing_docs)]
    pub domain_id: DomainId,
    #[allow(missing_docs)]
    pub name: String,
}

/// Server-side description of one window and its parent domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    #[allow(missing_docs)]
    pub window_id: WindowId,
    #[allow(missing_docs)]
    pub domain_id: DomainId,
}

/// Server-side description of one tab and its parent window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    #[allow(missing_docs)]
    pub tab_id: TabId,
    #[allow(missing_docs)]
    pub window_id: WindowId,
}

/// Server-side description of one pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    #[allow(missing_docs)]
    pub pane: PaneRef,
    #[allow(missing_docs)]
    pub tab_id: TabId,
    #[allow(missing_docs)]
    pub cols: u16,
    #[allow(missing_docs)]
    pub rows: u16,
    #[allow(missing_docs)]
    pub title: String,
}

/// Server-pushed pane update: image cache mutations and an optional
/// frame. `seq` is monotonic per pane and is what the client
/// acknowledges with [`ClientMessage::AckApplied`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneUpdate {
    #[allow(missing_docs)]
    pub pane: PaneRef,
    #[allow(missing_docs)]
    pub seq: ServerSeq,
    /// Image cache mutations to apply before consuming `frame`.
    pub image_events: Vec<ImageCacheEvent>,
    /// Frame delta, if this update carries grid changes.
    pub frame: Option<FrameDelta>,
}

/// Numeric tag identifying an envelope's payload variant on the wire.
/// Values 1-999 are reserved for client messages, 1000+ for server.
/// Values are append-only — never reuse a number.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum MessageKind {
    ClientHello = 1,
    ClientSubscribe = 2,
    ClientSpawnPane = 3,
    ClientClosePane = 4,
    ClientResizePane = 5,
    ClientPaneInput = 6,
    ClientRequestSnapshot = 7,
    ClientImageCacheMiss = 8,
    ClientAckApplied = 9,
    ClientAckPresented = 10,
    ClientPing = 11,
    ClientGetLines = 12,
    ClientScrollPane = 13,
    ClientSetPaneTheme = 14,
    ClientSetPaneCursorShape = 15,
    ServerHello = 1001,
    ServerError = 1002,
    ServerTopology = 1003,
    ServerPaneUpdate = 1004,
    ServerPaneExited = 1005,
    ServerResyncRequired = 1006,
    ServerPong = 1007,
    ServerLines = 1008,
}

impl TryFrom<u16> for MessageKind {
    type Error = CodecError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        let kind = match value {
            1 => Self::ClientHello,
            2 => Self::ClientSubscribe,
            3 => Self::ClientSpawnPane,
            4 => Self::ClientClosePane,
            5 => Self::ClientResizePane,
            6 => Self::ClientPaneInput,
            7 => Self::ClientRequestSnapshot,
            8 => Self::ClientImageCacheMiss,
            9 => Self::ClientAckApplied,
            10 => Self::ClientAckPresented,
            11 => Self::ClientPing,
            12 => Self::ClientGetLines,
            13 => Self::ClientScrollPane,
            14 => Self::ClientSetPaneTheme,
            15 => Self::ClientSetPaneCursorShape,
            1001 => Self::ServerHello,
            1002 => Self::ServerError,
            1003 => Self::ServerTopology,
            1004 => Self::ServerPaneUpdate,
            1005 => Self::ServerPaneExited,
            1006 => Self::ServerResyncRequired,
            1007 => Self::ServerPong,
            1008 => Self::ServerLines,
            other => return Err(CodecError::UnknownMessage(other)),
        };
        Ok(kind)
    }
}

impl From<MessageKind> for u16 {
    fn from(value: MessageKind) -> Self {
        value as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde::{Serialize, de::DeserializeOwned};
    use std::fmt;

    use crate::frame::{
        CellAttrs, CellColor, CursorInfo, DirtySnapshot, GridPos, PlacementSnapshot, RowMeta,
        SnapshotImage, VtSnapshot,
    };
    use crate::identity::{ImageId, PaneEpoch, PaneId};
    use crate::image_cache::{ImageFormat, ImagePayload, ImagePutChunk};
    use crate::transport::{RequestId, decode_payload, encode_payload};

    fn pane() -> PaneRef {
        PaneRef {
            pane_id: PaneId(9),
            epoch: PaneEpoch(1),
        }
    }

    fn snapshot(cols: u16, rows: u16, generation: u64, texts: &[&str]) -> VtSnapshot {
        let mut snapshot = VtSnapshot::empty(cols, rows);
        snapshot.generation = generation;
        for text in texts {
            snapshot.push_cell(
                text,
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            );
        }
        snapshot
    }

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + DeserializeOwned + PartialEq + fmt::Debug,
    {
        let bytes = encode_payload(value).unwrap();
        let decoded = decode_payload(&bytes).unwrap();
        assert_eq!(&decoded, value);
        decoded
    }

    #[test]
    fn protocol_payloads_round_trip_through_postcard() {
        let pane = pane();
        let resize = Resize {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
        };
        round_trip(&ClientMessage::PaneInput {
            pane,
            bytes: b"abc".to_vec(),
        });
        round_trip(&ClientMessage::ResizePane { pane, resize });
        round_trip(&ClientMessage::ScrollPane { pane, delta: -3 });
        round_trip(&ClientMessage::SetPaneTheme {
            pane,
            colors: ThemeColors {
                fg: [1, 2, 3],
                bg: [4, 5, 6],
                cursor: [7, 8, 9],
                palette: [[0, 0, 0]; 256],
            },
        });
        round_trip(&ClientMessage::SetPaneCursorShape {
            pane,
            shape: CursorShape::Underline,
        });
        round_trip(&ClientMessage::GetLines {
            pane,
            range: LineRange {
                start: 0,
                count: 24,
            },
            since_seq: Some(ServerSeq(3)),
        });
        round_trip(&ServerMessage::Hello(ServerHello {
            version: ProtocolVersion(1),
            capabilities: vec![Capability::FrameDelta],
            server_id: ServerId(1),
            session_id: SessionId(2),
        }));
        round_trip(&ServerMessage::Error(ProtocolErrorPayload {
            kind: ProtocolErrorKind::NeedFull,
            message: "base missing".into(),
            request_id: RequestId(7),
            pane: Some(pane),
        }));
        round_trip(&ServerMessage::Lines(LineContent {
            pane,
            seq: ServerSeq(4),
            generation: 9,
            cols: 1,
            range: LineRange { start: 0, count: 1 },
            rows: vec![RowDelta::from_snapshot_row(&snapshot(1, 1, 9, &["x"]), 0).unwrap()],
        }));
    }

    #[test]
    fn snapshot_and_image_protocol_types_round_trip() {
        let mut snap = snapshot(2, 1, 3, &["α", "b"]);
        snap.cursor = CursorInfo {
            pos: GridPos { col: 1, row: 0 },
            visible: true,
            wide: false,
            shape: Some(CursorShape::Underline),
        };
        snap.modes.bracketed_paste = true;
        let pwd = std::env::temp_dir().join("seance-protocol-test-pwd");
        snap.pwd = Some(pwd.to_string_lossy().into_owned());
        snap.rows_meta[0] = RowMeta {
            wrap: true,
            wrap_continuation: false,
        };
        snap.dirty = DirtySnapshot::Partial(vec![0]);
        snap.placements.push(PlacementSnapshot {
            image_id: ImageId(42),
            placement_id: 1,
            viewport_col: 0,
            viewport_row: 0,
            pixel_width: 10,
            pixel_height: 10,
            source_x: 0,
            source_y: 0,
            source_width: 10,
            source_height: 10,
            image_width: 10,
            image_height: 10,
            z: 0,
        });
        snap.images.push(SnapshotImage {
            image_id: ImageId(42),
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        });
        round_trip(&snap);

        let key = ImageKey {
            pane: pane(),
            image_id: ImageId(42),
        };
        round_trip(&ImageCacheEvent::Put(ImagePayload {
            key,
            width: 1,
            height: 1,
            byte_len: 4,
            format: ImageFormat::Rgba8,
            digest: [5; 32],
            rgba: vec![1, 2, 3, 4],
        }));
        round_trip(&ImageCacheEvent::PutChunk(ImagePutChunk {
            key,
            offset: 0,
            bytes: vec![1, 2],
        }));
        round_trip(&ImageCacheEvent::Evict { key });
    }
}
