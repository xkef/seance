use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use bytes::Bytes;
use seance_mux_client::{
    Domain, DomainEvent, MuxEvent, PaneError, PaneFrameHistory, PaneSpawnOptions, SpawnError,
};
use seance_protocol::frame::{
    CursorShape, FrameDelta, Resize, SnapshotImage, ThemeColors, VtSnapshot,
};
use seance_protocol::identity::{
    DomainId as ProtocolDomainId, ImageKey, PaneEpoch, PaneId, PaneRef, ServerSeq,
};
use seance_protocol::image_cache::{ImageCacheEvent, ImageFormat, ImagePayload};
use seance_protocol::limits::MAX_RETAINED_PANE_UPDATES;
use seance_protocol::mux::PaneUpdate;
use seance_vt::{VtEvent, VtSessionHandle, VtSessionOptions, spawn_vt_session};

type EventSink = Arc<Mutex<Box<dyn Fn(MuxEvent) + Send>>>;

/// In-process [`Domain`] impl that owns real PTYs through `seance-vt`.
///
/// Each spawned pane runs on a dedicated VT actor thread; LocalDomain
/// aggregates their events on a single mpsc and republishes them as
/// [`DomainEvent`]s when [`Domain::drain_events`] is called. Pane IDs are
/// minted from a per-domain counter, namespaced by the `DomainId` this
/// instance was constructed with so multiple `LocalDomain`s coexist without
/// collisions.
pub struct LocalDomain {
    domain: ProtocolDomainId,
    next_pane_id: u64,
    panes: HashMap<PaneRef, LocalPane>,
    pending_tx: mpsc::Sender<LocalDomainEvent>,
    pending_rx: mpsc::Receiver<LocalDomainEvent>,
    event_sink: EventSink,
}

impl LocalDomain {
    /// Construct a LocalDomain with the default `DomainId(1)`. `event_sink`
    /// is called whenever a VT actor produces an event that the host event
    /// loop should react to (typically a wake on the winit proxy).
    pub fn new<F>(event_sink: F) -> Self
    where
        F: Fn(MuxEvent) + Send + 'static,
    {
        Self::with_domain(ProtocolDomainId(1), event_sink)
    }

    /// Construct a LocalDomain with an explicit `DomainId`. Use when more
    /// than one Domain instance lives in the same process and you need pane
    /// IDs to remain unambiguous when serialized on the wire.
    pub fn with_domain<F>(domain: ProtocolDomainId, event_sink: F) -> Self
    where
        F: Fn(MuxEvent) + Send + 'static,
    {
        let (pending_tx, pending_rx) = mpsc::channel();
        Self {
            domain,
            next_pane_id: 1,
            panes: HashMap::new(),
            pending_tx,
            pending_rx,
            event_sink: Arc::new(Mutex::new(Box::new(event_sink))),
        }
    }

    /// The replay window for `pane`, bounded by
    /// [`seance_protocol::limits::MAX_RETAINED_PANE_UPDATES`]. Used by the
    /// serve loop to answer client resync requests.
    pub fn history(&self, pane: PaneRef) -> Option<&PaneFrameHistory> {
        self.panes.get(&pane).map(|pane| &pane.history)
    }

    /// The `DomainId` this LocalDomain mints PaneRefs under.
    pub fn domain_id(&self) -> ProtocolDomainId {
        self.domain
    }

    fn pane_mut(&mut self, pane: PaneRef) -> Result<&mut LocalPane, PaneError> {
        self.panes
            .get_mut(&pane)
            .ok_or_else(|| PaneError::new("message routed to a different pane"))
    }
}

