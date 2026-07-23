//! Frame-time bench harness (parent: M2 epic #5, sub-issue #26).
//!
//! Two measurement surfaces:
//!   1. CPU per-frame timing via [`Stopwatch`] — hooks a closure, produces
//!      p50/p95/p99 over N samples. [`drive_frame`] feeds a workload's bytes
//!      through a real PTY-less [`seance_vt::test_support::HeadlessTerminal`]
//!      and extracts a snapshot, so the number reflects actual VT ingest +
//!      snapshot-rebuild cost rather than a byte-count proxy.
//!   2. Headless GPU adapter/device via [`gpu::HeadlessGpu`] — a wgpu
//!      `Instance` with `Backends::PRIMARY`, no surface; `submit_noop` times
//!      an empty command buffer submit + `Maintain::Wait`.
//!
//! The stream of VT bytes a workload feeds is owned by [`workloads::Workload`].
//! The GPU-side cell rebuild (`CellBuilder::build_frame`) is not yet timed here:
//! the renderer still needs a surface, so only the CPU snapshot path runs
//! headless. GPU pipelines fold into surface (2) once they run surfaceless.

pub mod gpu;
pub mod workloads;

use std::time::Duration;

use seance_vt::VtSnapshot;
use seance_vt::test_support::HeadlessTerminal;

/// Grid the headless VT runs at while benching. A fixed 80×50 keeps the
/// per-frame snapshot cost comparable across workloads and matches the row
/// counts the workloads in [`workloads`] are shaped for.
pub const BENCH_COLS: u16 = 80;
pub const BENCH_ROWS: u16 = 50;

/// Run one render-loop cycle against a real headless VT: feed `bytes`, extract
/// a snapshot (the CPU frame rebuild), then ack the generation so dirty
/// tracking resets before the next cycle. Returns the snapshot so callers can
/// time it, inspect it, or `black_box` it to defeat dead-code elimination.
pub fn drive_frame(term: &mut HeadlessTerminal, bytes: &[u8]) -> VtSnapshot {
    term.feed(bytes);
    let snapshot = term
        .snapshot()
        .expect("headless snapshot should not fail on bench workloads");
    term.ack_rendered(snapshot.generation);
    snapshot
}

/// Collects per-iteration durations and reports percentile summaries.
///
/// `Duration` is kept as raw nanoseconds internally for sort + percentile
/// math; we don't need higher precision than the OS clock.
pub struct Stopwatch {
    samples: Vec<u64>,
}

impl Stopwatch {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            samples: Vec::with_capacity(n),
        }
    }

    /// Run `f` once, recording its elapsed time.
    pub fn time<F: FnOnce()>(&mut self, f: F) {
        let t0 = std::time::Instant::now();
        f();
        self.samples.push(t0.elapsed().as_nanos() as u64);
    }

    pub fn summary(&self) -> Summary {
        Summary::from_samples(&self.samples)
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

/// Sorted percentile summary over a sample set.
#[derive(Debug, Clone, Copy)]
pub struct Summary {
    pub count: usize,
    pub min: Duration,
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
    pub max: Duration,
    pub mean: Duration,
}

impl Summary {
    pub fn from_samples(samples: &[u64]) -> Self {
        if samples.is_empty() {
            let z = Duration::ZERO;
            return Self {
                count: 0,
                min: z,
                p50: z,
                p95: z,
                p99: z,
                max: z,
                mean: z,
            };
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let pct = |p: f64| {
            let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
            Duration::from_nanos(sorted[idx])
        };
        let sum: u128 = sorted.iter().map(|n| *n as u128).sum();
        Self {
            count: sorted.len(),
            min: Duration::from_nanos(sorted[0]),
            p50: pct(0.50),
            p95: pct(0.95),
            p99: pct(0.99),
            max: Duration::from_nanos(*sorted.last().unwrap()),
            mean: Duration::from_nanos((sum / sorted.len() as u128) as u64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_on_linear_ramp() {
        // 101 samples [0..=100] so `round((N-1) * p)` lands on exact integers.
        let samples: Vec<u64> = (0..=100).collect();
        let s = Summary::from_samples(&samples);
        assert_eq!(s.count, 101);
        assert_eq!(s.min, Duration::from_nanos(0));
        assert_eq!(s.max, Duration::from_nanos(100));
        assert_eq!(s.p50, Duration::from_nanos(50));
        assert_eq!(s.p95, Duration::from_nanos(95));
        assert_eq!(s.p99, Duration::from_nanos(99));
    }

    #[test]
    fn empty_summary_is_zero() {
        let s = Summary::from_samples(&[]);
        assert_eq!(s.count, 0);
        assert_eq!(s.mean, Duration::ZERO);
    }

    #[test]
    fn stopwatch_records_samples() {
        let mut sw = Stopwatch::with_capacity(4);
        for _ in 0..4 {
            sw.time(|| {
                let _ = std::hint::black_box(1 + 1);
            });
        }
        assert_eq!(sw.sample_count(), 4);
    }

    #[test]
    fn drive_frame_advances_generation() {
        let mut term = HeadlessTerminal::new(BENCH_COLS, BENCH_ROWS).unwrap();
        let first = drive_frame(&mut term, b"hello\r\n");
        let second = drive_frame(&mut term, b"world\r\n");
        assert!(second.generation > first.generation);
    }

    #[test]
    fn drives_every_workload_without_panicking() {
        for w in workloads::Workload::all() {
            let mut term = HeadlessTerminal::new(BENCH_COLS, BENCH_ROWS).unwrap();
            for _ in 0..2 {
                let snapshot = drive_frame(&mut term, &w.bytes);
                assert_eq!(snapshot.cols, BENCH_COLS, "{}", w.name);
                assert_eq!(snapshot.rows, BENCH_ROWS, "{}", w.name);
            }
        }
    }
}
