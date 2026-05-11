use std::collections::VecDeque;

use bytes::Bytes;
use seance_frame::FrameSource;
use seance_protocol::frame::{
    CellAttrs, CellColor, CursorInfo, DirtySnapshot, FrameDelta, RowDelta, TerminalModes,
    VtSnapshot,
};
use seance_protocol::identity::{ImageId, ImageKey, PaneEpoch, PaneId, PaneRef, ServerSeq};
use seance_protocol::image_cache::{ImageCacheEvent, ImageFormat, ImagePayload};
use seance_protocol::mux::{ClientMessage, MessageKind, PaneUpdate, ServerMessage};
use seance_protocol::transport::{
    InProcessTransport, StreamId, Transport, decode_client_frame, encode_server_frame,
};

use crate::{
    CursorShape, Domain, DomainEvent, MuxClient, PaneError, PaneFrameHistory, PaneSpawnOptions,
    PaneView, ProtocolDomain, ReplayBatch, Resize, SpawnError, ThemeColors,
};

fn pane_ref() -> PaneRef {
    PaneRef {
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
fn pane_view_applies_full_and_partial_updates() {
    let pane = pane_ref();
    let mut view = PaneView::new(pane);
    let first = PaneUpdate {
        pane,
        seq: ServerSeq(1),
        image_events: Vec::new(),
        frame: Some(FrameDelta::Full {
            generation: 1,
            snapshot: snapshot(1, "a"),
        }),
    };
    view.apply_update(&first).unwrap();
    assert_eq!(view.generation(), Some(1));

    let mut next = snapshot(2, "b");
    next.dirty = DirtySnapshot::Partial(vec![0]);
    let partial = PaneUpdate {
        pane,
        seq: ServerSeq(2),
        image_events: Vec::new(),
        frame: Some(FrameDelta::from_snapshot(
            view.latest_snapshot_for_tests(),
            &next,
        )),
    };
    view.apply_update(&partial).unwrap();

    let snapshot = view.latest_snapshot_for_tests().unwrap();
    assert_eq!(snapshot.cell_text(&snapshot.cells[0]), "b");
    assert_eq!(view.last_applied_seq(), Some(ServerSeq(2)));
}

#[test]
fn pane_view_frame_source_carries_pane_ref() {
    let pane = pane_ref();
    let mut view = PaneView::new(pane);
    let update = PaneUpdate {
        pane,
        seq: ServerSeq(1),
        image_events: Vec::new(),
        frame: Some(FrameDelta::Full {
            generation: 1,
            snapshot: snapshot(1, "x"),
        }),
    };
    view.apply_update(&update).unwrap();

    let mut source = view.frame_source().unwrap();

    assert_eq!(source.pane_ref(), pane);
}

#[derive(Default)]
struct ScriptedDomain {
    events: VecDeque<DomainEvent>,
    writes: Vec<(PaneRef, Bytes)>,
}

impl Domain for ScriptedDomain {
    fn spawn_pane(&mut self, _options: PaneSpawnOptions) -> Result<PaneRef, SpawnError> {
        Ok(pane_ref())
    }

    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError> {
        while let Some(event) = self.events.pop_front() {
            sink(event);
        }
        Ok(())
    }

    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError> {
        self.writes.push((pane, bytes));
        Ok(())
    }

    fn resize(&mut self, _pane: PaneRef, _resize: Resize) -> Result<(), PaneError> {
        Ok(())
    }

    fn scroll_lines(&mut self, _pane: PaneRef, _delta: i32) -> Result<(), PaneError> {
        Ok(())
    }

    fn set_theme_colors(&mut self, _pane: PaneRef, _colors: ThemeColors) -> Result<(), PaneError> {
        Ok(())
    }

    fn set_cursor_shape(&mut self, _pane: PaneRef, _shape: CursorShape) -> Result<(), PaneError> {
        Ok(())
    }

    fn ack_presented(&mut self, _pane: PaneRef, _generation: u64) -> Result<(), PaneError> {
        Ok(())
    }
}

#[test]
fn mux_client_drains_domain_updates_into_pane_view() {
    let pane = pane_ref();
    let update = PaneUpdate {
        pane,
        seq: ServerSeq(1),
        image_events: Vec::new(),
        frame: Some(FrameDelta::Full {
            generation: 1,
            snapshot: snapshot(1, "x"),
        }),
    };
    let mut client = MuxClient::new(ScriptedDomain {
        events: VecDeque::from([DomainEvent::PaneUpdate(update)]),
        ..ScriptedDomain::default()
    });

    let pane = client.spawn_pane(PaneSpawnOptions::default()).unwrap();

    assert_eq!(client.pane_view(pane).unwrap().generation(), Some(1));
}

#[test]
fn mux_client_refresh_collects_image_events() {
    let pane = pane_ref();
    let key = ImageKey {
        pane,
        image_id: ImageId(3),
    };
    let event = ImageCacheEvent::Put(ImagePayload {
        key,
        width: 1,
        height: 1,
        byte_len: 4,
        format: ImageFormat::Rgba8,
        digest: [3; 32],
        rgba: vec![9, 8, 7, 6],
    });
    let update = PaneUpdate {
        pane,
        seq: ServerSeq(1),
        image_events: vec![event.clone()],
        frame: Some(FrameDelta::Full {
            generation: 1,
            snapshot: snapshot(1, "x"),
        }),
    };
    let mut client = MuxClient::new(ScriptedDomain::default());
    let pane = client.spawn_pane(PaneSpawnOptions::default()).unwrap();
    client
        .domain_mut()
        .events
        .push_back(DomainEvent::PaneUpdate(update));

    let refresh = client.refresh_updates().unwrap();

    assert_eq!(refresh.image_events, vec![event]);
    assert!(refresh.frame_dirty);
    assert_eq!(client.pane_view(pane).unwrap().generation(), Some(1));
}

#[test]
fn pane_handle_routes_commands_through_domain() {
    let mut client = MuxClient::new(ScriptedDomain::default());
    let pane = client.spawn_pane(PaneSpawnOptions::default()).unwrap();

    client.pane(pane).write(Bytes::from_static(b"abc")).unwrap();

    assert_eq!(
        client.domain().writes,
        vec![(pane, Bytes::from_static(b"abc"))]
    );
}

#[test]
fn protocol_domain_encodes_commands_and_decodes_server_updates() {
    let (client_transport, server_transport) = InProcessTransport::pair();
    let pane = pane_ref();
    let mut domain = ProtocolDomain::new(client_transport);

    domain.write(pane, Bytes::from_static(b"abc")).unwrap();
    let frame = server_transport.try_recv().unwrap().unwrap();
    assert_eq!(frame.stream_id, StreamId::INPUT);
    assert_eq!(
        decode_client_frame(&frame).unwrap(),
        ClientMessage::PaneInput {
            pane,
            bytes: b"abc".to_vec()
        }
    );

    server_transport
        .send(
            encode_server_frame(ServerMessage::PaneUpdate(PaneUpdate {
                pane,
                seq: ServerSeq(7),
                image_events: Vec::new(),
                frame: Some(FrameDelta::Full {
                    generation: 1,
                    snapshot: snapshot(1, "z"),
                }),
            }))
            .unwrap(),
        )
        .unwrap();

    let mut events = Vec::new();
    domain
        .drain_events(&mut |event| events.push(event))
        .unwrap();
    assert!(
        matches!(events.as_slice(), [DomainEvent::PaneUpdate(update)] if update.seq == ServerSeq(7))
    );
}

#[test]
fn frame_history_replays_retained_updates() {
    let pane = pane_ref();
    let mut history = PaneFrameHistory::new(pane, 4);
    history.push(PaneUpdate {
        pane,
        seq: ServerSeq(1),
        image_events: Vec::new(),
        frame: Some(FrameDelta::Full {
            generation: 1,
            snapshot: snapshot(1, "a"),
        }),
    });
    history.push(PaneUpdate {
        pane,
        seq: ServerSeq(2),
        image_events: Vec::new(),
        frame: Some(FrameDelta::Partial {
            base_generation: 1,
            generation: 2,
            cols: 1,
            rows: 1,
            cursor: CursorInfo::default(),
            modes: TerminalModes::default(),
            pwd: None,
            placements: Vec::new(),
            dirty_rows: vec![RowDelta::from_snapshot_row(&snapshot(2, "b"), 0).unwrap()],
            hyperlinks: Vec::new(),
        }),
    });

    let replay = history.replay_since(Some(ServerSeq(1))).unwrap();
    assert!(
        matches!(replay, ReplayBatch::Replay(updates) if updates.len() == 1 && updates[0].seq == ServerSeq(2))
    );
}

#[test]
fn frame_history_resyncs_when_update_fell_out_of_ring() {
    let pane = pane_ref();
    let mut history = PaneFrameHistory::new(pane, 2);
    for seq in 1..=4 {
        history.push(PaneUpdate {
            pane,
            seq: ServerSeq(seq),
            image_events: Vec::new(),
            frame: Some(FrameDelta::Full {
                generation: seq,
                snapshot: snapshot(seq, "x"),
            }),
        });
    }

    let replay = history.replay_since(Some(ServerSeq(1))).unwrap();
    assert!(matches!(replay, ReplayBatch::Resync { full } if full.seq == ServerSeq(4)));
}

#[test]
fn protocol_spawn_uses_spawn_message_kind() {
    let (client_transport, server_transport) = InProcessTransport::pair();
    let mut domain = ProtocolDomain::new(client_transport);

    let err = domain.spawn_pane(PaneSpawnOptions::default()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "protocol pane spawn awaits server topology"
    );

    let frame = server_transport.try_recv().unwrap().unwrap();
    let message = decode_client_frame(&frame).unwrap();
    assert!(matches!(message, ClientMessage::SpawnPane { .. }));
    assert_eq!(MessageKind::ClientSpawnPane, message.kind());
}
