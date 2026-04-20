//! Frame-time harness runner.
//!
//! Iterates each built-in workload, records p50/p95/p99 for CPU and
//! headless-GPU submit, prints a fixed-width table to stdout.

use std::time::Duration;

use seance_bench::gpu::HeadlessGpu;
use seance_bench::workloads::Workload;
use seance_bench::{Stopwatch, Summary};

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
        print_row(workload.name, "cpu", &cpu);
        if let Some(s) = gpu_summary {
            print_row(workload.name, "gpu-submit", &s);
        }
    }
}

/// CPU proxy: sum the workload bytes (stand-in for future VT-feed +
/// `CellBuilder::build_frame`). Cost scales with byte count so it's
/// a meaningful relative baseline between workloads even as a stub.
fn run_cpu(workload: &Workload, iterations: usize) -> Summary {
    let mut sw = Stopwatch::with_capacity(iterations);
    for _ in 0..iterations {
        sw.time(|| {
            let mut acc: u64 = 0;
            for b in &workload.bytes {
                acc = acc.wrapping_add(*b as u64);
            }
            std::hint::black_box(acc);
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
        format!("{}ns", ns)
    } else if ns < 10_000_000 {
        format!("{:.1}µs", ns as f64 / 1_000.0)
    } else {
        format!("{:.1}ms", ns as f64 / 1_000_000.0)
    }
}
