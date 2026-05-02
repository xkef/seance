//! Winit event handlers that live on `App`. Split out from `app.rs` to keep
//! the main file focused on lifecycle and frame loop.

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton};
use winit::event_loop::ActiveEventLoop;

use seance_input::VtInput;

use crate::app::App;
use crate::command::AppCommand;
use crate::window_state::WindowState;

const FONT_SIZE_MIN: f32 = 6.0;
const FONT_SIZE_MAX: f32 = 72.0;

impl App {
    pub(crate) fn on_keyboard_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: &winit::event::KeyEvent,
    ) {
        let modes = self
            .window_state
            .as_ref()
            .map(WindowState::terminal_modes)
            .unwrap_or_default();
        let modifiers = self
            .window_state
            .as_ref()
            .map(|ws| ws.modifiers)
            .unwrap_or_default();

        if let Some(cmd) = self.keybinds.match_event(event, &modifiers) {
            let preserves_selection = matches!(cmd, AppCommand::Copy | AppCommand::SelectAll);
            if !preserves_selection
                && let Some(ws) = self.ws_mut()
                && ws.has_selection()
            {
                ws.clear_selection();
            }
            self.execute_app_command(event_loop, cmd);
            self.mark_dirty();
            return;
        }

        let input = self.input.handle_key(event, &modifiers, modes);

        if let Some(ws) = self.ws_mut() {
            if event.state == ElementState::Pressed
                && !matches!(input, VtInput::Ignore)
                && ws.has_selection()
            {
                ws.clear_selection();
            }
            if let VtInput::Write(bytes) = input {
                ws.terminal.write(&bytes);
            }
            ws.mark_dirty();
        }
    }

    fn execute_app_command(&mut self, event_loop: &ActiveEventLoop, cmd: AppCommand) {
        match cmd {
            AppCommand::Quit | AppCommand::CloseWindow => event_loop.exit(),
            AppCommand::Copy => {
                if let Some(ws) = self.ws_mut() {
                    ws.copy_selection_to_clipboard();
                    ws.clear_selection();
                }
            }
            AppCommand::Paste => {
                if let Some(ws) = self.ws_mut() {
                    ws.paste_from_clipboard();
                }
            }
            AppCommand::SelectAll => {
                if let Some(ws) = self.ws_mut() {
                    ws.select_all();
                    ws.sync_selection_to_overlay();
                }
            }
            AppCommand::FontSizeDelta(delta) => {
                self.font_size =
                    (self.font_size + f32::from(delta)).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
                self.apply_font_size();
            }
            AppCommand::FontSizeReset => {
                self.font_size = self.config.font.size;
                self.apply_font_size();
            }
        }
    }

    pub(crate) fn on_mouse_wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        let Some(ws) = self.window_state.as_mut() else {
            return;
        };
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                let ch = ws.cell_size[1].max(1.0);
                (pos.y / f64::from(ch)) as i32
            }
        };
        if lines == 0 {
            return;
        }
        let modes = ws.terminal_modes();
        if let Some(data) = self.input.encode_mouse_wheel(lines, modes) {
            ws.terminal.write(&data);
        } else {
            ws.terminal.scroll_lines(-lines);
        }
        ws.mark_dirty();
    }

    pub(crate) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let Some(ws) = self.window_state.as_mut() else {
            return;
        };
        ws.mouse.cursor_pos = position;
        if !ws.mouse.is_down {
            return;
        }
        let (col, row) = ws.renderer.pixel_to_grid(position.x, position.y);
        ws.update_selection(col, row);
        ws.sync_selection_to_overlay();
        ws.mark_dirty();
    }

    pub(crate) fn on_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        let Some(ws) = self.window_state.as_mut() else {
            return;
        };
        match state {
            ElementState::Pressed => handle_mouse_press(ws),
            ElementState::Released => {
                ws.mouse.is_down = false;
                ws.copy_selection_to_clipboard();
            }
        }
    }
}

fn handle_mouse_press(ws: &mut WindowState) {
    if ws.modifiers.state().super_key() {
        let _ = ws.window.drag_window();
        return;
    }
    let (col, row) = ws
        .renderer
        .pixel_to_grid(ws.mouse.cursor_pos.x, ws.mouse.cursor_pos.y);
    let clicks = ws.mouse.register_click(col, row);
    match clicks {
        1 => ws.start_selection(col, row),
        2 => ws.start_word_selection(col, row),
        3 => ws.start_line_selection(row),
        _ => {}
    }
    ws.sync_selection_to_overlay();
    ws.mouse.is_down = true;
    ws.mark_dirty();
}
