//! Bundled Ghostty themes, embedded into the binary at compile time.
//!
//! Sourced from `mbadolato/iTerm2-Color-Schemes/ghostty/` via
//! `tools/setup-themes.sh`, pinned to the same SHA ghostty itself uses so
//! `theme = "Foo"` resolves to identical bytes in seance and Ghostty.

use include_dir::{Dir, include_dir};

/// All bundled theme files, keyed by Title-Case filename (no extension).
pub static THEMES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../vendor/ghostty-themes");

/// Return the raw text of a bundled theme by its Title-Case name, if present.
pub fn get(name: &str) -> Option<&'static str> {
    THEMES.get_file(name)?.contents_utf8()
}

/// Iterate (name, text) pairs for every bundled theme.
pub fn iter() -> impl Iterator<Item = (&'static str, &'static str)> {
    THEMES.files().filter_map(|f| {
        let name = f.path().file_name()?.to_str()?;
        let text = f.contents_utf8()?;
        Some((name, text))
    })
}

/// Number of bundled themes. Used in tests and diagnostics.
pub fn count() -> usize {
    THEMES.files().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catppuccin_frappe_is_present() {
        let text = get("Catppuccin Frappe").expect("bundled Catppuccin Frappe missing");
        assert!(text.contains("palette = 0="));
        assert!(text.contains("background = #"));
    }

    #[test]
    fn bundled_count_is_in_expected_range() {
        // Guards against an empty vendor dir (setup script didn't run) or
        // accidentally shipping the entire upstream repo.
        let n = count();
        assert!(
            n >= 100,
            "only {n} bundled themes — run tools/setup-themes.sh"
        );
        assert!(n <= 2000, "{n} bundled themes — vendor dir looks wrong");
    }
}
