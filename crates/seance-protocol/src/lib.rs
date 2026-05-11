//! Wire-format types and codec for the seance multiplexer protocol.
//!
//! Split into focused modules so callers learn only the seam they cross:
//!
//! - [`identity`] — newtype IDs (`PaneRef`, `ImageKey`, `ClientId`, …).
//! - [`frame`] — terminal grid snapshots, deltas, and per-cell types.
//! - [`image_cache`] — OSC-side image upload events.
//! - [`mux`] — `ClientMessage` / `ServerMessage` and topology types.
//! - [`transport`] — envelope framing, codec, and the [`transport::Transport`]
//!   trait + [`transport::InProcessTransport`].
//! - [`limits`] — DoS budget constants gating per-connection backpressure.
//!
//! See `docs/protocol.md` for the protocol design.

#![warn(missing_docs)]

pub mod frame;
pub mod identity;
pub mod image_cache;
pub mod limits;
pub mod mux;
pub mod transport;
