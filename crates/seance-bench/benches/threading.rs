//! Threading stress-scenario runner (sub-issue #172).
//!
//! `harness = false`: this is a plain `main`, not a libtest bench. Run with
//! `cargo bench -p seance-bench threading`. It drives each scenario at full
//! scale, prints a table, and checks the pass/fail thresholds from the issue.
//! The strict latency bounds are calibrated for a release build on a real
//! machine; treat a red line in a sandbox as informational, not a hard gate.

use std::process::ExitCode;
use std::time::Duration;

use seance_bench::threading::{self, ThreadingHarness};

fn main() -> ExitCode {
    let Some(harness) = ThreadingHarness::with_defaults() else {
        eprintln!("threading bench: PTY session unavailable on this platform; nothing to run");
        return ExitCode::SUCCESS;
    };
    if !harness.enter_loopback(Duration::from_secs(8)) {
        eprintln!("threading bench: PTY loopback never came up; skipping");
        return ExitCode::SUCCESS;
    }

    let mut ok = true;

    // Scenario 1 — reader saturation.
    let sat = threading::reader_saturation(&harness, 200);
    println!(
        "reader_saturation: {} echoes  p50={:?} p99={:?} max={:?}",
        sat.samples, sat.summary.p50, sat.summary.p99, sat.summary.max
    );
    ok &= check("reader_saturation: every marker echoed", sat.samples == 200);

    // Scenario 2 — DEC 2026 sync bursts.
    let bursts = 1000;
    let sync = threading::dec2026_bursts(&harness, bursts);
    println!(
        "dec2026_bursts: {} bursts  wakes={}  final_gen={}",
        sync.bursts, sync.content_dirty_events, sync.final_generation
    );
    ok &= check(
        "dec2026_bursts: publishes within 10% of close count",
        sync.content_dirty_events <= bursts + bursts / 10,
    );

    // Scenario 4 — wake coalescing.
    let wake = threading::wake_coalescing(&harness, Duration::from_secs(5));
    println!(
        "wake_coalescing: wakes={} generations={} ratio={:.1}:1",
        wake.content_dirty_events,
        wake.generations,
        wake.ratio()
    );
    ok &= check(
        "wake_coalescing: <=10000 wakes over 5s",
        wake.content_dirty_events <= 10_000,
    );
    ok &= check(
        "wake_coalescing: >=100:1 publish:wake",
        wake.ratio() >= 100.0,
    );

    // Scenario 5 — resize storm.
    let resize = threading::resize_storm(&harness, 500);
    println!(
        "resize_storm: {} commands -> {}x{} (expected {}x{})",
        resize.commands_sent,
        resize.final_cols,
        resize.final_rows,
        resize.expected_cols,
        resize.expected_rows
    );
    ok &= check("resize_storm: converged to last size", resize.converged());

    // Scenario 6 — command while flooding.
    let interactive = threading::command_while_flooding(&harness, 32);
    println!(
        "command_while_flooding: applied={} latency={:?}",
        interactive.applied, interactive.latency
    );
    ok &= check(
        "command_while_flooding: command landed",
        interactive.applied,
    );

    // Scenario 3 — shutdown race (spawns fresh sessions; run last).
    let shutdown = threading::shutdown_race(1000);
    println!(
        "shutdown_race: {} iters  clean={}  multi={}  join_timeouts={}",
        shutdown.iterations, shutdown.exited_once, shutdown.exited_multiple, shutdown.join_timeouts
    );
    ok &= check(
        "shutdown_race: no double Exited",
        shutdown.exited_multiple == 0,
    );
    ok &= check(
        "shutdown_race: no join timeout",
        shutdown.join_timeouts == 0,
    );

    if ok {
        println!("\nall threading scenarios passed");
        ExitCode::SUCCESS
    } else {
        eprintln!("\none or more threading scenarios failed");
        ExitCode::FAILURE
    }
}

fn check(label: &str, pass: bool) -> bool {
    println!("  [{}] {label}", if pass { "PASS" } else { "FAIL" });
    pass
}
