use serde::{Deserialize, Serialize};

use crate::frame::{CursorShape, FrameDelta, LineRange, Resize, RowDelta, ThemeColors};
use crate::identity::{
    DomainId, ImageKey, PaneRef, ServerId, ServerSeq, SessionId, TabId, WindowId,
};
use crate::image_cache::ImageCacheEvent;
use crate::limits::MAX_DECODED_MESSAGE_BYTES;
use crate::transport::{CodecError, RequestId};

pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);
pub const MIN_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ProtocolVersion(pub u16);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Capability {
    Zstd,
    FrameDelta,
    ImageCache,
    ImageChunks,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub min_version: ProtocolVersion,
    pub max_version: ProtocolVersion,
    pub capabilities: Vec<Capability>,
    pub max_message_bytes: u32,
    pub max_image_bytes: u64,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    pub version: ProtocolVersion,
    pub capabilities: Vec<Capability>,
    pub server_id: ServerId,
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolErrorKind {
    VersionMismatch,
    UnsupportedCapability,
    UnknownMessage,
    BadRoute,
    StalePane,
    NeedFull,
    FrameTooLarge,
    ImageTooLarge,
    ProtocolCorrupt,
    PaneExited,
    TransportEof,
    Detached,
    ServerPaneError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolErrorPayload {
    pub kind: ProtocolErrorKind,
    pub message: String,
    pub request_id: RequestId,
    pub pane: Option<PaneRef>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello(Hello),
    Subscribe {
        pane: Option<PaneRef>,
    },
    SpawnPane {
        domain: DomainId,
        cols: u16,
        rows: u16,
    },
    ClosePane {
        pane: PaneRef,
    },
    ResizePane {
        pane: PaneRef,
        resize: Resize,
    },
    PaneInput {
        pane: PaneRef,
        bytes: Vec<u8>,
    },
    RequestSnapshot {
        pane: PaneRef,
    },
    ImageCacheMiss {
        key: ImageKey,
    },
    AckApplied {
        pane: PaneRef,
        seq: ServerSeq,
    },
    AckPresented {
        pane: PaneRef,
        generation: u64,
    },
    Ping {
        nonce: u64,
    },
    GetLines {
        pane: PaneRef,
        range: LineRange,
        since_seq: Option<ServerSeq>,
    },
    ScrollPane {
        pane: PaneRef,
        delta: i32,
    },
    SetPaneTheme {
        pane: PaneRef,
        colors: ThemeColors,
    },
    SetPaneCursorShape {
        pane: PaneRef,
        shape: CursorShape,
    },
}

impl ClientMessage {
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

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    Hello(ServerHello),
    Error(ProtocolErrorPayload),
    Topology(Topology),
    PaneUpdate(PaneUpdate),
    PaneExited {
        pane: PaneRef,
        exit_status: Option<i32>,
    },
    ResyncRequired {
        pane: PaneRef,
        reason: String,
    },
    Pong {
        nonce: u64,
    },
    Lines(LineContent),
}

impl ServerMessage {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineContent {
    pub pane: PaneRef,
    pub seq: ServerSeq,
    pub generation: u64,
    pub cols: u16,
    pub range: LineRange,
    pub rows: Vec<RowDelta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    pub domains: Vec<DomainInfo>,
    pub windows: Vec<WindowInfo>,
    pub tabs: Vec<TabInfo>,
    pub panes: Vec<PaneInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainInfo {
    pub domain_id: DomainId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub window_id: WindowId,
    pub domain_id: DomainId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    pub tab_id: TabId,
    pub window_id: WindowId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane: PaneRef,
    pub tab_id: TabId,
    pub cols: u16,
    pub rows: u16,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneUpdate {
    pub pane: PaneRef,
    pub seq: ServerSeq,
    pub image_events: Vec<ImageCacheEvent>,
    pub frame: Option<FrameDelta>,
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
