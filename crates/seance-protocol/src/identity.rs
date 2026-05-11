//! Newtype identifiers used across the protocol.
//!
//! The inner integer of each newtype is `pub` so callers can construct
//! values directly; the fields carry `#[allow(missing_docs)]` because
//! their meaning is fully captured by the wrapping type.

use serde::{Deserialize, Serialize};

/// Monotonic per-server sequence number stamped on outbound updates so
/// clients can detect gaps, ack progress, and resume after reconnect.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ServerSeq(#[allow(missing_docs)] pub u64);

/// Frame generation counter. Bumps once per terminal-state mutation
/// visible to the renderer; clients use it to order
/// [`crate::frame::FrameDelta`] application.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Generation(#[allow(missing_docs)] pub u64);

/// Server process identity. Different values across server restarts let
/// clients invalidate cached state from a prior server.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ServerId(#[allow(missing_docs)] pub u64);

/// Server-assigned session identity scoped to a [`ServerId`].
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct SessionId(#[allow(missing_docs)] pub u64);

/// Server-assigned identity of a connected client.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ClientId(#[allow(missing_docs)] pub u64);

/// Identity of a multiplexer "domain" — a namespace of windows/tabs/panes
/// (e.g. local PTYs vs. a remote SSH host).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct DomainId(#[allow(missing_docs)] pub u64);

/// Identity of a window within a domain.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct WindowId(#[allow(missing_docs)] pub u64);

/// Identity of a tab within a window.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TabId(#[allow(missing_docs)] pub u64);

/// Identity of a pane (PTY-backed terminal) within a tab. A given pane
/// id can be reused across pane lifetimes; pair with [`PaneEpoch`] in
/// [`PaneRef`] to disambiguate.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PaneId(#[allow(missing_docs)] pub u64);

/// Generation counter incremented on every pane respawn under a given
/// [`PaneId`]. Used to detect stale references after a respawn.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PaneEpoch(#[allow(missing_docs)] pub u64);

/// Server-assigned image identity scoped to a [`PaneRef`] (see
/// [`ImageKey`]).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ImageId(#[allow(missing_docs)] pub u64);

impl From<u32> for ImageId {
    fn from(value: u32) -> Self {
        Self(u64::from(value))
    }
}

impl From<u64> for ImageId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// Stable reference to a pane: identity + respawn epoch. Comparing the
/// epoch lets the server reject inputs aimed at a stale pane after
/// respawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneRef {
    #[allow(missing_docs)]
    pub pane_id: PaneId,
    #[allow(missing_docs)]
    pub epoch: PaneEpoch,
}

impl PaneRef {
    /// Sentinel pane reference for the local-only client (no remote
    /// server).
    pub const LOCAL: Self = Self {
        pane_id: PaneId(0),
        epoch: PaneEpoch(0),
    };
}

/// Pane-scoped image identity. The same [`ImageId`] in different panes
/// is a distinct image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImageKey {
    #[allow(missing_docs)]
    pub pane: PaneRef,
    #[allow(missing_docs)]
    pub image_id: ImageId,
}

impl ImageKey {
    /// Build an [`ImageKey`] scoped to [`PaneRef::LOCAL`].
    pub fn local(image_id: ImageId) -> Self {
        Self {
            pane: PaneRef::LOCAL,
            image_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_keys_scope_pane_local_ids() {
        let first = ImageKey {
            pane: PaneRef {
                pane_id: PaneId(1),
                epoch: PaneEpoch(1),
            },
            image_id: ImageId(7),
        };
        let second = ImageKey {
            pane: PaneRef {
                pane_id: PaneId(2),
                epoch: PaneEpoch(1),
            },
            image_id: ImageId(7),
        };
        assert_ne!(first, second);
        assert_eq!(ImageKey::local(ImageId(7)).pane, PaneRef::LOCAL);
    }
}
