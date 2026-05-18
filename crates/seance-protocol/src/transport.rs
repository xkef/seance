use std::fmt;
use std::sync::mpsc;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::identity::ServerSeq;
use crate::limits::MAX_DECODED_MESSAGE_BYTES;
use crate::mux::{ClientMessage, MessageKind, ServerMessage};

/// Correlates a server response to the client request that triggered it.
/// [`RequestId::PUSH`] (= 0) marks unilateral server pushes.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct RequestId(pub u64);

/// Partitions the wire into independent ordering domains so a slow stream
/// (e.g. a large image upload) cannot head-of-line-block keyboard input.
/// Transport impls are expected to preserve per-stream ordering but may
/// interleave streams freely.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct StreamId(pub u16);

impl StreamId {
    /// Lifecycle / topology / acks. Default for messages that don't fit
    /// the dedicated streams below.
    pub const CONTROL: Self = Self(0);
    /// Keyboard / paste / mouse bytes from client to server.
    pub const INPUT: Self = Self(1);
    /// Frame deltas and `Lines` data from server to client.
    pub const OUTPUT: Self = Self(2);
    /// Image cache events (Put / Chunk / Evict).
    pub const IMAGES: Self = Self(3);
}

impl RequestId {
    /// Sentinel meaning "server-initiated push, not a response".
    pub const PUSH: Self = Self(0);

    pub fn is_push(self) -> bool {
        self.0 == 0
    }
}

/// A single encoded frame ready for the wire. `bytes` already includes
/// the length-prefixed envelope; the receiver decodes it via
/// [`decode_envelope`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportFrame {
    pub stream_id: StreamId,
    pub bytes: Vec<u8>,
}

/// Frame-oriented transport contract.
///
/// Implementors preserve per-stream ordering. `try_recv` is non-blocking
/// — callers poll it (often paired with a wake mechanism). The trait is
/// intentionally not `Send`-bounded so single-threaded server bootstraps
/// can use cheap interior-mutability impls; transports crossing thread
/// or process boundaries add their own `Send`/`Sync` requirements as
/// needed.
pub trait Transport {
    /// Enqueue `frame` for delivery. Returns `Err(Closed)` if the peer
    /// has hung up.
    fn send(&self, frame: TransportFrame) -> Result<(), TransportError>;

    /// Non-blocking receive. Returns `Ok(None)` when no frame is
    /// currently available, `Err(Closed)` when the peer has hung up.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub request_id: RequestId,
    pub server_seq: ServerSeq,
    pub kind: u16,
    pub payload: Vec<u8>,
}

