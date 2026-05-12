use seance_protocol::identity::PaneRef;
use seance_protocol::image_cache::ImageCacheEvent;
use seance_protocol::mux::PaneUpdate;
use seance_vt::ClipboardRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    Wake,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    PaneUpdate(PaneUpdate),
    PaneExited {
        pane: PaneRef,
    },
    /// An OSC 52 clipboard request originating from `pane` was parsed by the
    /// VT layer. The mux client forwards these unchanged through
    /// [`ClientRefresh::clipboard_requests`]; arbitration (the
    /// `clipboard.{read,write}` policy, OS clipboard access) lives in the
    /// application layer.
    ClipboardRequest {
        pane: PaneRef,
        request: ClipboardRequest,
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
    pub clipboard_requests: Vec<(PaneRef, ClipboardRequest)>,
}

impl ClientRefresh {
    pub fn is_empty(&self) -> bool {
        !self.frame_dirty
            && self.image_events.is_empty()
            && self.exited.is_empty()
            && self.errors.is_empty()
            && self.clipboard_requests.is_empty()
    }
}
