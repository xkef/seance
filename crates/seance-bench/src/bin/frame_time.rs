//! Frame-time harness runner.
//!
//! Iterates each built-in workload, records p50/p95/p99 for the CPU
//! VT-frame path and headless-GPU submit, prints a fixed-width table to
//! stdout.

use std::time::Duration;

use seance_bench::gpu::HeadlessGpu;
use seance_bench::workloads::Workload;
use seance_bench::{BENCH_COLS, BENCH_ROWS, Stopwatch, Summary, drive_frame};
use seance_vt::test_support::HeadlessTerminal;

const DEFAULT_ITERATIONS: usize = 10_000;

fn main() {
    let iterations = std::env::var("SEANCE_BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ITERATIONS);

    let gpu = pollster::block_on(HeadlessGpu::new());
    match &gpu {
        Some(g) => println!("headless gpu: {}", g.adapter_name),
        None => println!("headless gpu: unavailable (skipping GPU timings)"),
    }
    println!("iterations per workload: {iterations}\n");

    print_header();
    for workload in Workload::all() {
        let cpu = run_cpu(&workload, iterations);
        let gpu_summary = gpu.as_ref().map(|g| run_gpu(g, iterations));
        print_row(workload.name, "vt-frame", &cpu);
        if let Some(s) = gpu_summary {
            print_row(workload.name, "gpu-submit", &s);
        }
    }
}

/// CPU per-frame cost: feed the workload bytes through a real headless VT and
/// extract a snapshot each iteration. A single terminal is reused across the
/// run so the measured cost reflects steady-state ingest + snapshot rebuild
/// under continuous output, the way a live render loop sees it.
fn run_cpu(workload: &Workload, iterations: usize) -> Summary {
    let mut term = HeadlessTerminal::new(BENCH_COLS, BENCH_ROWS)
        .expect("headless terminal construction should not fail");
    let mut sw = Stopwatch::with_capacity(iterations);
    for _ in 0..iterations {
        sw.time(|| {
            std::hint::black_box(drive_frame(&mut term, &workload.bytes));
        });
    }
    sw.summary()
}

fn run_gpu(gpu: &HeadlessGpu, iterations: usize) -> Summary {
    let mut sw = Stopwatch::with_capacity(iterations);
    for _ in 0..iterations {
        sw.time(|| {
            std::hint::black_box(gpu.submit_noop());
        });
    }
    sw.summary()
}

fn print_header() {
    println!(
        "{:<16} {:<12} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "workload", "phase", "p50", "p95", "p99", "min", "max"
    );
    println!("{}", "-".repeat(84));
}

fn print_row(workload: &str, phase: &str, s: &Summary) {
    println!(
        "{:<16} {:<12} {:>10} {:>10} {:>10} {:>10} {:>10}",
        workload,
        phase,
        fmt(s.p50),
        fmt(s.p95),
        fmt(s.p99),
        fmt(s.min),
        fmt(s.max),
    );
}

fn fmt(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns < 10_000 {
        format!("{ns}ns")
    } else if ns < 10_000_000 {
        format!("{:.1}µs", ns as f64 / 1_000.0)
    } else {
        format!("{:.1}ms", ns as f64 / 1_000_000.0)
    }
}
