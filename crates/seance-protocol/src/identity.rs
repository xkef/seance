use serde::{Deserialize, Serialize};

/// Monotonic sequence number assigned by the server to each pushed update.
/// Clients ack the last applied/presented value back so the server knows
/// what frames it can drop from its replay history.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ServerSeq(pub u64);

/// Per-pane render-frame counter. Increments whenever the VT publishes a
/// new snapshot. Distinct from [`ServerSeq`], which counts protocol
/// messages — a single message may carry one frame (one Generation bump)
/// or none (e.g. PaneExited).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Generation(pub u64);

/// Identifies a server process. Distinguishes which long-lived server a
/// client is currently connected to so reattach can refuse a mismatched
/// session.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ServerId(pub u64);

/// Identifies a single mux session within a server. Survives client
/// disconnect/reconnect; persists for the daemon's lifetime.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct SessionId(pub u64);

/// Identifies a connected client within a session. Multi-client work
/// (M12 Phase 5) uses this to arbitrate input focus and spectator mode.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ClientId(pub u64);

/// Namespaces panes by the [`Domain`] that owns them so two Domain impls
/// (e.g. `LocalDomain` and a future `SshDomain`) can each mint
/// `PaneId(1), PaneId(2), …` independently without collisions on the
/// wire.
///
/// [`Domain`]: ../../seance_mux/trait.Domain.html
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct DomainId(pub u64);

/// Identifies a window within a domain. Tabs and pane splits live inside
/// windows; M6 (multiplexing) will populate this hierarchy.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct WindowId(pub u64);

/// Identifies a tab within a window. Holds a single split tree.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TabId(pub u64);

/// Identifies a pane within its domain. See [`PaneRef`] for the full
/// global handle that includes the [`DomainId`] and an epoch.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PaneId(pub u64);

/// Per-pane epoch. Bumped whenever the same `PaneId` is reused (e.g.
/// after a respawn) so stale references can be detected as routing to a
/// "different pane" rather than silently delivered to a fresh one.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PaneEpoch(pub u64);

/// Identifies an image (Kitty graphics or iTerm2 protocol) within the
/// pane that defined it. See [`ImageKey`] for the global handle.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ImageId(pub u64);

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

/// Global, wire-stable handle to a pane.
///
/// Carries the minting [`DomainId`], the per-domain [`PaneId`], and the
/// reuse [`PaneEpoch`]. All three must match for a message to route to
/// the intended pane; a mismatched epoch is treated as routing to a
/// "different pane" so stale handles produced by reconnects don't
/// silently bind to a freshly-spawned pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneRef {
    pub domain: DomainId,
    pub pane_id: PaneId,
    pub epoch: PaneEpoch,
}

impl PaneRef {
    /// Default PaneRef used by tests and embedded contexts that have only
    /// one in-process domain. Real `LocalDomain` instances start counting
    /// at `PaneId(1)`, so this never collides with a real spawned pane.
    pub const LOCAL: Self = Self {
        domain: DomainId(1),
        pane_id: PaneId(0),
        epoch: PaneEpoch(0),
    };
}

/// Global handle to an image: the [`PaneRef`] that defined it plus the
/// [`ImageId`] within that pane. Image IDs are pane-scoped so two panes
/// can reuse the same id without ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImageKey {
    pub pane: PaneRef,
    pub image_id: ImageId,
}

impl ImageKey {
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
                domain: DomainId(1),
                pane_id: PaneId(1),
                epoch: PaneEpoch(1),
            },
            image_id: ImageId(7),
        };
        let second = ImageKey {
            pane: PaneRef {
                domain: DomainId(1),
                pane_id: PaneId(2),
                epoch: PaneEpoch(1),
            },
            image_id: ImageId(7),
        };
        assert_ne!(first, second);
        assert_eq!(ImageKey::local(ImageId(7)).pane, PaneRef::LOCAL);
    }
}
