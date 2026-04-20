//! Parser for Ghostty's theme-file subset.
//!
//! Theme files are plain text, one `key = value` per line, `#` comments.
//! Ghostty permits a broader config grammar (conditionals, lists) but
//! theme files in the wild — including all 486 bundled themes — use only
//! the keys enumerated in [`Key`]. Unknown keys are skipped with a warn
//! log so unexpected input degrades rather than erroring.
//!
//! Color values are `#RRGGBB` or `#RGB` hex. The bundled set uses
//! `#RRGGBB` exclusively; X11 color names and the `cell-foreground` /
//! `cell-background` specials are accepted by Ghostty but do not appear
//! in the bundled files, so they are rejected here with a logged warning.

use super::{Theme, default_xterm_palette};

/// Error returned when a theme source cannot be parsed. Callers usually
/// log and fall back to a different theme rather than propagating.
#[derive(Debug, Clone)]
pub enum ParseError {
    Line { line: usize, kind: LineError },
}

#[derive(Debug, Clone)]
pub enum LineError {
    MissingEquals,
    EmptyKey,
    PaletteIndex(String),
    PaletteNoEquals,
    BadColor(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Line { line, kind } => write!(f, "line {line}: {kind}"),
        }
    }
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LineError::MissingEquals => write!(f, "missing '=' separator"),
            LineError::EmptyKey => write!(f, "empty key"),
            LineError::PaletteIndex(s) => write!(f, "palette index '{s}' is not 0..=255"),
            LineError::PaletteNoEquals => {
                write!(f, "palette value missing '=' (expected `N=#RRGGBB`)")
            }
            LineError::BadColor(s) => write!(f, "could not parse color '{s}'"),
        }
    }
}

impl std::error::Error for ParseError {}
impl std::error::Error for LineError {}

/// Parse a theme file's text into a [`Theme`]. Unknown keys are logged
/// but do not fail the parse; malformed color values do.
pub fn parse_source(text: &str) -> Result<Theme, ParseError> {
    let mut theme = Theme::blank();
    theme.palette = default_xterm_palette();

    for (line_idx, raw) in text.lines().enumerate() {
        let line_no = line_idx + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => {
                return Err(ParseError::Line {
                    line: line_no,
                    kind: LineError::MissingEquals,
                });
            }
        };
        if key.is_empty() {
            return Err(ParseError::Line {
                line: line_no,
                kind: LineError::EmptyKey,
            });
        }

        match key {
            "palette" => apply_palette(&mut theme, value, line_no)?,
            "background" => theme.bg = rgba(parse_color(value, line_no)?),
            "foreground" => theme.fg = parse_color(value, line_no)?,
            "cursor-color" => theme.cursor = rgba(parse_color(value, line_no)?),
            "cursor-text" => theme.cursor_text = Some(parse_color(value, line_no)?),
            "selection-background" => {
                theme.selection_bg = rgba_f32(parse_color(value, line_no)?, 0.4);
            }
            "selection-foreground" => theme.selection_fg = Some(parse_color(value, line_no)?),
            // Ghostty accepts these but they never appear in bundled files
            // and we have no implementation yet. Log and skip.
            "palette-generate" | "palette-harmonious" => {
                log::debug!("theme: ignoring `{key} = {value}` (not implemented)");
            }
            // Silently ignored per Ghostty parity: theme files cannot set
            // the active theme or pull in more config files.
            "theme" | "config-file" => {}
            _ => {
                log::warn!("theme: unknown key `{key}` on line {line_no}, skipping");
            }
        }
    }

    Ok(theme)
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        // `#` only introduces a comment when it's the *first* non-whitespace
        // character. Color values like `#RRGGBB` must not be split.
        Some(idx) if line[..idx].trim().is_empty() => "",
        _ => line,
    }
}

