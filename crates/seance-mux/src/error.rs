use std::fmt;

use seance_protocol::transport::{CodecError, TransportError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnError {
    message: String,
}

impl SpawnError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SpawnError {}

impl From<seance_vt::SpawnError> for SpawnError {
    fn from(value: seance_vt::SpawnError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<PaneError> for SpawnError {
    fn from(value: PaneError) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneError {
    message: String,
}

impl PaneError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PaneError {}

impl From<seance_vt::VtSessionError> for PaneError {
    fn from(value: seance_vt::VtSessionError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<TransportError> for PaneError {
    fn from(value: TransportError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<CodecError> for PaneError {
    fn from(value: CodecError) -> Self {
        Self::new(value.to_string())
    }
}
