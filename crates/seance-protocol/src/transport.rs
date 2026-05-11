//! Envelope framing, codec functions, and the [`Transport`] trait.

use std::fmt;
use std::sync::mpsc;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::identity::ServerSeq;
use crate::limits::MAX_DECODED_MESSAGE_BYTES;
use crate::mux::{ClientMessage, MessageKind, ServerMessage};

/// Correlation token attached to a client request. The server echoes
/// it on the matching [`crate::mux::ServerMessage`]. [`RequestId::PUSH`]
/// (zero) marks server-initiated messages with no client request.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct RequestId(#[allow(missing_docs)] pub u64);

/// Logical multiplexing channel within a single transport. Used by
/// [`client_stream`] / [`server_stream`] to keep input, output, image,
/// and control traffic separable.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct StreamId(#[allow(missing_docs)] pub u16);

impl StreamId {
    /// Handshake, topology, and acknowledgement traffic.
    pub const CONTROL: Self = Self(0);
    /// Client-to-server PTY input.
    pub const INPUT: Self = Self(1);
    /// Server-to-client frame and line traffic.
    pub const OUTPUT: Self = Self(2);
    /// Image cache events.
    pub const IMAGES: Self = Self(3);
}

impl RequestId {
    /// Sentinel for messages with no associated client request — i.e.
    /// server-initiated push.
    pub const PUSH: Self = Self(0);

    /// Whether this id is the [`PUSH`](Self::PUSH) sentinel.
    pub fn is_push(self) -> bool {
        self.0 == 0
    }
}

/// Bytes ready for transport, tagged with the [`StreamId`] they belong
/// on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportFrame {
    #[allow(missing_docs)]
    pub stream_id: StreamId,
    /// Length-prefixed envelope bytes.
    pub bytes: Vec<u8>,
}

/// Bidirectional, non-blocking transport over which
/// [`TransportFrame`]s are exchanged. Implementations are expected to
/// preserve frame boundaries and per-stream ordering.
pub trait Transport {
    /// Send `frame`. Returns [`TransportError::Closed`] when the peer
    /// has gone away.
    fn send(&self, frame: TransportFrame) -> Result<(), TransportError>;

    /// Pull the next frame without blocking. `Ok(None)` means no frame
    /// is currently available; [`TransportError::Closed`] means EOF.
    fn try_recv(&self) -> Result<Option<TransportFrame>, TransportError>;
}

/// Failure mode reported by [`Transport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The transport peer has disconnected.
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

/// In-process transport backed by a pair of [`mpsc`] channels. Useful
/// for the local-only client/server topology and for tests.
pub struct InProcessTransport {
    tx: mpsc::Sender<TransportFrame>,
    rx: mpsc::Receiver<TransportFrame>,
}

impl InProcessTransport {
    /// Build a connected pair: each end's `send` reaches the other's
    /// `try_recv`.
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

/// Metadata wrapper around a serialized message payload. The wire form
/// is `<varint-length-prefix><postcard-encoded Envelope>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    #[allow(missing_docs)]
    pub request_id: RequestId,
    #[allow(missing_docs)]
    pub server_seq: ServerSeq,
    /// Numeric tag of the inner payload's [`MessageKind`].
    pub kind: u16,
    /// Postcard-encoded message body.
    pub payload: Vec<u8>,
}

impl Envelope {
    /// Decode [`Self::kind`] into a [`MessageKind`], or report
    /// [`CodecError::UnknownMessage`] if the tag is not recognised.
    pub fn known_kind(&self) -> Result<MessageKind, CodecError> {
        MessageKind::try_from(self.kind)
    }
}

/// Direction a particular [`MessageKind`] is expected to flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDirection {
    /// Client-emitted message.
    Client,
    /// Server-emitted message.
    Server,
}

