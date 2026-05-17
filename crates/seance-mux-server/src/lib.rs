//! Server-side mux.
//!
//! The frontend (renderer, input, window) lives in `seance-mux-client` and speaks
//! only the wire protocol from `seance-protocol`. This crate is the
//! counterpart that owns terminal state on the server side:
//!
//! - [`LocalDomain`] wraps `seance-vt` VT sessions as a [`seance_mux_client::Domain`]
//!   impl. It's the "local server" — a process spawning real PTYs.
//! - [`serve`] is the protocol-dispatch loop: it reads `ClientMessage`s off a
//!   `Transport`, drives a `Domain`, and pushes `ServerMessage`s back. The
//!   same loop is reused for in-process bootstrap today and for cross-process
//!   daemons under [M12].
//! - [`spawn_local_server`] is the in-process bootstrap: it pairs a
//!   `LocalDomain` with an `InProcessTransport` and returns the client end
//!   the frontend will hand to [`seance_mux_client::ProtocolDomain`].
//!
//! [M12]: https://github.com/xkef/seance/issues/221

mod local;
mod serve;

pub use local::LocalDomain;
pub use serve::{ServeConfig, ServeError, serve, spawn_local_server};