impl Domain for LocalDomain {
    fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError> {
        let pane_ref = PaneRef {
            domain: self.domain,
            pane_id: PaneId(self.next_pane_id),
            epoch: PaneEpoch(1),
        };
        self.next_pane_id += 1;

        let pending_tx = self.pending_tx.clone();
        let event_sink = Arc::clone(&self.event_sink);
        let vt = spawn_vt_session(vt_options_from(&options), move |event| {
            let local_event = match event {
                VtEvent::ContentDirty => LocalDomainEvent::ContentDirty { pane: pane_ref },
                VtEvent::ClipboardActivity => {
                    LocalDomainEvent::ClipboardActivity { pane: pane_ref }
                }
                VtEvent::Exited => LocalDomainEvent::Exited { pane: pane_ref },
            };
            let _ = pending_tx.send(local_event);
            emit_mux_event(&event_sink, MuxEvent::Wake);
        })
        .map_err(|err| SpawnError::new(err.to_string()))?;

        self.panes.insert(pane_ref, LocalPane::new(pane_ref, vt));
        Ok(pane_ref)
    }

    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError> {
        while let Ok(event) = self.pending_rx.try_recv() {
            match event {
                LocalDomainEvent::ContentDirty { .. } => {}
                LocalDomainEvent::ClipboardActivity { pane } => {
                    if let Some(local) = self.panes.get(&pane) {
                        for request in local.vt.drain_clipboard_requests() {
                            sink(DomainEvent::ClipboardRequest { pane, request });
                        }
                    }
                }
                LocalDomainEvent::Exited { pane } => {
                    if let Some(local) = self.panes.get_mut(&pane) {
                        local.exited = true;
                    }
                    sink(DomainEvent::PaneExited { pane });
                }
            }
        }

        let pane_refs = self.panes.keys().copied().collect::<Vec<_>>();
        for pane_ref in pane_refs {
            let Some(local) = self.panes.get_mut(&pane_ref) else {
                continue;
            };
            if local.exited {
                continue;
            }
            local.vt.clear_content_dirty_pending();
            let Some(snapshot) = local.vt.latest_snapshot() else {
                continue;
            };
            if local
                .server_snapshot
                .as_ref()
                .is_some_and(|latest| latest.generation == snapshot.generation)
            {
                continue;
            }

            let image_events = snapshot_image_events(pane_ref, &snapshot.images);
            let frame_snapshot = Arc::new(snapshot_without_image_payloads(&snapshot));
            let delta =
                FrameDelta::from_snapshot(local.server_snapshot.as_deref(), &frame_snapshot);
            let update = PaneUpdate {
                pane: pane_ref,
                seq: local.alloc_seq(),
                image_events,
                frame: Some(delta),
            };
            local.history.push(update.clone());
            local.server_snapshot = Some(frame_snapshot);
            sink(DomainEvent::PaneUpdate(update));
        }
        Ok(())
    }

    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .write(bytes)
            .map_err(vt_err_to_pane_err)
    }

    fn resize(&mut self, pane: PaneRef, resize: Resize) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .resize(resize)
            .map_err(vt_err_to_pane_err)
    }

    fn scroll_lines(&mut self, pane: PaneRef, delta: i32) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .scroll_lines(delta)
            .map_err(vt_err_to_pane_err)
    }

    fn set_theme_colors(&mut self, pane: PaneRef, colors: ThemeColors) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .set_theme_colors(colors)
            .map_err(vt_err_to_pane_err)
    }

    fn set_cursor_shape(&mut self, pane: PaneRef, shape: CursorShape) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .set_cursor_shape(shape)
            .map_err(vt_err_to_pane_err)
    }

    fn ack_presented(&mut self, pane: PaneRef, generation: u64) -> Result<(), PaneError> {
        self.pane_mut(pane)?
            .vt
            .ack_rendered(generation)
            .map_err(vt_err_to_pane_err)
    }
}

fn emit_mux_event(sink: &EventSink, event: MuxEvent) {
    if let Ok(sink) = sink.lock() {
        sink(event);
    }
}

fn vt_options_from(options: &PaneSpawnOptions) -> VtSessionOptions {
    VtSessionOptions {
        cols: options.cols,
        rows: options.rows,
        pixel_width: options.pixel_width,
        pixel_height: options.pixel_height,
        initial_cursor_shape: options.initial_cursor_shape,
        max_scrollback: options.max_scrollback,
    }
}

fn vt_err_to_pane_err(err: seance_vt::VtSessionError) -> PaneError {
    PaneError::new(err.to_string())
}

pub(crate) fn snapshot_without_image_payloads(snapshot: &VtSnapshot) -> VtSnapshot {
    let mut snapshot = snapshot.clone();
    snapshot.images.clear();
    snapshot
}