/// Reasons a wire-level encode or decode failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Envelope tag value did not correspond to a [`MessageKind`].
    UnknownMessage(#[allow(missing_docs)] u16),
    /// Frame length exceeded
    /// [`crate::limits::MAX_DECODED_MESSAGE_BYTES`] (or a custom cap
    /// passed to [`decode_envelope`]).
    OversizedFrame {
        #[allow(missing_docs)]
        len: usize,
        #[allow(missing_docs)]
        max: usize,
    },
    /// Buffer ended mid-frame.
    TruncatedFrame,
    /// Length prefix flagged a compressed frame; this build does not
    /// implement compression.
    BadCompressionFlag,
    /// Length-prefix varint overflowed `u64`.
    VarintOverflow,
    /// Postcard reported a deserialization error; payload is its
    /// message.
    CorruptPayload(#[allow(missing_docs)] String),
    /// A message arrived in the wrong direction (e.g. a server-only
    /// kind on a client-decode path).
    UnexpectedMessageKind {
        #[allow(missing_docs)]
        direction: MessageDirection,
        #[allow(missing_docs)]
        kind: MessageKind,
    },
    /// Envelope kind did not match the expected variant for the call.
    WrongMessageKind {
        #[allow(missing_docs)]
        expected: MessageKind,
        #[allow(missing_docs)]
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

/// Postcard-encode `payload` into a byte vector.
pub fn encode_payload<T: Serialize>(payload: &T) -> Result<Vec<u8>, CodecError> {
    postcard::to_stdvec(payload).map_err(|err| CodecError::CorruptPayload(err.to_string()))
}

/// Postcard-decode `bytes` into `T`.
pub fn decode_payload<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    postcard::from_bytes(bytes).map_err(|err| CodecError::CorruptPayload(err.to_string()))
}

/// Encode `payload` into a length-prefixed [`Envelope`] tagged with
/// `kind`.
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

/// Decode the next length-prefixed [`Envelope`] from `input`. Returns
/// the envelope and the number of bytes consumed; subsequent bytes are
/// the next frame. Frames larger than `max_len` fail with
/// [`CodecError::OversizedFrame`].
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

/// Decode `envelope.payload` into `T`, asserting the envelope tag
/// matches `expected`.
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

/// Wrap `message` in an envelope, encode it, and route it onto the
/// appropriate [`StreamId`]. `request_id` is the client's correlation
/// token.
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

/// Wrap `message` in an envelope, encode it, and route it onto the
/// appropriate [`StreamId`]. The server seq is read from `message`.
pub fn encode_server_frame(message: ServerMessage) -> Result<TransportFrame, CodecError> {
    let kind = message.kind();
    let seq = server_seq(&message);
    let bytes = encode_envelope(kind, RequestId::PUSH, seq, &message)?;
    Ok(TransportFrame {
        stream_id: server_stream(&message),
        bytes,
    })
}

/// Decode a [`TransportFrame`] received on a client-bound stream into
/// a [`ClientMessage`]. Rejects server-only [`MessageKind`]s.
pub fn decode_client_frame(frame: &TransportFrame) -> Result<ClientMessage, CodecError> {
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
    Ok(message)
}

/// Decode a [`TransportFrame`] received on a server-bound stream into
/// a [`ServerMessage`]. Rejects client-only [`MessageKind`]s.
pub fn decode_server_frame(frame: &TransportFrame) -> Result<ServerMessage, CodecError> {
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
    Ok(message)
}

/// Map a client message variant to the [`StreamId`] it should travel on.
pub fn client_stream(message: &ClientMessage) -> StreamId {
    match message {
        ClientMessage::PaneInput { .. } => StreamId::INPUT,
        ClientMessage::ImageCacheMiss { .. } => StreamId::IMAGES,
        _ => StreamId::CONTROL,
    }
}

/// Map a server message variant to the [`StreamId`] it should travel on.
pub fn server_stream(message: &ServerMessage) -> StreamId {
    match message {
        ServerMessage::PaneUpdate(update) if !update.image_events.is_empty() => StreamId::IMAGES,
        ServerMessage::PaneUpdate(_) | ServerMessage::Lines(_) => StreamId::OUTPUT,
        _ => StreamId::CONTROL,
    }
}

/// Sequence number a server message should be stamped with.
/// Non-sequenced messages return [`ServerSeq`] of zero.
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
        | MessageKind::ServerResyncRequired
        | MessageKind::ServerPong
        | MessageKind::ServerLines => Ok(()),
        _ => Err(CodecError::UnexpectedMessageKind {
            direction: MessageDirection::Server,
            kind,
        }),
    }
}

/// Prepend `payload` with a varint length prefix. The low bit of the
/// varint encodes the `compressed` flag; this build never sets it but
/// reads it for forward-compatibility (see
/// [`CodecError::BadCompressionFlag`]).
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

    use crate::identity::{PaneEpoch, PaneId, PaneRef, ServerSeq};
    use crate::limits::MAX_DECODED_MESSAGE_BYTES;
    use crate::mux::{ClientMessage, MessageKind, ServerMessage};

    fn pane() -> PaneRef {
        PaneRef {
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
