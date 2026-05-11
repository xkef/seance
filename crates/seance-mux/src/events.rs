use seance_protocol::identity::PaneRef;
use seance_protocol::image_cache::ImageCacheEvent;
use seance_protocol::mux::PaneUpdate;

/// Events the multiplexer emits to whatever drives its event loop.
/// Used to wake the consumer when there is pending work in any domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    /// Wake the consumer; it should call
    /// [`crate::MuxClient::refresh_updates`].
    Wake,
}

/// Per-domain event surfaced by [`crate::Domain::drain_events`].
/// Consumed by the [`crate::MuxClient`] and turned into
/// [`ClientRefresh`] state.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    /// New pane state — image cache mutations and an optional frame.
    PaneUpdate(#[allow(missing_docs)] PaneUpdate),
    /// `pane`'s process exited; the client should drop it.
    PaneExited {
        #[allow(missing_docs)]
        pane: PaneRef,
    },
    /// Domain-side error surfaced asynchronously. `pane` is `Some` when
    /// the error pertains to a specific pane.
    Error {
        #[allow(missing_docs)]
        pane: Option<PaneRef>,
        #[allow(missing_docs)]
        message: String,
    },
}

/// Aggregate result of a [`crate::MuxClient::refresh_updates`] call —
/// what the caller needs to redraw, fetch, or surface to the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientRefresh {
    /// At least one pane received a frame delta; caller should redraw.
    pub frame_dirty: bool,
    /// Image cache mutations to apply before the next paint.
    pub image_events: Vec<ImageCacheEvent>,
    /// Panes whose process exited during this refresh.
    pub exited: Vec<PaneRef>,
    /// Asynchronous error messages surfaced by domains during this
    /// refresh; the caller chooses how to present them.
    pub errors: Vec<String>,
}

impl ClientRefresh {
    /// Whether the refresh produced any work for the caller.
    pub fn is_empty(&self) -> bool {
        !self.frame_dirty
            && self.image_events.is_empty()
            && self.exited.is_empty()
            && self.errors.is_empty()
    }
}