impl Envelope {
    pub fn known_kind(&self) -> Result<MessageKind, CodecError> {
        MessageKind::try_from(self.kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDirection {
    Client,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    UnknownMessage(u16),
    OversizedFrame {
        len: usize,
        max: usize,
    },
    TruncatedFrame,
    BadCompressionFlag,
    VarintOverflow,
    CorruptPayload(String),
    UnexpectedMessageKind {
        direction: MessageDirection,
        kind: MessageKind,
    },
    WrongMessageKind {
        expected: MessageKind,
        actual: u16,
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMessage(kind) => write!(f, "unknown message kind {kind}"),
            Self::OversizedFrame { len, max } => write!(f, "frame is {len} bytes, max is {max}"),
            Self::TruncatedFrame => f.write_str("truncated frame"),
            Self::BadCompressionFlag => f.write_str("compression flag is set but unsupported"),
            Self::VarintOverflow => f.write_str("frame length varint overflow"),
            Self::CorruptPayload(err) => write!(f, "corrupt payload: {err}"),
            Self::UnexpectedMessageKind { direction, kind } => {
                write!(f, "expected {direction:?} message kind, got {kind:?}")
            }
            Self::WrongMessageKind { expected, actual } => {
                write!(f, "wrong message kind: expected {expected:?}, got {actual}")
            }
        }
    }
}

impl std::error::Error for CodecError {}

pub fn encode_payload<T: Serialize>(payload: &T) -> Result<Vec<u8>, CodecError> {
    postcard::to_stdvec(payload).map_err(|err| CodecError::CorruptPayload(err.to_string()))
}

pub fn decode_payload<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    postcard::from_bytes(bytes).map_err(|err| CodecError::CorruptPayload(err.to_string()))
}

pub fn encode_envelope<T: Serialize>(
    kind: MessageKind,
    request_id: RequestId,
    server_seq: ServerSeq,
    payload: &T,
) -> Result<Vec<u8>, CodecError> {
    let envelope = Envelope {
        request_id,
        server_seq,
        kind: kind.into(),
        payload: encode_payload(payload)?,
    };
    let bytes = encode_payload(&envelope)?;
    Ok(encode_length_prefixed(&bytes, false))
}

pub fn decode_envelope(input: &[u8], max_len: usize) -> Result<(Envelope, usize), CodecError> {
    let (len, compressed, prefix_len) = decode_prefix(input)?;
    if compressed {
        return Err(CodecError::BadCompressionFlag);
    }
    if len > max_len {
        return Err(CodecError::OversizedFrame { len, max: max_len });
    }
    let end = prefix_len
        .checked_add(len)
        .ok_or(CodecError::VarintOverflow)?;
    if input.len() < end {
        return Err(CodecError::TruncatedFrame);
    }
    let envelope = decode_payload(&input[prefix_len..end])?;
    Ok((envelope, end))
}

pub fn decode_typed_payload<T: DeserializeOwned>(
    envelope: &Envelope,
    expected: MessageKind,
) -> Result<T, CodecError> {
    if envelope.kind != u16::from(expected) {
        return Err(CodecError::WrongMessageKind {
            expected,
            actual: envelope.kind,
        });
    }
    decode_payload(&envelope.payload)
}

pub fn encode_client_frame(
    message: ClientMessage,
    request_id: RequestId,
) -> Result<TransportFrame, CodecError> {
    let kind = message.kind();
    let bytes = encode_envelope(kind, request_id, ServerSeq(0), &message)?;
    Ok(TransportFrame {
        stream_id: client_stream(&message),
        bytes,
    })
}

pub fn encode_server_frame(message: ServerMessage) -> Result<TransportFrame, CodecError> {
    encode_server_frame_with_request(message, RequestId::PUSH)
}

/// Encode a server message that carries a `request_id` correlating it to a
/// prior client request (e.g. the [`ServerMessage::Topology`] reply to a
/// [`ClientMessage::SpawnPane`]). Use [`RequestId::PUSH`] for unilateral
/// server-initiated messages (the default path through
/// [`encode_server_frame`]).
pub fn encode_server_frame_with_request(
    message: ServerMessage,
    request_id: RequestId,
) -> Result<TransportFrame, CodecError> {
    let kind = message.kind();
    let seq = server_seq(&message);
    let bytes = encode_envelope(kind, request_id, seq, &message)?;
    Ok(TransportFrame {
        stream_id: server_stream(&message),
        bytes,
    })
}

pub fn decode_client_frame(frame: &TransportFrame) -> Result<ClientMessage, CodecError> {
    let (_request_id, message) = decode_client_frame_with_request(frame)?;
    Ok(message)
}

/// Decode a client frame and return both the originating [`RequestId`] and
/// the [`ClientMessage`]. The server uses the request id to correlate
/// responses (e.g. tagging a [`ServerMessage::Topology`] as the reply to a
/// [`ClientMessage::SpawnPane`]).
pub fn decode_client_frame_with_request(
    frame: &TransportFrame,
) -> Result<(RequestId, ClientMessage), CodecError> {
    let (envelope, _consumed) = decode_envelope(&frame.bytes, MAX_DECODED_MESSAGE_BYTES)?;
    let kind = envelope.known_kind()?;
    ensure_client_kind(kind)?;
    let message: ClientMessage = decode_typed_payload(&envelope, kind)?;
    if message.kind() != kind {
        return Err(CodecError::WrongMessageKind {
            expected: message.kind(),
            actual: kind.into(),
        });
    }
    Ok((envelope.request_id, message))
}

pub fn decode_server_frame(frame: &TransportFrame) -> Result<ServerMessage, CodecError> {
    let (_request_id, message) = decode_server_frame_with_request(frame)?;
    Ok(message)
}

/// Decode a server frame and return both the correlating [`RequestId`] and
/// the [`ServerMessage`]. The client uses the request id to match a server
/// reply to the in-flight client request that triggered it; a value of
/// [`RequestId::PUSH`] means the message is unsolicited.
pub fn decode_server_frame_with_request(
    frame: &TransportFrame,
) -> Result<(RequestId, ServerMessage), CodecError> {
    let (envelope, _consumed) = decode_envelope(&frame.bytes, MAX_DECODED_MESSAGE_BYTES)?;
    let kind = envelope.known_kind()?;
    ensure_server_kind(kind)?;
    let message: ServerMessage = decode_typed_payload(&envelope, kind)?;
    if message.kind() != kind {
        return Err(CodecError::WrongMessageKind {
            expected: message.kind(),
            actual: kind.into(),
        });
    }
    Ok((envelope.request_id, message))
}

pub fn client_stream(message: &ClientMessage) -> StreamId {
    match message {
        ClientMessage::PaneInput { .. } => StreamId::INPUT,
        ClientMessage::ImageCacheMiss { .. } => StreamId::IMAGES,
        _ => StreamId::CONTROL,
    }
}

pub fn server_stream(message: &ServerMessage) -> StreamId {
    match message {
        ServerMessage::PaneUpdate(update) if !update.image_events.is_empty() => StreamId::IMAGES,
        ServerMessage::PaneUpdate(_) | ServerMessage::Lines(_) => StreamId::OUTPUT,
        _ => StreamId::CONTROL,
    }
}

/// Bound on how often the in-process server bootstrap parks itself when both
/// directions are quiet. Picked to match the deadline scheduler's coarse
/// wake granularity without paying noticeable in-process latency.
pub const SERVE_IDLE_BACKOFF_MS: u64 = 1;

pub fn server_seq(message: &ServerMessage) -> ServerSeq {
    match message {
        ServerMessage::PaneUpdate(update) => update.seq,
        ServerMessage::Lines(lines) => lines.seq,
        _ => ServerSeq(0),
    }
}

fn ensure_client_kind(kind: MessageKind) -> Result<(), CodecError> {
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
        _ => Err(CodecError::UnexpectedMessageKind {
            direction: MessageDirection::Client,
            kind,
        }),
    }
}

