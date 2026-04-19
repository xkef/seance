//! Deterministic clock for tests.
//!
//! Replaces `Instant::now()` wherever the renderer would consult wall
//! time (cursor blink, animations, atlas generation counters). Tests
//! `tick()` it manually so snapshots are stable across runs.

use std::cell::Cell;

#[derive(Default)]
pub struct TestClock {
    ticks: Cell<u64>,
}

impl TestClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(&self) -> u64 {
        let next = self.ticks.get() + 1;
        self.ticks.set(next);
        next
    }

    pub fn now(&self) -> u64 {
        self.ticks.get()
    }

    pub fn reset(&self) {
        self.ticks.set(0);
    }
}
