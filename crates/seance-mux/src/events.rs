use seance_protocol::{ImageCacheEvent, PaneRef, PaneUpdate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    Wake,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    PaneUpdate(PaneUpdate),
    PaneExited {
        pane: PaneRef,
    },
    Error {
        pane: Option<PaneRef>,
        message: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientRefresh {
    pub frame_dirty: bool,
    pub image_events: Vec<ImageCacheEvent>,
    pub exited: Vec<PaneRef>,
    pub errors: Vec<String>,
}

impl ClientRefresh {
    pub fn is_empty(&self) -> bool {
        !self.frame_dirty
            && self.image_events.is_empty()
            && self.exited.is_empty()
            && self.errors.is_empty()
    }
}
