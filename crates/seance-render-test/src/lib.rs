//! Layered test harness for the seance renderer.
//!
//! Phase A: `TestWorld` wraps a PTY-less `HeadlessTerminal` and exposes
//! `dump_frame()` — the LLM-readable ASCII grid + annotation dump that
//! powers Layer 4 snapshot tests.
//!
//! Layers 2, 3, 5, 6, 8 and the `seance-render-dbg` CLI arrive in later
//! phases; the module layout is shaped to host them without churn.

pub mod clock;
pub mod dump;
pub mod fonts;
pub mod rng;
pub mod world;

pub use clock::TestClock;
pub use fonts::TestFont;
pub use rng::DeterministicRng;
pub use world::TestWorld;
