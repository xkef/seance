//! Threading stress harness (parent: M2 epic #5, sub-issue #172).
//!
//! Concurrency bugs surface under load, not under `cargo test`. The VT actor
//! ([`seance_vt::spawn_vt_session`]) owns libghostty and the PTY on one IO
//! thread; UI code sends [`seance_vt::VtCommand`]s and consumes immutable
//! snapshots. This module drives that actor hard and asserts the invariants
//! the actor model is supposed to hold — keystroke-to-echo latency under a
//! reader flood, snapshot coalescing across DEC 2026 sync blocks, an
//! exactly-once `Exited` on the shutdown race, wake de-duplication, resize
//! coalescing, and interactive-command latency while flooding.
//!
//! Headless — no winit. An [`mpsc::Sender`] substitutes for the winit
//! `EventLoopProxy`; the [`EventCounters`] behind it stand in for the
//! `send_event` call count the real proxy would see.
//!
//! The load generator is a PTY loopback: the spawned shell runs
//! `stty raw -echo; cat`, so every byte the harness writes is echoed back
//! verbatim and parsed by the VT. That gives a byte-exact, shell-agnostic way
//! to feed escape sequences (sync-output toggles, large payloads) through the
//! real IO path without depending on prompt or echo quirks.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use bytes::Bytes;
use seance_vt::{Resize, VtEvent, VtSessionHandle, VtSessionOptions, VtSnapshot, spawn_vt_session};

use crate::Summary;

/// Per-variant tally of the wake-up events the actor emits. Under the real UI
/// these map one-to-one to `EventLoopProxy::send_event` calls, so the
/// `content_dirty` count is the wake-coalescing metric scenario 4 bounds.
#[derive(Debug, Default)]
pub struct EventCounters {
    pub content_dirty: AtomicU64,
    pub clipboard: AtomicU64,
    pub exited: AtomicU64,
}

impl EventCounters {
    fn record(&self, event: VtEvent) {
        let slot = match event {
            VtEvent::ContentDirty => &self.content_dirty,
            VtEvent::ClipboardActivity => &self.clipboard,
            VtEvent::Exited => &self.exited,
        };
        slot.fetch_add(1, Ordering::Relaxed);
    }

    pub fn content_dirty(&self) -> u64 {
        self.content_dirty.load(Ordering::Relaxed)
    }

    pub fn exited(&self) -> u64 {
        self.exited.load(Ordering::Relaxed)
    }
}

/// A live VT actor plus the event side-channel the UI would own.
pub struct ThreadingHarness {
    handle: VtSessionHandle,
    events: mpsc::Receiver<VtEvent>,
    counters: Arc<EventCounters>,
    cols: u16,
    rows: u16,
}

/// Distinctive token used to detect that the `cat` loopback is live: once the
/// whole grid holds only this string, the shell has entered raw mode and is
/// echoing our escape sequences back for the VT to interpret.
const READY_TOKEN: &str = "SEANCE_READY_MARKER";

impl ThreadingHarness {
    /// Spawn a VT actor whose event sink both forwards into `events` and tallies
    /// [`EventCounters`]. Returns `None` when the platform cannot spawn a PTY
    /// session (non-Unix, or a sandbox without a usable shell), so callers can
    /// skip gracefully instead of failing.
    pub fn spawn(options: VtSessionOptions) -> Option<Self> {
        let (cols, rows) = (options.cols, options.rows);
        let counters = Arc::new(EventCounters::default());
        let (tx, rx) = mpsc::channel();
        let sink_counters = Arc::clone(&counters);
        let handle = spawn_vt_session(options, move |event| {
            sink_counters.record(event);
            let _ = tx.send(event);
        })
        .ok()?;
        Some(Self {
            handle,
            events: rx,
            counters,
            cols,
            rows,
        })
    }

    pub fn with_defaults() -> Option<Self> {
        Self::spawn(VtSessionOptions::default())
    }

    pub fn counters(&self) -> &Arc<EventCounters> {
        &self.counters
    }

    /// Feed raw bytes to the shell's stdin. In the loopback configuration the
    /// shell (`cat`) echoes them straight back into the VT.
    pub fn write(&self, bytes: impl Into<Bytes>) -> bool {
        self.handle.write(bytes.into()).is_ok()
    }

