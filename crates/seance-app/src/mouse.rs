use std::time::{Duration, Instant};

use winit::dpi::PhysicalPosition;

const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);

pub(crate) struct MouseState {
    pub(crate) cursor_pos: PhysicalPosition<f64>,
    pub(crate) is_down: bool,
    click_count: u8,
    last_click_time: Instant,
    last_click_pos: (u16, u16),
}

impl Default for MouseState {
    fn default() -> Self {
        Self {
            cursor_pos: PhysicalPosition::new(0.0, 0.0),
            is_down: false,
            click_count: 0,
            last_click_time: Instant::now(),
            last_click_pos: (0, 0),
        }
    }
}

impl MouseState {
    pub(crate) fn register_click(&mut self, col: u16, row: u16) -> u8 {
        let now = Instant::now();
        let fast = now.duration_since(self.last_click_time) < MULTI_CLICK_WINDOW;
        let same_spot = (col, row) == self.last_click_pos;
        self.click_count = if fast && same_spot {
            (self.click_count % 3) + 1
        } else {
            1
        };
        self.last_click_time = now;
        self.last_click_pos = (col, row);
        self.click_count
    }
}
