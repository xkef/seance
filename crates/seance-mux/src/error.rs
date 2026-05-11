use std::fmt;

use seance_protocol::transport::{CodecError, TransportError};

/// Failure surfaced from a [`crate::Domain::spawn_pane`] call. Wraps a
/// human-readable detail string so the protocol and local back-ends
/// can surface their own diagnostics through one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnError {
    message: String,
}

impl SpawnError {
    /// Build a spawn error with the given diagnostic message.
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

/// Failure surfaced from a per-pane operation on a [`crate::Domain`].
/// Wraps a human-readable detail string; Display reproduces it verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneError {
    message: String,
}

impl PaneError {
    /// Build a pane error with the given diagnostic message.
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
