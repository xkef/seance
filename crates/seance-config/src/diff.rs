//! Config diff used by the hot-reload path (#13) to decide what to
//! invalidate when the file on disk changes.
//!
//! Kept in this crate (rather than in `seance-app`) because it is a pure
//! function of two [`Config`] values — no winit / wgpu dependency, so it is
//! unit-testable in isolation.

use crate::Config;

/// Which subsystems a [`Config`] delta requires us to refresh.
///
/// Constructed via [`ConfigDiff::between`]. A freshly-parsed config equal to
/// the old one produces an all-false diff, letting the caller skip work.
///
/// `font_family_changed` is surfaced separately from `font_size_changed`
/// because changing the family requires rebuilding the whole font backend
/// (not supported by the live renderer today — the app logs a notice telling
/// the user to restart).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConfigDiff {
    /// `theme =` key changed — re-resolve and swap the palette.
    pub theme_changed: bool,
    /// `font.size` changed — `TerminalRenderer::set_font_size` handles this
    /// (clears glyph atlas, recomputes cell metrics).
    pub font_size_changed: bool,
    /// `font.family` changed — live reload not yet implemented; caller logs
    /// a notice.
    pub font_family_changed: bool,
    /// Cursor/padding/opacity/mouse-hide changed — a plain repaint is enough
    /// (downstream consumers read `Config` directly on the next frame).
    pub repaint_only: bool,
}

impl ConfigDiff {
    /// Compute what changed between `old` and `new`.
    pub fn between(old: &Config, new: &Config) -> Self {
        let theme_changed = old.theme != new.theme;
        let font_size_changed = old.font.size != new.font.size;
        let font_family_changed = old.font.family != new.font.family;

        // Fields whose consumers will pick up changes on the next paint.
        // Grouped together so we can request a single redraw if any of them
        // moved — we don't need a more granular signal than that.
        let repaint_only = old.window.padding_x != new.window.padding_x
            || old.window.padding_y != new.window.padding_y
            || old.window.background_opacity != new.window.background_opacity
            || old.cursor.style != new.cursor.style
            || old.cursor.blink != new.cursor.blink
            || old.mouse.hide_while_typing != new.mouse.hide_while_typing;

        Self {
            theme_changed,
            font_size_changed,
            font_family_changed,
            repaint_only,
        }
    }

    /// True when nothing the renderer / app cares about changed.
    pub fn is_empty(&self) -> bool {
        !(self.theme_changed
            || self.font_size_changed
            || self.font_family_changed
            || self.repaint_only)
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::CursorStyle;

    #[test]
    fn identical_configs_yield_empty_diff() {
        let a = Config::default();
        let b = Config::default();
        assert!(ConfigDiff::between(&a, &b).is_empty());
    }

    #[test]
    fn theme_change_is_detected() {
        let a = Config::default();
        let mut b = Config::default();
        b.theme = Some("Gruvbox Dark".to_string());
        let d = ConfigDiff::between(&a, &b);
        assert!(d.theme_changed);
        assert!(!d.font_size_changed);
        assert!(!d.repaint_only);
    }

    #[test]
    fn font_size_change_is_detected() {
        let a = Config::default();
        let mut b = Config::default();
        b.font.size = 18.0;
        let d = ConfigDiff::between(&a, &b);
        assert!(d.font_size_changed);
        assert!(!d.font_family_changed);
        assert!(!d.theme_changed);
    }

    #[test]
    fn font_family_change_is_separate_from_size() {
        let a = Config::default();
        let mut b = Config::default();
        b.font.family = "Berkeley Mono".to_string();
        let d = ConfigDiff::between(&a, &b);
        assert!(d.font_family_changed);
        assert!(!d.font_size_changed);
    }

    #[test]
    fn cursor_style_change_is_repaint_only() {
        let a = Config::default();
        let mut b = Config::default();
        b.cursor.style = CursorStyle::Bar;
        let d = ConfigDiff::between(&a, &b);
        assert!(d.repaint_only);
        assert!(!d.theme_changed);
        assert!(!d.font_size_changed);
    }

    #[test]
    fn padding_change_is_repaint_only() {
        let a = Config::default();
        let mut b = Config::default();
        b.window.padding_x = 12;
        let d = ConfigDiff::between(&a, &b);
        assert!(d.repaint_only);
    }

    #[test]
    fn multiple_fields_set_multiple_flags() {
        let a = Config::default();
        let mut b = Config::default();
        b.theme = Some("Gruvbox Dark".to_string());
        b.font.size = 20.0;
        b.window.padding_y = 8;
        let d = ConfigDiff::between(&a, &b);
        assert!(d.theme_changed);
        assert!(d.font_size_changed);
        assert!(d.repaint_only);
        assert!(!d.is_empty());
    }
}
