//! Byte-budget constants that gate per-connection backpressure.

/// Hard cap on a single decoded envelope. Frames larger than this fail
/// with [`crate::transport::CodecError::OversizedFrame`] before
/// deserialization.
pub const MAX_DECODED_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum payload size of a single [`crate::mux::ClientMessage::PaneInput`].
pub const MAX_PTY_INPUT_BYTES: usize = 1024 * 1024;
/// Per-client budget for queued PTY input bytes; exceeding it is a
/// backpressure / abuse signal that the server may act on.
pub const MAX_PENDING_INPUT_BYTES_PER_CLIENT: usize = 4 * 1024 * 1024;
/// Maximum size of a single [`crate::image_cache::ImagePutChunk::bytes`]
/// segment.
pub const MAX_IMAGE_CHUNK_BYTES: usize = 1024 * 1024;
/// Per-client budget for queued outbound bytes before the server drops
/// or resets the slowest pane subscription.
pub const MAX_PENDING_OUTBOUND_BYTES_PER_CLIENT: usize = 32 * 1024 * 1024;
/// How many recent [`crate::mux::PaneUpdate`] messages the server
/// retains per pane for resume; older updates require a snapshot resync.
pub const MAX_RETAINED_PANE_UPDATES: usize = 512;
