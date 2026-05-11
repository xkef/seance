use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ServerSeq(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Generation(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ServerId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct SessionId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ClientId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct DomainId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct WindowId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TabId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PaneId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PaneEpoch(pub u64);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneRef {
    pub pane_id: PaneId,
    pub epoch: PaneEpoch,
}

impl PaneRef {
    pub const LOCAL: Self = Self {
        pane_id: PaneId(0),
        epoch: PaneEpoch(0),
    };
}

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
