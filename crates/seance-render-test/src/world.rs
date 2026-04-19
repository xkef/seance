//! Top-level test fixture.
//!
//! Owns a headless VT, a deterministic clock, and a seeded RNG. Later
//! phases add a lazy headless wgpu instance for L5 render-to-texture;
//! Phase A keeps GPU state out of scope.

use seance_vt::FrameSource;
use seance_vt::test_support::HeadlessTerminal;

use crate::clock::TestClock;
use crate::dump::dump_frame;
use crate::fonts::TestFont;
use crate::rng::DeterministicRng;

const DEFAULT_SEED: u64 = 0xDEAD_BEEF;

pub struct TestWorld {
    cols: u16,
    rows: u16,
    vt: HeadlessTerminal,
    clock: TestClock,
    rng: DeterministicRng,
    font: TestFont,
}

impl TestWorld {
    pub fn new(cols: u16, rows: u16) -> Self {
        let vt = HeadlessTerminal::new(cols, rows)
            .expect("HeadlessTerminal construction should not fail");
        Self {
            cols,
            rows,
            vt,
            clock: TestClock::new(),
            rng: DeterministicRng::new(DEFAULT_SEED),
            font: TestFont::default(),
        }
    }

    pub fn with_font(mut self, font: TestFont) -> Self {
        self.font = font;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.rng = DeterministicRng::new(seed);
        self
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.vt.feed(bytes);
    }

    pub fn feed_fixture(&mut self, name: &str) {
        self.vt.feed(load_fixture(name));
    }

    pub fn tick(&self) -> u64 {
        self.clock.tick()
    }

    pub fn clock(&self) -> &TestClock {
        &self.clock
    }

    pub fn rng(&mut self) -> &mut DeterministicRng {
        &mut self.rng
    }

    pub fn font(&self) -> TestFont {
        self.font
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn terminal(&self) -> &HeadlessTerminal {
        &self.vt
    }

    pub fn terminal_mut(&mut self) -> &mut HeadlessTerminal {
        &mut self.vt
    }

    pub fn dump_frame(&mut self) -> String {
        let source: &mut dyn FrameSource = &mut self.vt;
        dump_frame(source)
    }
}

fn load_fixture(name: &str) -> &'static [u8] {
    match name {
        "empty" => include_bytes!("../fixtures/vt_streams/empty.bin"),
        "hello_world" => include_bytes!("../fixtures/vt_streams/hello_world.bin"),
        "ansi_colors" => include_bytes!("../fixtures/vt_streams/ansi_colors.bin"),
        "box_drawing" => include_bytes!("../fixtures/vt_streams/box_drawing.bin"),
        "wide_chars" => include_bytes!("../fixtures/vt_streams/wide_chars.bin"),
        other => panic!("unknown fixture: {other}"),
    }
}
