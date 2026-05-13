use bytes::Bytes;
use seance_protocol::frame::{CursorShape, Resize, ThemeColors};
use seance_protocol::identity::PaneRef;
use seance_vt::DEFAULT_MAX_SCROLLBACK;

use crate::{DomainEvent, PaneError, SpawnError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSpawnOptions {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub initial_cursor_shape: CursorShape,
    pub max_scrollback: usize,
}

impl Default for PaneSpawnOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
            initial_cursor_shape: CursorShape::Block,
            max_scrollback: DEFAULT_MAX_SCROLLBACK,
        }
    }
}

impl From<PaneSpawnOptions> for seance_vt::VtSessionOptions {
    fn from(value: PaneSpawnOptions) -> Self {
        Self {
            cols: value.cols,
            rows: value.rows,
            pixel_width: value.pixel_width,
            pixel_height: value.pixel_height,
            initial_cursor_shape: value.initial_cursor_shape,
            max_scrollback: value.max_scrollback,
        }
    }
}

pub trait Domain {
    fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError>;

    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError>;

    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError>;

    fn resize(&mut self, pane: PaneRef, resize: Resize) -> Result<(), PaneError>;

    fn scroll_lines(&mut self, pane: PaneRef, delta: i32) -> Result<(), PaneError>;

    fn set_theme_colors(&mut self, pane: PaneRef, colors: ThemeColors) -> Result<(), PaneError>;

    fn set_cursor_shape(&mut self, pane: PaneRef, shape: CursorShape) -> Result<(), PaneError>;

    fn ack_presented(&mut self, pane: PaneRef, generation: u64) -> Result<(), PaneError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_spawn_options_propagate_max_scrollback_to_vt() {
        let options = PaneSpawnOptions {
            max_scrollback: 12_345,
            ..PaneSpawnOptions::default()
        };
        let vt: seance_vt::VtSessionOptions = options.into();
        assert_eq!(vt.max_scrollback, 12_345);
    }
}
