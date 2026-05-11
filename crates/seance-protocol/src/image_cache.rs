//! Server-driven image cache events sent over [`crate::transport`].

use serde::{Deserialize, Serialize};

use crate::identity::ImageKey;

/// Pixel encoding of an image payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    /// 8-bit-per-channel RGBA, tightly packed (`width * height * 4`).
    Rgba8,
}

/// Single-shot image upload payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePayload {
    #[allow(missing_docs)]
    pub key: ImageKey,
    #[allow(missing_docs)]
    pub width: u32,
    #[allow(missing_docs)]
    pub height: u32,
    /// Total byte length of `rgba` (redundant for validation).
    pub byte_len: u64,
    #[allow(missing_docs)]
    pub format: ImageFormat,
    /// Content hash (e.g. SHA-256) used to deduplicate uploads.
    pub digest: [u8; 32],
    /// Pixel bytes in `format` encoding.
    pub rgba: Vec<u8>,
}

/// Header for a chunked image upload — describes the full image; the
/// bytes arrive in following [`ImagePutChunk`] messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePutStart {
    #[allow(missing_docs)]
    pub key: ImageKey,
    #[allow(missing_docs)]
    pub width: u32,
    #[allow(missing_docs)]
    pub height: u32,
    /// Total byte length the chunks must sum to.
    pub byte_len: u64,
    #[allow(missing_docs)]
    pub format: ImageFormat,
    #[allow(missing_docs)]
    pub digest: [u8; 32],
    /// Server-imposed chunk size; capped by
    /// [`crate::limits::MAX_IMAGE_CHUNK_BYTES`].
    pub chunk_bytes: u32,
}

/// One chunk of a chunked image upload, addressed by `offset` into the
/// total byte stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePutChunk {
    #[allow(missing_docs)]
    pub key: ImageKey,
    /// Byte offset of `bytes` within the full image.
    pub offset: u64,
    #[allow(missing_docs)]
    pub bytes: Vec<u8>,
}

/// Server-driven mutation of the client's image cache.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageCacheEvent {
    /// Single-shot put of a complete image.
    Put(#[allow(missing_docs)] ImagePayload),
    /// Begin a chunked upload.
    PutStart(#[allow(missing_docs)] ImagePutStart),
    /// One chunk of an in-progress upload.
    PutChunk(#[allow(missing_docs)] ImagePutChunk),
    /// All chunks for `key` have been delivered; the image is now usable.
    PutComplete {
        #[allow(missing_docs)]
        key: ImageKey,
    },
    /// Server dropped this image from its cache; clients should too.
    Evict {
        #[allow(missing_docs)]
        key: ImageKey,
    },
}