pub(crate) fn snapshot_image_events(
    pane: PaneRef,
    images: &[SnapshotImage],
) -> Vec<ImageCacheEvent> {
    images
        .iter()
        .map(|image| {
            ImageCacheEvent::Put(ImagePayload {
                key: ImageKey {
                    pane,
                    image_id: image.image_id,
                },
                width: image.width,
                height: image.height,
                byte_len: image.rgba.len() as u64,
                format: ImageFormat::Rgba8,
                digest: image_digest(image.width, image.height, &image.rgba),
                rgba: image.rgba.clone(),
            })
        })
        .collect()
}

fn image_digest(width: u32, height: u32, rgba: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let width = width.to_le_bytes();
    let height = height.to_le_bytes();
    for lane in 0..4u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ lane;
        for byte in width.iter().chain(height.iter()).chain(rgba.iter()) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        out[(lane as usize * 8)..][..8].copy_from_slice(&hash.to_le_bytes());
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalDomainEvent {
    ContentDirty { pane: PaneRef },
    ClipboardActivity { pane: PaneRef },
    Exited { pane: PaneRef },
}

struct LocalPane {
    vt: VtSessionHandle,
    server_snapshot: Option<Arc<VtSnapshot>>,
    history: PaneFrameHistory,
    next_seq: u64,
    exited: bool,
}

impl LocalPane {
    fn new(pane_ref: PaneRef, vt: VtSessionHandle) -> Self {
        Self {
            vt,
            server_snapshot: None,
            history: PaneFrameHistory::new(pane_ref, MAX_RETAINED_PANE_UPDATES),
            next_seq: 1,
            exited: false,
        }
    }

    fn alloc_seq(&mut self) -> ServerSeq {
        let seq = ServerSeq(self.next_seq);
        self.next_seq += 1;
        seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seance_protocol::frame::{CellAttrs, CellColor};
    use seance_protocol::identity::ImageId;

    fn pane_ref() -> PaneRef {
        PaneRef {
            domain: ProtocolDomainId(1),
            pane_id: PaneId(1),
            epoch: PaneEpoch(1),
        }
    }

    fn snapshot(generation: u64, text: &str) -> VtSnapshot {
        let mut snapshot = VtSnapshot::empty(1, 1);
        snapshot.generation = generation;
        snapshot.push_cell(
            text,
            CellColor::Default,
            CellColor::Default,
            CellAttrs::default(),
        );
        snapshot
    }

    #[test]
    fn snapshot_image_events_scope_payloads_to_pane() {
        let pane = pane_ref();
        let image = SnapshotImage {
            image_id: ImageId(9),
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        };

        let events = snapshot_image_events(pane, std::slice::from_ref(&image));

        assert_eq!(events.len(), 1);
        let ImageCacheEvent::Put(payload) = &events[0] else {
            panic!("expected image put event");
        };
        assert_eq!(
            payload.key,
            ImageKey {
                pane,
                image_id: image.image_id,
            }
        );
        assert_eq!(payload.width, image.width);
        assert_eq!(payload.height, image.height);
        assert_eq!(payload.byte_len, 4);
        assert_eq!(payload.format, ImageFormat::Rgba8);
        assert_ne!(payload.digest, [0; 32]);
        assert_eq!(payload.rgba, image.rgba);
    }

    #[test]
    fn snapshot_without_image_payloads_preserves_frame_state() {
        let mut snapshot = snapshot(3, "x");
        snapshot.images.push(SnapshotImage {
            image_id: ImageId(1),
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0],
        });

        let stripped = snapshot_without_image_payloads(&snapshot);

        assert!(stripped.images.is_empty());
        assert_eq!(stripped.generation, snapshot.generation);
        assert_eq!(stripped.cells, snapshot.cells);
        assert_eq!(stripped.text, snapshot.text);
    }

    #[test]
    fn pane_spawn_options_propagate_max_scrollback_to_vt() {
        let options = PaneSpawnOptions {
            max_scrollback: 12_345,
            ..PaneSpawnOptions::default()
        };
        let vt = vt_options_from(&options);
        assert_eq!(vt.max_scrollback, 12_345);
    }
}
