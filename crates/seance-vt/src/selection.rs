//! Selection types — re-exported from `seance-protocol` so callers of
//! `seance-vt` can build/inspect selections without taking a direct
//! dependency on the protocol crate.

pub use seance_protocol::frame::{GridPos, Selection, SelectionGranularity};
