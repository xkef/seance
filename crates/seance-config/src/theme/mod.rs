//! Theme data + Ghostty-syntax theme loading.
//!
//! A theme is a palette (256 entries), background / foreground / cursor /
//! selection colors, and some optional specials. Theme files live alongside
//! `config.toml` but use Ghostty's config syntax (`key = value`) so seance
//! can ingest the ~486 themes Ghostty ships, byte-for-byte identical.
//!
//! Entry points:
//! - [`load`] — resolve the `theme` key from `config.toml` to a `Theme`.
//! - [`bundled`] — iterate or look up the embedded bundled themes.
//! - [`parse_source`] — parse a theme file's text into a `Theme`.

pub mod bundled;
pub mod parser;
pub mod resolve;

pub use parser::{ParseError, parse_source};
pub use resolve::{LoadError, ThemeSpec, load};

/// One theme — a palette plus window/cursor/selection colors.
///
/// Fields default to a neutral-ish placeholder (black background, light
/// foreground, no selection). In normal use you obtain a Theme via
/// [`load`], which always yields something from the bundled set.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Window background (RGBA; A is 0xff unless a theme overrides it).
    pub bg: [u8; 4],
    /// Default foreground for cells that don't specify their own color.
    pub fg: [u8; 3],
    /// Cursor fill color (RGBA). Alpha is currently always 0xff.
    pub cursor: [u8; 4],
    /// Text color drawn atop the cursor when the cursor covers a cell.
    /// `None` → the cell's resolved background (Ghostty's default behavior).
    pub cursor_text: Option<[u8; 3]>,
    /// Selection background (RGBA-float, blended over cells).
    pub selection_bg: [f32; 4],
    /// Explicit selection foreground; `None` → keep the cell's own fg.
    pub selection_fg: Option<[u8; 3]>,
    /// 256-entry palette (ANSI 0..16 + xterm 216 cube + 24-slot grayscale).
    pub palette: [[u8; 3]; 256],
}

impl Theme {
    /// Blank theme — xterm default palette, no styling. Intended for tests
    /// and as the last-ditch fallback when every normal resolution path
    /// fails. Not exposed via `Default` because production code should
    /// always route through [`load`].
    pub fn blank() -> Self {
        Self {
            bg: [0, 0, 0, 255],
            fg: [200, 200, 200],
            cursor: [200, 200, 200, 255],
            cursor_text: None,
            selection_bg: [0.3, 0.5, 0.8, 0.4],
            selection_fg: None,
            palette: default_xterm_palette(),
        }
    }
}

/// xterm 16 + 6×6×6 cube + 24-slot grayscale. Used as the starting palette
/// before a theme file's `palette =` lines overwrite individual entries.
pub(crate) fn default_xterm_palette() -> [[u8; 3]; 256] {
    let mut p = [[0u8; 3]; 256];
    // 16 ANSI base colors (standard xterm defaults).
    const ANSI: [[u8; 3]; 16] = [
        [0x00, 0x00, 0x00],
        [0xcd, 0x00, 0x00],
        [0x00, 0xcd, 0x00],
        [0xcd, 0xcd, 0x00],
        [0x00, 0x00, 0xee],
        [0xcd, 0x00, 0xcd],
        [0x00, 0xcd, 0xcd],
        [0xe5, 0xe5, 0xe5],
        [0x7f, 0x7f, 0x7f],
        [0xff, 0x00, 0x00],
        [0x00, 0xff, 0x00],
        [0xff, 0xff, 0x00],
        [0x5c, 0x5c, 0xff],
        [0xff, 0x00, 0xff],
        [0x00, 0xff, 0xff],
        [0xff, 0xff, 0xff],
    ];
    p[..16].copy_from_slice(&ANSI);
    let step = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                let idx = 16 + (r as usize) * 36 + (g as usize) * 6 + (b as usize);
                p[idx] = [step(r), step(g), step(b)];
            }
        }
    }
    for i in 0..24u8 {
        let v = 8 + 10 * i;
        p[232 + i as usize] = [v, v, v];
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_is_256_long_with_sane_ansi() {
        let p = default_xterm_palette();
        assert_eq!(p.len(), 256);
        assert_eq!(p[0], [0, 0, 0]);
        assert_eq!(p[15], [0xff, 0xff, 0xff]);
        // 6×6×6 cube starts at 16 with pure black.
        assert_eq!(p[16], [0, 0, 0]);
    }
}
