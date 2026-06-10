use serde::{Deserialize, Serialize};

use crate::clipboard::ClipboardRequest;
use crate::frame::{CursorShape, FrameDelta, Resize, ThemeColors};
use crate::identity::{DomainId, PaneRef, ServerSeq, TabId, WindowId};
use crate::image_cache::ImageCacheEvent;
use crate::transport::{CodecError, RequestId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolErrorKind {
    ProtocolCorrupt,
    ServerPaneError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolErrorPayload {
    pub kind: ProtocolErrorKind,
    pub message: String,
    pub request_id: RequestId,
    pub pane: Option<PaneRef>,
}

/// Every message a client can send.
///
/// Most variants are fire-and-forget — the only one that requires a
/// response is [`ClientMessage::SpawnPane`], which the server answers
/// with a [`ServerMessage::Topology`] tagged with the originating
/// [`RequestId`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    SpawnPane {
        domain: DomainId,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
        initial_cursor_shape: CursorShape,
        /// `usize` on the application side; sent as `u64` so the wire stays
        /// architecture-independent. Server clamps to `usize::MAX` on the
        /// receive side.
        max_scrollback: u64,
    },
    ResizePane {
        pane: PaneRef,
        resize: Resize,
    },
    PaneInput {
        pane: PaneRef,
        bytes: Vec<u8>,
    },
    AckPresented {
        pane: PaneRef,
        generation: u64,
    },
    Ping {
        nonce: u64,
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
            Self::SpawnPane { .. } => MessageKind::ClientSpawnPane,
            Self::ResizePane { .. } => MessageKind::ClientResizePane,
            Self::ScrollPane { .. } => MessageKind::ClientScrollPane,
            Self::SetPaneTheme { .. } => MessageKind::ClientSetPaneTheme,
            Self::SetPaneCursorShape { .. } => MessageKind::ClientSetPaneCursorShape,
            Self::PaneInput { .. } => MessageKind::ClientPaneInput,
            Self::AckPresented { .. } => MessageKind::ClientAckPresented,
            Self::Ping { .. } => MessageKind::ClientPing,
        }
    }
}

/// Every message the server can send.
///
/// Frames are ordered per-pane (a client never sees update N+1 before
/// update N for the same pane). Cross-pane ordering is not guaranteed.
/// Direct responses to a client request carry the originating
/// [`RequestId`]; spontaneous server pushes use [`RequestId::PUSH`].
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    Error(ProtocolErrorPayload),
    Topology(Topology),
    PaneUpdate(PaneUpdate),
    PaneExited {
        pane: PaneRef,
        exit_status: Option<i32>,
    },
    PaneClipboardRequest {
        pane: PaneRef,
        request: ClipboardRequest,
    },
    ResyncRequired {
        pane: PaneRef,
        reason: String,
    },
    Pong {
        nonce: u64,
    },
}

impl ServerMessage {
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Error(_) => MessageKind::ServerError,
            Self::Topology(_) => MessageKind::ServerTopology,
            Self::PaneUpdate(_) => MessageKind::ServerPaneUpdate,
            Self::PaneExited { .. } => MessageKind::ServerPaneExited,
            Self::PaneClipboardRequest { .. } => MessageKind::ServerPaneClipboardRequest,
            Self::ResyncRequired { .. } => MessageKind::ServerResyncRequired,
            Self::Pong { .. } => MessageKind::ServerPong,
        }
    }
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

/// The atomic server unit. `image_events` apply before the `frame`
/// (when both are present) so that a frame referencing a freshly
/// uploaded image never lands before that image is in the client's
/// cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneUpdate {
    pub pane: PaneRef,
    pub seq: ServerSeq,
    pub image_events: Vec<ImageCacheEvent>,
    pub frame: Option<FrameDelta>,
}

/// Stable u16 tags for every [`ClientMessage`] / [`ServerMessage`]
/// variant. Client-direction tags are 1..=15; server-direction tags are
/// 1001..=1099. Wire-stable across releases — never renumber or reuse an
/// id (gaps are retired ids); add new variants with the next free id in
/// the appropriate range.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageKind {
    ClientSpawnPane = 3,
    ClientResizePane = 5,
    ClientPaneInput = 6,
    ClientAckPresented = 10,
    ClientPing = 11,
    ClientScrollPane = 13,
    ClientSetPaneTheme = 14,
    ClientSetPaneCursorShape = 15,
    ServerError = 1002,
    ServerTopology = 1003,
    ServerPaneUpdate = 1004,
    ServerPaneExited = 1005,
    ServerResyncRequired = 1006,
    ServerPong = 1007,
    ServerPaneClipboardRequest = 1009,
}

impl TryFrom<u16> for MessageKind {
    type Error = CodecError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        let kind = match value {
            3 => Self::ClientSpawnPane,
            5 => Self::ClientResizePane,
            6 => Self::ClientPaneInput,
            10 => Self::ClientAckPresented,
            11 => Self::ClientPing,
            13 => Self::ClientScrollPane,
            14 => Self::ClientSetPaneTheme,
            15 => Self::ClientSetPaneCursorShape,
            1002 => Self::ServerError,
            1003 => Self::ServerTopology,
            1004 => Self::ServerPaneUpdate,
            1005 => Self::ServerPaneExited,
            1006 => Self::ServerResyncRequired,
            1007 => Self::ServerPong,
            1009 => Self::ServerPaneClipboardRequest,
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
    use crate::identity::{DomainId, ImageId, ImageKey, PaneEpoch, PaneId};
    use crate::image_cache::{ImageFormat, ImagePayload};
    use crate::transport::{RequestId, decode_payload, encode_payload};

    fn pane() -> PaneRef {
        PaneRef {
            domain: DomainId(1),
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
        round_trip(&ServerMessage::Error(ProtocolErrorPayload {
            kind: ProtocolErrorKind::ServerPaneError,
            message: "pane spawn failed".into(),
            request_id: RequestId(7),
            pane: Some(pane),
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
        round_trip(&ImageCacheEvent::Evict { key });
    }
}
