//! Winit event handlers that live on `App`. Split out from `app.rs` to keep
//! the main file focused on lifecycle and frame loop.

use bytes::Bytes;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton};
use winit::event_loop::ActiveEventLoop;

use seance_input::{MouseAction, MouseEventInput, VtInput};

use crate::app::App;
use crate::command::AppCommand;
use crate::surface_state::SurfaceState;

const FONT_SIZE_MIN: f32 = 6.0;
const FONT_SIZE_MAX: f32 = 72.0;

impl App {
    pub(crate) fn on_keyboard_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: &winit::event::KeyEvent,
    ) {
        let modes = self
            .surface
            .as_ref()
            .map(SurfaceState::terminal_modes)
            .unwrap_or_default();
        let modifiers = self
            .surface
            .as_ref()
            .map(|surface| surface.modifiers)
            .unwrap_or_default();

        if let Some(cmd) = self.keybinds.match_event(event, &modifiers) {
            let preserves_selection = matches!(cmd, AppCommand::Copy | AppCommand::SelectAll);
            if !preserves_selection
                && let Some(surface) = self.surface_mut()
                && surface.has_selection()
            {
                surface.clear_selection();
                surface.mark_dirty();
            }
            self.execute_app_command(event_loop, cmd);
            return;
        }

        let input = self.input.handle_key(event, &modifiers, modes);

        if let Some(surface) = self.surface_mut() {
            let mut selection_changed = false;
            if event.state == ElementState::Pressed
                && !matches!(input, VtInput::Ignore)
                && surface.has_selection()
            {
                surface.clear_selection();
                selection_changed = true;
            }
            if let VtInput::Write(bytes) = input {
                surface.write_to_pty(Bytes::from(bytes));
            }
            if selection_changed {
                surface.mark_dirty();
            }
        }
    }

    fn execute_app_command(&mut self, event_loop: &ActiveEventLoop, cmd: AppCommand) {
        match cmd {
            AppCommand::Quit | AppCommand::CloseWindow => {
                self.surface = None;
                event_loop.exit();
            }
            AppCommand::Copy => {
                if let Some(surface) = self.surface_mut() {
                    surface.copy_selection_to_clipboard();
                    surface.clear_selection();
                    surface.mark_dirty();
                }
            }
            AppCommand::Paste => {
                if let Some(surface) = self.surface_mut() {
                    surface.paste_from_clipboard();
                }
            }
            AppCommand::SelectAll => {
                if let Some(surface) = self.surface_mut() {
                    surface.select_all();
                    surface.sync_selection_to_overlay();
                    surface.mark_dirty();
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
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                let ch = surface.cell_size[1].max(1.0);
                (pos.y / f64::from(ch)) as i32
            }
        };
        if lines == 0 {
            return;
        }
        let modes = surface.terminal_modes();
        if let Some(data) = self.input.encode_mouse_wheel(lines, modes) {
            surface.write_to_pty(Bytes::from(data));
        } else {
            surface.scroll_lines(-lines);
        }
    }

    pub(crate) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        surface.mouse.cursor_pos = position;

        let modes = surface.terminal_modes();
        let shift_held = surface.modifiers.state().shift_key();
        // Forward motion to the PTY when an app has opted into motion-bearing
        // tracking modes (DECSET 1002 button-event or 1003 any-event), unless
        // the user is holding Shift to force local selection. The encoder
        // itself filters X10/Normal and the no-button-held case under 1002.
        if modes.mouse_tracking.reports_motion() && !shift_held {
            let input = MouseEventInput {
                action: MouseAction::Motion,
                button: None,
                position_px: clamp_position(position),
                any_button_pressed: surface.mouse.is_down,
                mods: surface.modifiers,
                size: surface.renderer.mouse_size(),
            };
            if let Some(data) = self.input.encode_mouse_event(input, modes) {
                surface.write_to_pty(Bytes::from(data));
            }
            return;
        }

        if !surface.mouse.is_down {
            return;
        }
        let (col, row) = surface.renderer.pixel_to_grid(position.x, position.y);
        surface.update_selection(col, row);
        surface.sync_selection_to_overlay();
        surface.mark_dirty();
    }

    pub(crate) fn on_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };

        let modes = surface.terminal_modes();
        let shift_held = surface.modifiers.state().shift_key();
        // Tracking active and Shift not held: forward to the PTY and skip
        // the local-selection path entirely. Shift+click/drag preserves
        // local selection so users can still copy text inside mouse-aware
        // apps (xterm/Ghostty convention).
        if modes.mouse_tracking.is_enabled() && !shift_held {
            let action = match state {
                ElementState::Pressed => MouseAction::Press,
                ElementState::Released => MouseAction::Release,
            };
            // Update `is_down` BEFORE encoding so that any-button-pressed
            // reflects the post-event state when reporting under DECSET
            // 1002 motion (xterm reports drag bytes carrying the held
            // button, and the very first motion after press needs the
            // bit set).
            surface.mouse.is_down = state == ElementState::Pressed;
            let input = MouseEventInput {
                action,
                button: Some(button),
                position_px: clamp_position(surface.mouse.cursor_pos),
                any_button_pressed: surface.mouse.is_down,
                mods: surface.modifiers,
                size: surface.renderer.mouse_size(),
            };
            if let Some(data) = self.input.encode_mouse_event(input, modes) {
                surface.write_to_pty(Bytes::from(data));
            }
            return;
        }

        // Tracking off (or Shift held): legacy local-selection path.
        // Right/middle remain unbound locally — match the old behavior.
        if button != MouseButton::Left {
            return;
        }
        match state {
            ElementState::Pressed => handle_mouse_press(surface),
            ElementState::Released => {
                surface.mouse.is_down = false;
                surface.copy_selection_to_clipboard();
            }
        }
    }
}

fn clamp_position(p: PhysicalPosition<f64>) -> (f32, f32) {
    (p.x.max(0.0) as f32, p.y.max(0.0) as f32)
}

fn handle_mouse_press(surface: &mut SurfaceState) {
    if surface.modifiers.state().super_key() {
        let _ = surface.window.drag_window();
        return;
    }
    let (col, row) = surface
        .renderer
        .pixel_to_grid(surface.mouse.cursor_pos.x, surface.mouse.cursor_pos.y);
    let clicks = surface.mouse.register_click(col, row);
    match clicks {
        1 => surface.start_selection(col, row),
        2 => surface.start_word_selection(col, row),
        3 => surface.start_line_selection(row),
        _ => {}
    }
    surface.sync_selection_to_overlay();
    surface.mouse.is_down = true;
    surface.mark_dirty();
}
