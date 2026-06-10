use serde::{Deserialize, Serialize};

use crate::identity::ImageKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Rgba8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePayload {
    pub key: ImageKey,
    pub width: u32,
    pub height: u32,
    pub byte_len: u64,
    pub format: ImageFormat,
    pub digest: [u8; 32],
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageCacheEvent {
    Put(ImagePayload),
    Evict { key: ImageKey },
}
