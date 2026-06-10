//! Keyboard and mouse input handling.
//!
//! Translates winit events into VT escape sequences via
//! libghostty-vt's key and mouse encoders. App-level shortcuts (Cmd+Q,
//! clipboard, font size) are matched upstream before reaching here.

mod keymap;
#[cfg(target_os = "macos")]
mod uckey;

use libghostty_vt::{key, mouse};
use seance_protocol::frame::{MouseSize, MouseTracking, TerminalModes};
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton};
use winit::keyboard::PhysicalKey;

/// Result of encoding a VT-bound key event.
#[derive(Debug)]
pub enum VtInput {
    /// Raw bytes to write to the PTY.
    Write(Vec<u8>),
    /// The event produced nothing to forward.
    Ignore,
}

/// macOS "option-as-alt" policy.
///
/// On macOS, Option serves double duty: it's both the VT Alt modifier
/// (readline `Alt+f`/`Alt+b`, vim `<M-…>`) and the OS text composer
/// (`Opt+o` → `ø`). This enum picks which role Option plays per side.
/// Ignored on non-macOS — Alt is always Alt there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OptionAsAlt {
    /// Both Option keys compose macOS special characters. macOS-friendly
    /// default — preserves `ø`/`¬`/`–` input.
    #[default]
    None,
    /// Only left-Option sends ESC-prefix; right-Option still composes.
    Left,
    /// Only right-Option sends ESC-prefix; left-Option still composes.
    Right,
    /// Both Option keys send ESC-prefix. Breaks macOS text composition.
    Both,
}

impl OptionAsAlt {
    fn to_libghostty(self) -> key::OptionAsAlt {
        match self {
            Self::None => key::OptionAsAlt::False,
            Self::Left => key::OptionAsAlt::Left,
            Self::Right => key::OptionAsAlt::Right,
            Self::Both => key::OptionAsAlt::True,
        }
    }
}

/// Returns true when an Option-modified press should resolve through the
/// macOS text composer (and therefore through `UCKeyTranslate` for dead
/// keys) rather than emit an ESC-prefix. Composer side is the *opposite*
/// of whatever side the configured policy treats as Alt.
fn composer_side(policy: OptionAsAlt, alt_held: bool, right_option_held: bool) -> bool {
    if !alt_held {
        return false;
    }
    match policy {
        OptionAsAlt::None => true,
        OptionAsAlt::Left => right_option_held,
        OptionAsAlt::Right => !right_option_held,
        OptionAsAlt::Both => false,
    }
}

/// Press / Release / Motion classification for a mouse event the app
/// wants forwarded to the PTY. Mirrors libghostty-vt's `mouse::Action`
/// without forcing callers to import the libghostty type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// Inputs to `InputHandler::encode_mouse_event`. Pure-data record so
/// callers don't have to talk to libghostty-vt directly.
#[derive(Debug, Clone, Copy)]
pub struct MouseEventInput {
    pub action: MouseAction,
    /// `None` is permitted only for `Action::Motion` under any-event
    /// tracking, where there is no button to report.
    pub button: Option<MouseButton>,
    /// Surface-space pixel coordinates of the cursor. The encoder
    /// converts these to cell coordinates internally using `size`.
    pub position_px: (f32, f32),
    /// Whether at least one mouse button is currently held. Used by
    /// the encoder to distinguish motion-with-drag from pure motion
    /// under DECSET 1002.
    pub any_button_pressed: bool,
    pub mods: Modifiers,
    pub size: MouseSize,
}

fn tracking_mode_to_libghostty(t: MouseTracking) -> mouse::TrackingMode {
    match t {
        MouseTracking::None => mouse::TrackingMode::None,
        MouseTracking::X10 => mouse::TrackingMode::X10,
        MouseTracking::Normal => mouse::TrackingMode::Normal,
        MouseTracking::Button => mouse::TrackingMode::Button,
        MouseTracking::Any => mouse::TrackingMode::Any,
    }
}