fn ensure_server_kind(kind: MessageKind) -> Result<(), CodecError> {
    match kind {
        MessageKind::ServerHello
        | MessageKind::ServerError
        | MessageKind::ServerTopology
        | MessageKind::ServerPaneUpdate
        | MessageKind::ServerPaneExited
        | MessageKind::ServerPaneClipboardRequest
        | MessageKind::ServerResyncRequired
        | MessageKind::ServerPong
        | MessageKind::ServerLines => Ok(()),
        _ => Err(CodecError::UnexpectedMessageKind {
            direction: MessageDirection::Server,
            kind,
        }),
    }
}

pub fn encode_length_prefixed(payload: &[u8], compressed: bool) -> Vec<u8> {
    let value = ((payload.len() as u64) << 1) | u64::from(compressed);
    let mut out = Vec::with_capacity(varint_len(value) + payload.len());
    encode_varint(value, &mut out);
    out.extend_from_slice(payload);
    out
}

fn decode_prefix(input: &[u8]) -> Result<(usize, bool, usize), CodecError> {
    let (value, used) = decode_varint(input)?;
    let compressed = (value & 1) != 0;
    let len = usize::try_from(value >> 1).map_err(|_| CodecError::VarintOverflow)?;
    Ok((len, compressed, used))
}

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn decode_varint(input: &[u8]) -> Result<(u64, usize), CodecError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (idx, byte) in input.iter().copied().enumerate() {
        if idx == 10 {
            return Err(CodecError::VarintOverflow);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, idx + 1));
        }
        shift += 7;
    }
    Err(CodecError::TruncatedFrame)
}

