//! Wire-level protocol for seance's mux.
//!
//! Owned data only — nothing here borrows from libghostty, wgpu, or winit,
//! so every type is safe to serialize, send across a transport, and
//! reassemble on another machine.
//!
//! - [`identity`] — opaque newtype IDs ([`identity::DomainId`],
//!   [`identity::PaneRef`], [`identity::ImageKey`], …).
//! - [`frame`] — owned snapshot ([`frame::VtSnapshot`]), [`frame::FrameDelta`],
//!   cursor / placement / cell types shared with the renderer through
//!   `seance-frame`.
//! - [`mux`] — [`mux::ClientMessage`] / [`mux::ServerMessage`] enums,
//!   [`mux::PaneUpdate`], topology.
//! - [`transport`] — length-prefixed postcard envelopes, the
//!   [`transport::Transport`] trait, and [`transport::InProcessTransport`]
//!   for in-process bootstrap and tests.
//! - [`image_cache`] — out-of-band image put / evict events, ordered with
//!   the frame they apply to.
//! - [`clipboard`] — OSC 52 request data (parser lives in `seance-vt`).
//! - [`limits`] — protocol-wide byte limits and history bounds.
//! - [`agent`] — [`agent::PaneSnapshot`], the stable versioned snapshot
//!   exposed to agent-plane consumers.

pub mod agent;
pub mod clipboard;
pub mod frame;
pub mod identity;
pub mod image_cache;
pub mod limits;
pub mod mux;
pub mod transport;
