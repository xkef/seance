use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use bytes::Bytes;
use seance_mux_client::{
    Domain, DomainEvent, MuxClient, PaneError, PaneSpawnOptions, ProtocolDomain, SpawnError,
};
use seance_mux_server::{ServeConfig, serve};
use seance_protocol::frame::{CursorShape, Resize, ThemeColors};
use seance_protocol::identity::{DomainId, PaneEpoch, PaneId, PaneRef};
use seance_protocol::transport::InProcessTransport;

/// Stub Domain that exercises `serve` without spawning real PTYs. Records
/// writes so the test can assert they made the round-trip from client to
/// server side over the wire protocol.
#[derive(Default)]
struct RecordingDomain {
    writes: Arc<Mutex<Vec<(PaneRef, Bytes)>>>,
    queued: VecDeque<DomainEvent>,
    next_id: u64,
}

impl Domain for RecordingDomain {
    fn spawn_pane(&mut self, _options: PaneSpawnOptions) -> Result<PaneRef, SpawnError> {
        self.next_id += 1;
        Ok(PaneRef {
            domain: DomainId(1),
            pane_id: PaneId(self.next_id),
            epoch: PaneEpoch(1),
        })
    }

    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError> {
        while let Some(event) = self.queued.pop_front() {
            sink(event);
        }
        Ok(())
    }

    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError> {
        self.writes.lock().unwrap().push((pane, bytes));
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
fn frontend_to_backend_round_trips_via_protocol() {
    let (client_transport, server_transport) = InProcessTransport::pair();
    let writes: Arc<Mutex<Vec<(PaneRef, Bytes)>>> = Arc::new(Mutex::new(Vec::new()));
    let domain = RecordingDomain {
        writes: Arc::clone(&writes),
        ..RecordingDomain::default()
    };

    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let server_thread = thread::spawn(move || {
        let config = ServeConfig::new(move || {
            let _ = wake_tx.send(());
        });
        let _ = serve(domain, server_transport, config);
    });

    let mut client = MuxClient::new(ProtocolDomain::new(client_transport));
    let pane = client.spawn_pane(PaneSpawnOptions::default()).unwrap();
    client.pane(pane).write(Bytes::from_static(b"hi")).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if writes
            .lock()
            .unwrap()
            .iter()
            .any(|(_, bytes)| bytes == b"hi".as_slice())
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("write did not reach server within timeout");
        }
        let _ = wake_rx.recv_timeout(Duration::from_millis(50));
    }

    let recorded = writes.lock().unwrap().clone();
    assert!(
        recorded
            .iter()
            .any(|(p, b)| *p == pane && b == b"hi".as_slice())
    );

    // Drop the client transport so serve exits.
    drop(client);
    let _ = server_thread.join();
}