fn mouse_action_to_libghostty(a: MouseAction) -> mouse::Action {
    match a {
        MouseAction::Press => mouse::Action::Press,
        MouseAction::Release => mouse::Action::Release,
        MouseAction::Motion => mouse::Action::Motion,
    }
}

fn map_mouse_button(button: MouseButton) -> mouse::Button {
    match button {
        MouseButton::Left => mouse::Button::Left,
        MouseButton::Right => mouse::Button::Right,
        MouseButton::Middle => mouse::Button::Middle,
        // winit "Back"/"Forward" are the side buttons; xterm conventionally
        // assigns them as buttons 8 and 9.
        MouseButton::Back => mouse::Button::Eight,
        MouseButton::Forward => mouse::Button::Nine,
        MouseButton::Other(_) => mouse::Button::Unknown,
    }
}

/// Translates winit events into VT bytes.
pub struct InputHandler {
    key_encoder: key::Encoder<'static>,
    mouse_encoder: mouse::Encoder<'static>,
    /// Last size pushed to `mouse_encoder` via `set_size`. Tracked here
    /// because the libghostty wrapper resets the encoder's last-cell
    /// motion dedup state on every `set_size` call — even when the
    /// value is unchanged — so we must skip the call when nothing has
    /// actually changed.
    mouse_size: Option<MouseSize>,
    option_as_alt: OptionAsAlt,
    #[cfg(target_os = "macos")]
    uckey: uckey::UcKey,
}

impl Default for InputHandler {
    fn default() -> Self {
        let mut key_encoder = key::Encoder::new().expect("key encoder");
        // Enable DEC mode 1036 so ALT+<char> produces `ESC <char>` (matches
        // xterm/Ghostty defaults). Without this, the encoder drops the ALT
        // bit and just emits the un-prefixed character.
        key_encoder.set_alt_esc_prefix(true);
        let mut mouse_encoder = mouse::Encoder::new().expect("mouse encoder");
        // Dedup motion to one report per cell change. Without this, every
        // sub-pixel cursor jitter during a held button forwards a motion
        // byte; tmux then reads `Press → Motion → Release` for what was a
        // simple click and counts it as a drag, breaking the multi-click
        // detector that drives `TripleClick1Pane` / copy-mode entry.
        mouse_encoder.set_track_last_cell(true);
        Self {
            key_encoder,
            mouse_encoder,
            mouse_size: None,
            option_as_alt: OptionAsAlt::default(),
            #[cfg(target_os = "macos")]
            uckey: uckey::UcKey::new(),
        }
    }
}