    fn latest(&self) -> Option<Arc<VtSnapshot>> {
        self.handle.latest_snapshot()
    }

    /// Simulate one render tick: re-arm the content-dirty gate and acknowledge
    /// the newest generation, exactly as the winit loop does after painting a
    /// frame. Without this the actor emits `ContentDirty` only once, so the
    /// wake-coalescing behaviour under load only shows up once a consumer keeps
    /// clearing the gate.
    pub fn render_tick(&self) {
        if let Some(snapshot) = self.latest() {
            let _ = self.handle.ack_rendered(snapshot.generation);
        }
        self.handle.clear_content_dirty_pending();
    }

    /// Drain every pending event without blocking, returning how many arrived.
    fn drain_events(&self) -> usize {
        let mut n = 0;
        while self.events.try_recv().is_ok() {
            n += 1;
        }
        n
    }

    /// Block until `predicate` holds for the latest snapshot or `timeout`
    /// elapses. Waits on the event channel between checks so it stays
    /// event-driven rather than busy-spinning.
    fn wait_until(
        &self,
        timeout: Duration,
        mut predicate: impl FnMut(&VtSnapshot) -> bool,
    ) -> Option<Arc<VtSnapshot>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(snapshot) = self.latest()
                && predicate(&snapshot)
            {
                return Some(snapshot);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            // Ignore the event payload — we re-read the snapshot slot on wake.
            let _ = self
                .events
                .recv_timeout(remaining.min(Duration::from_millis(8)));
        }
    }

    /// Put the shell into a raw byte-loopback (`stty raw -echo; cat`) and wait
    /// until the grid confirms the VT is interpreting our echoed escapes.
    /// Returns `false` if the loopback never came up within `timeout`.
    pub fn enter_loopback(&self, timeout: Duration) -> bool {
        if !self.write(Bytes::from_static(b"stty raw -echo; cat\n")) {
            return false;
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            // Clear the screen and drop the token at home; once the whole grid
            // reads back as just the token, cat is live and echoing raw bytes.
            let mut probe = b"\x1b[2J\x1b[H".to_vec();
            probe.extend_from_slice(READY_TOKEN.as_bytes());
            if !self.write(Bytes::from(probe)) {
                return false;
            }
            if self
                .wait_until(Duration::from_millis(200), |snap| {
                    snap.text.trim() == READY_TOKEN
                })
                .is_some()
            {
                // Clear once more so scenarios start from a blank grid.
                let _ = self.write(Bytes::from_static(b"\x1b[2J\x1b[H"));
                self.wait_until(Duration::from_millis(200), |snap| {
                    snap.text.trim().is_empty()
                });
                self.render_tick();
                self.drain_events();
                return true;
            }
        }
        false
    }

    /// Send `Shutdown`, then join the IO thread, bounding the wait so a hung
    /// join surfaces as `false` rather than blocking the test forever.
    pub fn join_within(self, timeout: Duration) -> bool {
        let ThreadingHarness { handle, .. } = self;
        let (done_tx, done_rx) = mpsc::channel();
        // `join` consumes the handle, so hand it to a watcher thread and wait on
        // a channel with a deadline.
        std::thread::spawn(move || {
            let _ = handle.join();
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(timeout).is_ok()
    }

    fn write_flood_chunk(&self, byte: u8, len: usize) -> bool {
        self.write(Bytes::from(vec![byte; len]))
    }
}

/// A single unit of load plus the metric a scenario reports. Latency summaries
/// reuse the crate [`Summary`] percentile machinery.
#[derive(Debug, Clone)]
pub struct LatencyReport {
    pub label: &'static str,
    pub samples: usize,
    pub summary: Summary,
}

/// Scenario 1 — reader saturation. Interleave a continuous byte flood with
/// unique markers and time how long each marker takes to surface in a snapshot.
/// Faithful to the "type one key per frame while the reader is buried" shape
/// without depending on a shell's own echo semantics.
pub fn reader_saturation(h: &ThreadingHarness, marks: usize) -> LatencyReport {
    const FLOOD_CHUNK: usize = 8 * 1024;
    let mut latencies = Vec::with_capacity(marks);
    for i in 0..marks {
        // Bury the reader, then drop a distinctive marker on its own line.
        let _ = h.write_flood_chunk(b'x', FLOOD_CHUNK);
        let mark = format!("\r\nMK{i:06}\r\n");
        let needle = format!("MK{i:06}");
        let start = Instant::now();
        let _ = h.write(Bytes::from(mark));
        if h.wait_until(Duration::from_millis(1000), |snap| {
            snap.text.contains(needle.as_str())
        })
        .is_some()
        {
            latencies.push(start.elapsed().as_nanos() as u64);
        }
        h.render_tick();
    }
    LatencyReport {
        label: "reader_saturation",
        samples: latencies.len(),
        summary: Summary::from_samples(&latencies),
    }
}

/// Scenario 2 — DEC 2026 synchronized-output bursts. Each burst wraps a large
/// payload in `\x1b[?2026h` … `\x1b[?2026l`. The actor must hold snapshot
/// publishes for the duration of each sync block and emit roughly one publish
/// per explicit close, not one per interior write.
#[derive(Debug, Clone, Copy)]
pub struct SyncBurstReport {
    pub bursts: u64,
    pub content_dirty_events: u64,
    pub final_generation: u64,
}

pub fn dec2026_bursts(h: &ThreadingHarness, bursts: u64) -> SyncBurstReport {
    const PAYLOAD: usize = 16 * 1024;
    let before = h.counters().content_dirty();
    for i in 0..bursts {
        let mut buf = Vec::with_capacity(PAYLOAD + 32);
        buf.extend_from_slice(b"\x1b[?2026h");
        // Interior writes that would each dirty the grid outside a sync block.
        buf.extend_from_slice(format!("\x1b[H\x1b[2Jsync{i:04}\r\n").as_bytes());
        buf.extend(std::iter::repeat_n(b'.', PAYLOAD));
        buf.extend_from_slice(b"\x1b[?2026l");
        let _ = h.write(Bytes::from(buf));
        // One render tick per burst re-arms the wake gate so a real publish can
        // wake us again on the next close.
        let needle = format!("sync{i:04}");
        h.wait_until(Duration::from_millis(200), |snap| {
            snap.text.contains(needle.as_str())
        });
        h.render_tick();
    }
    let after = h.counters().content_dirty();
    let final_generation = h.latest().map(|s| s.generation).unwrap_or(0);
    SyncBurstReport {
        bursts,
        content_dirty_events: after - before,
        final_generation,
    }
}

/// Scenario 3 — shutdown race. Spawn a session, make the child exit
/// immediately, and confirm `VtEvent::Exited` fires exactly once and the IO
/// thread joins without a timeout. Repeats `iterations` times.
#[derive(Debug, Clone, Copy)]
pub struct ShutdownRaceReport {
    pub iterations: u64,
    pub exited_once: u64,
    pub exited_multiple: u64,
    pub join_timeouts: u64,
    pub skipped: u64,
}

pub fn shutdown_race(iterations: u64) -> ShutdownRaceReport {
    let mut report = ShutdownRaceReport {
        iterations,
        exited_once: 0,
        exited_multiple: 0,
        join_timeouts: 0,
        skipped: 0,
    };
    for _ in 0..iterations {
        let Some(h) = ThreadingHarness::with_defaults() else {
            report.skipped += 1;
            continue;
        };
        // Ask the shell to exit right away, racing spawn against teardown.
        let _ = h.write(Bytes::from_static(b"exit 0\n"));
        let exited = h
            .wait_until(Duration::from_secs(2), |_| h.counters().exited() >= 1)
            .is_some()
            || h.counters().exited() >= 1;
        let count = h.counters().exited();
        if exited && count == 1 {
            report.exited_once += 1;
        } else if count > 1 {
            report.exited_multiple += 1;
        }
        if !h.join_within(Duration::from_secs(2)) {
            report.join_timeouts += 1;
        }
    }
    report
}

/// Scenario 4 — wake coalescing. Flood the reader for `duration` while a
/// render loop clears the content-dirty gate at ~60 Hz. The number of
/// `ContentDirty` wakes must stay far below the number of snapshot
/// generations (publishes) produced over the same window.
#[derive(Debug, Clone, Copy)]
pub struct WakeCoalesceReport {
    pub content_dirty_events: u64,
    pub generations: u64,
}

impl WakeCoalesceReport {
    /// Publishes per wake. A value well above 1 shows the gate is coalescing.
    pub fn ratio(&self) -> f64 {
        if self.content_dirty_events == 0 {
            self.generations as f64
        } else {
            self.generations as f64 / self.content_dirty_events as f64
        }
    }
}

pub fn wake_coalescing(h: &ThreadingHarness, duration: Duration) -> WakeCoalesceReport {
    const FLOOD_CHUNK: usize = 4 * 1024;
    let before = h.counters().content_dirty();
    let gen_before = h.latest().map(|s| s.generation).unwrap_or(0);
    let deadline = Instant::now() + duration;
    let mut next_tick = Instant::now() + Duration::from_millis(16);
    while Instant::now() < deadline {
        let _ = h.write_flood_chunk(b'y', FLOOD_CHUNK);
        // Yield so the actor thread drains what we just queued; without this a
        // tight writer can outrun the parser and let pending writes balloon.
        std::thread::yield_now();
        if Instant::now() >= next_tick {
            h.render_tick();
            next_tick += Duration::from_millis(16);
        }
    }
    // Let the last publishes settle, then take a final render tick.
    h.wait_until(Duration::from_millis(200), |_| false);
    h.render_tick();
    let after = h.counters().content_dirty();
    let gen_after = h.latest().map(|s| s.generation).unwrap_or(gen_before);
    WakeCoalesceReport {
        content_dirty_events: after - before,
        generations: gen_after.saturating_sub(gen_before),
    }
}

/// Scenario 5 — resize storm. Fire many resizes back to back; the actor
/// coalesces the command backlog so only the final size survives, and it does
/// far fewer VT resizes than commands sent.
#[derive(Debug, Clone, Copy)]
pub struct ResizeStormReport {
    pub commands_sent: u64,
    pub final_cols: u16,
    pub final_rows: u16,
    pub expected_cols: u16,
    pub expected_rows: u16,
}

impl ResizeStormReport {
    pub fn converged(&self) -> bool {
        self.final_cols == self.expected_cols && self.final_rows == self.expected_rows
    }
}

pub fn resize_storm(h: &ThreadingHarness, count: u64) -> ResizeStormReport {
    let base_cols = h.cols.max(20);
    let base_rows = h.rows.max(10);
    let mut last = (base_cols, base_rows);
    for i in 0..count {
        // Sweep through a handful of distinct sizes so coalescing has to pick
        // the final one, not just drop duplicates.
        let cols = base_cols + (i % 7) as u16;
        let rows = base_rows + (i % 5) as u16;
        last = (cols, rows);
        let _ = h.handle.resize(Resize {
            cols,
            rows,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
        });
    }
    let (expected_cols, expected_rows) = last;
    let settled = h.wait_until(Duration::from_secs(2), |snap| {
        snap.cols == expected_cols && snap.rows == expected_rows
    });
    let (final_cols, final_rows) = settled
        .map(|s| (s.cols, s.rows))
        .or_else(|| h.latest().map(|s| (s.cols, s.rows)))
        .unwrap_or((0, 0));
    ResizeStormReport {
        commands_sent: count,
        final_cols,
        final_rows,
        expected_cols,
        expected_rows,
    }
}

/// Scenario 6 — interactive command while flooding. The VT session exposes no
/// selection command (selection is resolved client-side in seance-mux-client),
/// so this adapts the original "selection while flooding" scenario to the
/// nearest observable interaction the actor owns: a resize issued mid-flood
/// must land in a snapshot within a bounded window, i.e. an out-of-band UI
/// command is not starved behind the reader backlog.
#[derive(Debug, Clone, Copy)]
pub struct InteractiveLatencyReport {
    pub applied: bool,
    pub latency: Duration,
}

pub fn command_while_flooding(
    h: &ThreadingHarness,
    flood_chunks: usize,
) -> InteractiveLatencyReport {
    const FLOOD_CHUNK: usize = 16 * 1024;
    let cols = h.cols.max(20) + 3;
    let rows = h.rows.max(10) + 2;
    for _ in 0..flood_chunks {
        let _ = h.write_flood_chunk(b'z', FLOOD_CHUNK);
    }
    let start = Instant::now();
    let _ = h.handle.resize(Resize {
        cols,
        rows,
        pixel_width: cols * 10,
        pixel_height: rows * 20,
    });
    let applied = h
        .wait_until(Duration::from_millis(500), |snap| {
            snap.cols == cols && snap.rows == rows
        })
        .is_some();
    InteractiveLatencyReport {
        applied,
        latency: start.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard: spin up the loopback once; if the environment cannot provide a
    /// PTY shell, skip the whole suite rather than fail.
    fn loopback() -> Option<ThreadingHarness> {
        let h = ThreadingHarness::with_defaults()?;
        if h.enter_loopback(Duration::from_secs(8)) {
            Some(h)
        } else {
            None
        }
    }

    #[test]
    fn reader_saturation_echoes_every_marker() {
        let Some(h) = loopback() else {
            eprintln!("skipping: no PTY loopback in this environment");
            return;
        };
        let report = reader_saturation(&h, 12);
        // Markers must keep surfacing despite the flood — the reader never
        // starves. Allow a single late echo for slow shared CI hosts.
        assert!(
            report.samples >= 11,
            "reader starved under flood: only {}/12 markers echoed",
            report.samples
        );
    }

    #[test]
    fn dec2026_coalesces_publishes() {
        let Some(h) = loopback() else {
            eprintln!("skipping: no PTY loopback in this environment");
            return;
        };
        let bursts = 8;
        let report = dec2026_bursts(&h, bursts);
        // Each burst contains many interior writes but one close; wakes must
        // not blow up past a small multiple of the burst count.
        assert!(
            report.content_dirty_events <= bursts * 3,
            "sync bursts over-published: {report:?}"
        );
        assert!(
            report.final_generation > 0,
            "no snapshot produced: {report:?}"
        );
    }

    #[test]
    fn shutdown_race_fires_exited_once() {
        // Cheap enough to run a handful of iterations in CI; the bench binary
        // drives the full 1000.
        let report = shutdown_race(20);
        if report.skipped == report.iterations {
            eprintln!("skipping: no PTY session in this environment");
            return;
        }
        assert_eq!(
            report.exited_multiple, 0,
            "Exited fired more than once: {report:?}"
        );
        assert_eq!(
            report.join_timeouts, 0,
            "IO thread failed to join: {report:?}"
        );
        let ran = report.iterations - report.skipped;
        assert!(
            report.exited_once >= ran / 2,
            "too few clean exits: {report:?}"
        );
    }

    #[test]
    fn wake_gate_coalesces_under_flood() {
        let Some(h) = loopback() else {
            eprintln!("skipping: no PTY loopback in this environment");
            return;
        };
        let report = wake_coalescing(&h, Duration::from_millis(600));
        assert!(
            report.generations > 0,
            "flood produced no snapshots: {report:?}"
        );
        // A ~60 Hz render loop over a heavy flood must see far fewer wakes than
        // publishes; a 4:1 floor is conservative for a de-dup gate.
        assert!(
            report.ratio() >= 4.0,
            "wake gate failed to coalesce: {report:?} ratio={:.1}",
            report.ratio()
        );
    }

    #[test]
    fn resize_storm_converges_to_last_size() {
        let Some(h) = loopback() else {
            eprintln!("skipping: no PTY loopback in this environment");
            return;
        };
        let report = resize_storm(&h, 500);
        assert!(
            report.converged(),
            "resize storm did not settle: {report:?}"
        );
    }

    #[test]
    fn command_lands_while_flooding() {
        let Some(h) = loopback() else {
            eprintln!("skipping: no PTY loopback in this environment");
            return;
        };
        let report = command_while_flooding(&h, 8);
        assert!(
            report.applied,
            "cursor-shape command lost under flood: {report:?}"
        );
    }
}