fn varint_len(mut value: u64) -> usize {
    let mut len = 1;
    while value >= 0x80 {
        len += 1;
        value >>= 7;
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    use crate::identity::{DomainId, PaneEpoch, PaneId, PaneRef, ServerSeq};
    use crate::limits::MAX_DECODED_MESSAGE_BYTES;
    use crate::mux::{ClientMessage, MessageKind, ServerMessage};

    fn pane() -> PaneRef {
        PaneRef {
            domain: DomainId(1),
            pane_id: PaneId(9),
            epoch: PaneEpoch(1),
        }
    }

    #[test]
    fn protocol_transport_round_trips_typed_frames() {
        let (client, server) = InProcessTransport::pair();
        let pane = pane();
        let message = ClientMessage::PaneInput {
            pane,
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
    fn protocol_decode_rejects_wrong_direction() {
        let frame = encode_server_frame(ServerMessage::Pong { nonce: 9 }).unwrap();
        assert_eq!(
            decode_client_frame(&frame).unwrap_err(),
            CodecError::UnexpectedMessageKind {
                direction: MessageDirection::Client,
                kind: MessageKind::ServerPong,
            }
        );
    }

    #[test]
    fn envelope_codec_round_trips_and_has_stable_golden_bytes() {
        let encoded = encode_envelope(
            MessageKind::ClientPing,
            RequestId(7),
            ServerSeq(0),
            &ClientMessage::Ping { nonce: 99 },
        )
        .unwrap();
        assert_eq!(encoded, vec![12, 7, 0, 11, 2, 10, 99]);

        let (envelope, consumed) = decode_envelope(&encoded, MAX_DECODED_MESSAGE_BYTES).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(envelope.known_kind().unwrap(), MessageKind::ClientPing);
        let msg: ClientMessage = decode_typed_payload(&envelope, MessageKind::ClientPing).unwrap();
        assert_eq!(msg, ClientMessage::Ping { nonce: 99 });
    }

    #[test]
    fn envelope_codec_fails_cleanly() {
        let unknown = Envelope {
            request_id: RequestId(1),
            server_seq: ServerSeq(0),
            kind: 65000,
            payload: Vec::new(),
        };
        assert_eq!(
            unknown.known_kind().unwrap_err(),
            CodecError::UnknownMessage(65000)
        );

        let oversized = encode_length_prefixed(&[0; 8], false);
        assert_eq!(
            decode_envelope(&oversized, 7).unwrap_err(),
            CodecError::OversizedFrame { len: 8, max: 7 }
        );

        let truncated = encode_length_prefixed(&[1, 2, 3], false);
        assert_eq!(
            decode_envelope(&truncated[..truncated.len() - 1], MAX_DECODED_MESSAGE_BYTES)
                .unwrap_err(),
            CodecError::TruncatedFrame
        );

        let compressed = encode_length_prefixed(&[], true);
        assert_eq!(
            decode_envelope(&compressed, MAX_DECODED_MESSAGE_BYTES).unwrap_err(),
            CodecError::BadCompressionFlag
        );

        let corrupt = encode_length_prefixed(&[0xff], false);
        assert!(matches!(
            decode_envelope(&corrupt, MAX_DECODED_MESSAGE_BYTES).unwrap_err(),
            CodecError::CorruptPayload(_)
        ));
    }
}