impl InputHandler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the macOS option-as-alt policy. The encoder uses this (together
    /// with the `ALT_SIDE` bit on each event's mods) to decide whether to
    /// emit `ESC`-prefix for Option+key or pass the composed text through.
    pub fn set_option_as_alt(&mut self, mode: OptionAsAlt) {
        self.option_as_alt = mode;
        self.key_encoder
            .set_macos_option_as_alt(mode.to_libghostty());
    }

    /// Encode a key event as VT bytes (cursor keys, function keys, etc.).
    pub fn handle_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &Modifiers,
        modes: TerminalModes,
    ) -> VtInput {
        if event.state != ElementState::Pressed {
            return VtInput::Ignore;
        }
        let bytes = self.encode_key(event, modifiers, modes);
        if bytes.is_empty() {
            VtInput::Ignore
        } else {
            VtInput::Write(bytes)
        }
    }

    /// Encode a mouse wheel event as VT mouse sequences. Returns
    /// `None` when mouse tracking is off (caller should scroll the
    /// viewport locally instead).
    pub fn encode_mouse_wheel(&mut self, lines: i32, modes: TerminalModes) -> Option<Vec<u8>> {
        if !modes.mouse_tracking.is_enabled() {
            return None;
        }
        // Wheel encodes as buttons 4/5 in all xterm tracking modes; the
        // sub-mode does not gate it.
        self.mouse_encoder
            .set_tracking_mode(mouse::TrackingMode::Normal);
        self.mouse_encoder.set_format(if modes.mouse_format_sgr {
            mouse::Format::Sgr
        } else {
            mouse::Format::X10
        });

        let button = if lines > 0 {
            mouse::Button::Four
        } else {
            mouse::Button::Five
        };
        let mut out = Vec::new();
        for _ in 0..lines.unsigned_abs() {
            let mut event = mouse::Event::new().ok()?;
            event
                .set_action(mouse::Action::Press)
                .set_button(Some(button))
                .set_position(mouse::Position { x: 0.0, y: 0.0 });
            self.mouse_encoder.encode_to_vec(&event, &mut out).ok()?;
        }
        if out.is_empty() { None } else { Some(out) }
    }

    /// Encode wheel scrolling in the alternate screen as Up/Down arrow
    /// sequences when DECSET 1007 (alternate scroll) is active. Returns
    /// `None` when the alt-screen isn't active, 1007 is off, or `lines`
    /// is zero — the caller falls back to local viewport scrolling, which
    /// is itself a no-op in the alt-screen.
    pub fn encode_alt_scroll(&self, lines: i32, modes: TerminalModes) -> Option<Vec<u8>> {
        if !modes.alt_screen || !modes.alt_scroll || lines == 0 {
            return None;
        }
        let seq: &[u8] = match (modes.cursor_keys, lines > 0) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1bOB",
            (false, true) => b"\x1b[A",
            (false, false) => b"\x1b[B",
        };
        let count = lines.unsigned_abs() as usize;
        let mut out = Vec::with_capacity(seq.len() * count);
        for _ in 0..count {
            out.extend_from_slice(seq);
        }
        Some(out)
    }

    /// Encode a mouse button or motion event as VT mouse sequences. Returns
    /// `None` when mouse tracking is off, or when the event isn't reportable
    /// under the active sub-mode (motion under X10/Normal, motion without a
    /// button held under DECSET 1002).
    pub fn encode_mouse_event(
        &mut self,
        input: MouseEventInput,
        modes: TerminalModes,
    ) -> Option<Vec<u8>> {
        if !modes.mouse_tracking.is_enabled() {
            return None;
        }
        if input.action == MouseAction::Motion {
            // X10 and Normal never report motion. Button-event reports
            // motion only while at least one button is held. Any-event
            // always reports motion.
            if !modes.mouse_tracking.reports_motion() {
                return None;
            }
            if !modes.mouse_tracking.reports_motion_without_button() && !input.any_button_pressed {
                return None;
            }
        }

        self.mouse_encoder
            .set_tracking_mode(tracking_mode_to_libghostty(modes.mouse_tracking));
        self.mouse_encoder.set_format(if modes.mouse_format_sgr {
            mouse::Format::Sgr
        } else {
            mouse::Format::X10
        });
        if self.mouse_size != Some(input.size) {
            self.mouse_encoder.set_size(mouse::EncoderSize {
                screen_width: input.size.screen_width,
                screen_height: input.size.screen_height,
                cell_width: input.size.cell_width.max(1),
                cell_height: input.size.cell_height.max(1),
                padding_top: input.size.padding_top,
                padding_bottom: input.size.padding_bottom,
                padding_left: input.size.padding_left,
                padding_right: input.size.padding_right,
            });
            self.mouse_size = Some(input.size);
        }
        self.mouse_encoder
            .set_any_button_pressed(input.any_button_pressed);

        let mut event = mouse::Event::new().ok()?;
        event
            .set_action(mouse_action_to_libghostty(input.action))
            .set_button(input.button.map(map_mouse_button))
            .set_mods(keymap::map_mods(&input.mods))
            .set_position(mouse::Position {
                x: input.position_px.0,
                y: input.position_px.1,
            });

        let mut out = Vec::new();
        self.mouse_encoder.encode_to_vec(&event, &mut out).ok()?;
        if out.is_empty() { None } else { Some(out) }
    }

    fn encode_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &Modifiers,
        modes: TerminalModes,
    ) -> Vec<u8> {
        self.key_encoder
            .set_cursor_key_application(modes.cursor_keys);

        let PhysicalKey::Code(code) = event.physical_key else {
            return Vec::new();
        };
        let Some(gk) = keymap::map_keycode(code) else {
            return Vec::new();
        };
        let Ok(mut key_event) = key::Event::new() else {
            return Vec::new();
        };

        #[cfg(target_os = "macos")]
        let composed = {
            let state = modifiers.state();
            let alt_held = state.alt_key();
            let right = uckey::right_option_held(modifiers);
            if composer_side(self.option_as_alt, alt_held, right) {
                self.uckey.translate(code, modifiers)
            } else if !alt_held
                && !state.control_key()
                && !state.super_key()
                && event.text.as_deref().is_none_or(str::is_empty)
            {
                // Dead-key recovery: bare `^` and Shift+`^` (grave) on ISO
                // layouts come through winit with empty text because
                // NSTextInputClient buffers the composition. UCKeyTranslate
                // with the no-dead-keys mask resolves the glyph synchronously.
                self.uckey.translate(code, modifiers)
            } else {
                None
            }
        };
        #[cfg(not(target_os = "macos"))]
        let composed: Option<String> = None;

        key_event
            .set_key(gk)
            .set_action(keymap::map_action(event.state))
            .set_mods(keymap::map_mods(modifiers));
        let utf8 = composed
            .as_deref()
            .or(event.text.as_deref())
            .filter(|t| is_safe_encoder_text(t));
        if let Some(text) = utf8 {
            key_event.set_utf8(Some(text));
        }

        let mut buf = Vec::new();
        let _ = self.key_encoder.encode_to_vec(&key_event, &mut buf);

        if tracing::enabled!(tracing::Level::TRACE) {
            tracing::trace!(
                "encode_key: code={code:?} text={:?} composed={:?} mods={:?} -> {buf:02x?}",
                event.text.as_deref(),
                composed.as_deref(),
                key_event.mods(),
            );
        }

        buf
    }
}