fn apply_palette(theme: &mut Theme, value: &str, line: usize) -> Result<(), ParseError> {
    let (idx_str, color_str) = value.split_once('=').ok_or(ParseError::Line {
        line,
        kind: LineError::PaletteNoEquals,
    })?;
    let idx: usize = idx_str.trim().parse().map_err(|_| ParseError::Line {
        line,
        kind: LineError::PaletteIndex(idx_str.trim().to_string()),
    })?;
    if idx > 255 {
        return Err(ParseError::Line {
            line,
            kind: LineError::PaletteIndex(idx_str.trim().to_string()),
        });
    }
    theme.palette[idx] = parse_color(color_str.trim(), line)?;
    Ok(())
}

fn parse_color(value: &str, line: usize) -> Result<[u8; 3], ParseError> {
    parse_hex(value).ok_or_else(|| ParseError::Line {
        line,
        kind: LineError::BadColor(value.to_string()),
    })
}

fn parse_hex(value: &str) -> Option<[u8; 3]> {
    let s = value.strip_prefix('#').unwrap_or(value);
    match s.len() {
        6 => Some([
            u8::from_str_radix(s.get(0..2)?, 16).ok()?,
            u8::from_str_radix(s.get(2..4)?, 16).ok()?,
            u8::from_str_radix(s.get(4..6)?, 16).ok()?,
        ]),
        3 => {
            let r = u8::from_str_radix(s.get(0..1)?, 16).ok()?;
            let g = u8::from_str_radix(s.get(1..2)?, 16).ok()?;
            let b = u8::from_str_radix(s.get(2..3)?, 16).ok()?;
            Some([r * 0x11, g * 0x11, b * 0x11])
        }
        _ => None,
    }
}

fn rgba([r, g, b]: [u8; 3]) -> [u8; 4] {
    [r, g, b, 0xff]
}

fn rgba_f32([r, g, b]: [u8; 3], alpha: f32) -> [f32; 4] {
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        alpha,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_theme() {
        let text = "\
            palette = 0=#000000\n\
            palette = 15=#ffffff\n\
            background = #112233\n\
            foreground = #abcdef\n\
            cursor-color = #ff00aa\n\
            # inline comment line\n\
        ";
        let t = parse_source(text).unwrap();
        assert_eq!(t.palette[0], [0, 0, 0]);
        assert_eq!(t.palette[15], [0xff, 0xff, 0xff]);
        assert_eq!(t.bg, [0x11, 0x22, 0x33, 0xff]);
        assert_eq!(t.fg, [0xab, 0xcd, 0xef]);
        assert_eq!(t.cursor, [0xff, 0x00, 0xaa, 0xff]);
    }

    #[test]
    fn short_hex_expands() {
        let t = parse_source("foreground = #abc\n").unwrap();
        assert_eq!(t.fg, [0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn selection_fg_and_cursor_text_are_optional() {
        let t = parse_source(
            "\
            selection-background = #334455\n\
            selection-foreground = #ffeedd\n\
            cursor-text = #010203\n\
        ",
        )
        .unwrap();
        assert!(t.selection_fg.is_some());
        assert_eq!(t.selection_fg.unwrap(), [0xff, 0xee, 0xdd]);
        assert_eq!(t.cursor_text, Some([0x01, 0x02, 0x03]));
        // alpha on selection stays at the hardcoded 0.4 blend default.
        assert!((t.selection_bg[3] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn unknown_key_is_tolerated() {
        let t = parse_source("nonsense = 42\nbackground = #101010\n").unwrap();
        assert_eq!(t.bg, [0x10, 0x10, 0x10, 0xff]);
    }

    #[test]
    fn bad_color_errors() {
        let err = parse_source("background = notahex\n").unwrap_err();
        assert!(format!("{err}").contains("notahex"));
    }

    #[test]
    fn palette_out_of_range_errors() {
        let err = parse_source("palette = 256=#000000\n").unwrap_err();
        assert!(format!("{err}").contains("256"));
    }

    #[test]
    fn comments_and_blanks_are_skipped() {
        let t = parse_source("\n  \n# header comment\nbackground = #808080\n").unwrap();
        assert_eq!(t.bg, [0x80, 0x80, 0x80, 0xff]);
    }
}