// `libghostty-vt::Event::set_utf8` requires the unmodified character before
// any Ctrl/Meta transformations and explicitly forbids C0 controls
// (U+0000-U+001F, U+007F) and macOS PUA function-key codes (U+F700-U+F8FF).
// winit puts those codepoints in `event.text` for keys like Enter (`\r`),
// Tab (`\t`), Backspace, and the macOS arrow / function keys — passing
// them through leaves the encoder in an undefined state. Drop the text on
// a forbidden codepoint and let the encoder fall back to the logical-key
// path, which produces the documented VT sequence.
fn is_safe_encoder_text(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c <= '\u{001F}' || c == '\u{007F}' || ('\u{F700}'..='\u{F8FF}').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> MouseSize {
        MouseSize {
            screen_width: 800,
            screen_height: 600,
            cell_width: 10,
            cell_height: 20,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
        }
    }

    fn modes(tracking: MouseTracking, sgr: bool) -> TerminalModes {
        TerminalModes {
            cursor_keys: false,
            mouse_tracking: tracking,
            mouse_format_sgr: sgr,
            bracketed_paste: false,
            alt_screen: false,
            alt_scroll: false,
        }
    }

    fn alt_scroll_modes(alt_screen: bool, alt_scroll: bool, cursor_keys: bool) -> TerminalModes {
        TerminalModes {
            cursor_keys,
            mouse_tracking: MouseTracking::None,
            mouse_format_sgr: false,
            bracketed_paste: false,
            alt_screen,
            alt_scroll,
        }
    }

    fn input(
        action: MouseAction,
        button: Option<MouseButton>,
        any_pressed: bool,
    ) -> MouseEventInput {
        MouseEventInput {
            action,
            button,
            position_px: (0.0, 0.0),
            any_button_pressed: any_pressed,
            mods: Modifiers::default(),
            size: size(),
        }
    }

    #[test]
    fn tracking_disabled_returns_none() {
        let mut h = InputHandler::new();
        let out = h.encode_mouse_event(
            input(MouseAction::Press, Some(MouseButton::Left), true),
            modes(MouseTracking::None, true),
        );
        assert!(out.is_none());
    }

    #[test]
    fn left_press_under_sgr_emits_sgr_csi() {
        let mut h = InputHandler::new();
        let out = h
            .encode_mouse_event(
                input(MouseAction::Press, Some(MouseButton::Left), true),
                modes(MouseTracking::Normal, true),
            )
            .expect("press should encode under Normal+SGR");
        // SGR mouse press starts ESC [ < and ends with M.
        assert_eq!(&out[..3], b"\x1b[<");
        assert_eq!(*out.last().unwrap(), b'M');
    }

    #[test]
    fn left_release_under_sgr_emits_lowercase_m() {
        let mut h = InputHandler::new();
        let out = h
            .encode_mouse_event(
                input(MouseAction::Release, Some(MouseButton::Left), false),
                modes(MouseTracking::Normal, true),
            )
            .expect("release should encode under Normal+SGR");
        assert_eq!(&out[..3], b"\x1b[<");
        assert_eq!(*out.last().unwrap(), b'm');
    }

    #[test]
    fn motion_under_normal_returns_none() {
        let mut h = InputHandler::new();
        let out = h.encode_mouse_event(
            input(MouseAction::Motion, None, true),
            modes(MouseTracking::Normal, true),
        );
        assert!(out.is_none());
    }

    #[test]
    fn motion_under_button_without_press_returns_none() {
        let mut h = InputHandler::new();
        let out = h.encode_mouse_event(
            input(MouseAction::Motion, None, false),
            modes(MouseTracking::Button, true),
        );
        assert!(out.is_none());
    }

    #[test]
    fn motion_under_button_with_press_encodes() {
        let mut h = InputHandler::new();
        let out = h.encode_mouse_event(
            input(MouseAction::Motion, Some(MouseButton::Left), true),
            modes(MouseTracking::Button, true),
        );
        assert!(out.is_some());
    }

    #[test]
    fn motion_at_same_cell_dedupes() {
        let mut h = InputHandler::new();
        let m = modes(MouseTracking::Button, true);
        // Seed the encoder's last_cell with a Press at (0,0).
        h.encode_mouse_event(input(MouseAction::Press, Some(MouseButton::Left), true), m)
            .expect("press should encode and seed last_cell");
        // A subsequent Motion at the same cell must not emit bytes —
        // tmux's multi-click detector breaks if Press → Motion → Release
        // arrives for every click of a triple-click.
        let dup =
            h.encode_mouse_event(input(MouseAction::Motion, Some(MouseButton::Left), true), m);
        assert!(dup.is_none(), "motion at the same cell should dedup");
    }

    #[test]
    fn motion_under_any_without_press_encodes() {
        let mut h = InputHandler::new();
        let out = h.encode_mouse_event(
            input(MouseAction::Motion, None, false),
            modes(MouseTracking::Any, true),
        );
        assert!(out.is_some());
    }

    #[test]
    fn wheel_disabled_when_tracking_none() {
        let mut h = InputHandler::new();
        assert!(
            h.encode_mouse_wheel(1, modes(MouseTracking::None, true))
                .is_none()
        );
    }

    #[test]
    fn wheel_enabled_under_any_tracking_mode() {
        let mut h = InputHandler::new();
        for t in [
            MouseTracking::X10,
            MouseTracking::Normal,
            MouseTracking::Button,
            MouseTracking::Any,
        ] {
            assert!(
                h.encode_mouse_wheel(1, modes(t, true)).is_some(),
                "wheel should encode under {t:?}",
            );
        }
    }

    #[test]
    fn safe_text_filter_rejects_libghostty_forbidden_codepoints() {
        for c in ['\r', '\t', '\u{0008}', '\u{001B}', '\u{0000}', '\u{001F}'] {
            let s = c.to_string();
            assert!(
                !is_safe_encoder_text(&s),
                "C0 control {c:?} should be filtered"
            );
        }
        assert!(!is_safe_encoder_text("\u{007F}"), "DEL should be filtered");

        for c in ['\u{F700}', '\u{F701}', '\u{F702}', '\u{F703}', '\u{F8FF}'] {
            let s = c.to_string();
            assert!(
                !is_safe_encoder_text(&s),
                "macOS PUA {c:?} should be filtered"
            );
        }

        assert!(!is_safe_encoder_text("a\u{F700}"));
        assert!(!is_safe_encoder_text("\r\n"));
    }

    #[test]
    fn safe_text_filter_passes_real_text() {
        for s in ["a", "Z", "1", " ", "ø", "é", "ab", "你好", "😀"] {
            assert!(is_safe_encoder_text(s), "{s:?} should pass through");
        }
        assert!(is_safe_encoder_text("\u{0020}"));
        assert!(is_safe_encoder_text("\u{F6FF}"));
        assert!(is_safe_encoder_text("\u{F900}"));
    }

    #[test]
    fn alt_scroll_disabled_when_not_alt_screen() {
        let h = InputHandler::new();
        assert!(
            h.encode_alt_scroll(3, alt_scroll_modes(false, true, false))
                .is_none()
        );
    }

    #[test]
    fn alt_scroll_disabled_when_1007_off() {
        let h = InputHandler::new();
        assert!(
            h.encode_alt_scroll(3, alt_scroll_modes(true, false, false))
                .is_none()
        );
    }

    #[test]
    fn alt_scroll_csi_up_under_normal_cursor_keys() {
        let h = InputHandler::new();
        let out = h
            .encode_alt_scroll(3, alt_scroll_modes(true, true, false))
            .expect("alt-scroll should emit CSI up");
        assert_eq!(out, b"\x1b[A\x1b[A\x1b[A");
    }

    #[test]
    fn alt_scroll_csi_down_under_normal_cursor_keys() {
        let h = InputHandler::new();
        let out = h
            .encode_alt_scroll(-2, alt_scroll_modes(true, true, false))
            .expect("alt-scroll should emit CSI down");
        assert_eq!(out, b"\x1b[B\x1b[B");
    }

    #[test]
    fn alt_scroll_ss3_under_application_cursor_keys() {
        let h = InputHandler::new();
        let out = h
            .encode_alt_scroll(1, alt_scroll_modes(true, true, true))
            .expect("alt-scroll should emit SS3 up under DECCKM");
        assert_eq!(out, b"\x1bOA");
    }

    #[test]
    fn alt_scroll_zero_lines_returns_none() {
        let h = InputHandler::new();
        assert!(
            h.encode_alt_scroll(0, alt_scroll_modes(true, true, false))
                .is_none()
        );
    }

    #[test]
    fn composer_side_short_circuits_without_alt() {
        for policy in [
            OptionAsAlt::None,
            OptionAsAlt::Left,
            OptionAsAlt::Right,
            OptionAsAlt::Both,
        ] {
            assert!(!composer_side(policy, false, false));
            assert!(!composer_side(policy, false, true));
        }
    }

    #[test]
    fn composer_side_policy_none_always_composes_when_alt_held() {
        assert!(composer_side(OptionAsAlt::None, true, false));
        assert!(composer_side(OptionAsAlt::None, true, true));
    }

    #[test]
    fn composer_side_policy_both_never_composes() {
        assert!(!composer_side(OptionAsAlt::Both, true, false));
        assert!(!composer_side(OptionAsAlt::Both, true, true));
    }

    #[test]
    fn composer_side_policy_left_composes_only_when_right_held() {
        assert!(!composer_side(OptionAsAlt::Left, true, false));
        assert!(composer_side(OptionAsAlt::Left, true, true));
    }

    #[test]
    fn composer_side_policy_right_composes_only_when_left_held() {
        assert!(composer_side(OptionAsAlt::Right, true, false));
        assert!(!composer_side(OptionAsAlt::Right, true, true));
    }
}
