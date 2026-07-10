mod default_url;

use fancy_regex::Regex;
use seance_protocol::frame::{GridPos, VtSnapshot};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkModifiers {
    pub super_key: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl LinkModifiers {
    pub fn matches(self, actual: Self) -> bool {
        (!self.super_key || actual.super_key)
            && (!self.ctrl || actual.ctrl)
            && (!self.alt || actual.alt)
            && (!self.shift || actual.shift)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAction {
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkHighlight {
    Hover,
    HoverMods,
    Always,
    AlwaysMods,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSource {
    Osc8,
    Rule { index: usize },
    DefaultUrlPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    Path(String),
}

/// A filesystem location parsed from a link target, carrying the optional
/// line/column anchor that tools like `rg --hyperlink-format=default` emit.
///
/// The opener uses `line`/`col` to jump an editor to the exact hit; without
/// them it would open the file at line 1. `path` is the raw path from the URL
/// (no percent-decoding — anchors never contain reserved characters, so the
/// trailing `:LINE:COL` split is unambiguous on the encoded form).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLocation {
    pub path: String,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

impl LinkTarget {
    /// Resolve this target to a [`FileLocation`] when it names a local file.
    ///
    /// Returns `Some` for `file://` URLs and for plain path targets, splitting
    /// off any `:LINE`, `:LINE:COL`, `#LINE`, or `#LLINE` anchor. Returns
    /// `None` for non-`file` URLs (e.g. `https://…`).
    pub fn file_location(&self) -> Option<FileLocation> {
        match self {
            Self::Url(url) => parse_file_location(url),
            Self::Path(path) => Some(split_anchor(path)),
        }
    }
}

/// Parse a `file://` URL into a [`FileLocation`], recognizing the location
/// anchors ripgrep, `fd`, and editors emit:
///
/// - `file:///abs/path` → no anchor
/// - `file:///abs/path:42` → line 42
/// - `file:///abs/path:42:7` → line 42, column 7
/// - `file:///abs/path#42` / `file:///abs/path#L42` → line 42
///
/// Returns `None` when `url` does not start with `file://`.
pub fn parse_file_location(url: &str) -> Option<FileLocation> {
    let rest = url.strip_prefix("file://")?;
    // Strip the optional authority: `file://host/path` keeps `/path`,
    // `file:///path` keeps `/path`. A bare `file://relative` has no slash and
    // is taken whole.
    let path = match rest.find('/') {
        Some(idx) => &rest[idx..],
        None => rest,
    };
    Some(split_anchor(path))
}

/// Split a `#LINE` / `#LLINE` fragment or a trailing `:LINE[:COL]` off a raw
/// path. Digit groups are only peeled when the remaining path stays non-empty,
/// so `:42` on a path that is *only* digits is left intact.
fn split_anchor(raw: &str) -> FileLocation {
    if let Some((base, frag)) = raw.rsplit_once('#')
        && !base.is_empty()
    {
        let digits = frag.strip_prefix(['L', 'l']).unwrap_or(frag);
        if let Some(line) = parse_u32(digits) {
            return FileLocation {
                path: base.to_owned(),
                line: Some(line),
                col: None,
            };
        }
    }

    let segs: Vec<&str> = raw.split(':').collect();
    let mut trailing = 0;
    while trailing < 2
        && segs.len() - trailing > 1
        && parse_u32(segs[segs.len() - 1 - trailing]).is_some()
    {
        trailing += 1;
    }

    let (line, col) = match trailing {
        1 => (parse_u32(segs[segs.len() - 1]), None),
        2 => (
            parse_u32(segs[segs.len() - 2]),
            parse_u32(segs[segs.len() - 1]),
        ),
        _ => (None, None),
    };

    FileLocation {
        path: segs[..segs.len() - trailing].join(":"),
        line,
        col,
    }
}

fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridRange {
    pub start: GridPos,
    pub end: GridPos,
}

impl GridRange {
    pub fn contains(self, pos: GridPos) -> bool {
        if pos.row < self.start.row || pos.row > self.end.row {
            return false;
        }
        if self.start.row == self.end.row {
            return pos.col >= self.start.col && pos.col <= self.end.col;
        }
        if pos.row == self.start.row {
            return pos.col >= self.start.col;
        }
        if pos.row == self.end.row {
            return pos.col <= self.end.col;
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedLink {
    pub range: GridRange,
    pub target: LinkTarget,
    pub source: LinkSource,
}

pub struct LinkRule {
    regex: Regex,
    action: LinkAction,
    highlight: LinkHighlight,
    source: LinkSource,
}

impl LinkRule {
    pub fn new(index: usize, pattern: &str) -> Result<Self, fancy_regex::Error> {
        Ok(Self {
            regex: Regex::new(pattern)?,
            action: LinkAction::Open,
            highlight: LinkHighlight::HoverMods,
            source: LinkSource::Rule { index },
        })
    }
}

pub struct LinkDetector {
    enabled: bool,
    activation_mods: LinkModifiers,
    rules: Vec<LinkRule>,
    default_rule: Option<Regex>,
}

impl LinkDetector {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            activation_mods: LinkModifiers::default(),
            rules: Vec::new(),
            default_rule: None,
        }
    }

    pub fn default_ghostty_like(mods: LinkModifiers) -> Self {
        Self::from_options(mods, true, true).expect("default link regex should compile")
    }

    pub fn from_options(
        activation_mods: LinkModifiers,
        urls: bool,
        paths: bool,
    ) -> Result<Self, fancy_regex::Error> {
        Ok(Self {
            enabled: true,
            activation_mods,
            rules: Vec::new(),
            default_rule: default_url::compile(urls, paths)?,
        })
    }

    pub fn set_rules(&mut self, rules: Vec<LinkRule>) {
        self.rules = rules;
    }

    pub fn link_at(
        &self,
        snapshot: &VtSnapshot,
        pos: GridPos,
        mods: LinkModifiers,
    ) -> Option<DetectedLink> {
        if !self.enabled || !self.activation_mods.matches(mods) {
            return None;
        }

        if let Some(run) = snapshot.osc8_run_at(pos.col, pos.row) {
            return Some(DetectedLink {
                range: GridRange {
                    start: GridPos {
                        col: run.start_col,
                        row: run.row,
                    },
                    end: GridPos {
                        col: run.end_col,
                        row: run.row,
                    },
                },
                target: LinkTarget::Url(run.url.to_owned()),
                source: LinkSource::Osc8,
            });
        }

        let line = LogicalLineMap::for_row(snapshot, pos.row)?;
        for rule in &self.rules {
            if let Some(link) = find_in_regex(&line, pos, &rule.regex, rule.source) {
                let _ = (rule.action, rule.highlight);
                return Some(link);
            }
        }
        self.default_rule
            .as_ref()
            .and_then(|regex| find_in_regex(&line, pos, regex, LinkSource::DefaultUrlPath))
    }
}

fn find_in_regex(
    line: &LogicalLineMap,
    pos: GridPos,
    regex: &Regex,
    source: LinkSource,
) -> Option<DetectedLink> {
    for candidate in regex.find_iter(&line.text) {
        let Ok(candidate) = candidate else {
            continue;
        };
        let start = candidate.start();
        let end = candidate.end();
        if start == end || !line.match_contains_pos(start, end, pos) {
            continue;
        }
        let Some(range) = line.range_for_match(start, end) else {
            continue;
        };
        return Some(DetectedLink {
            range,
            target: default_url::classify(candidate.as_str()),
            source,
        });
    }
    None
}

struct LogicalLineMap {
    text: String,
    byte_positions: Vec<GridPos>,
}

impl LogicalLineMap {
    fn for_row(snapshot: &VtSnapshot, row: u16) -> Option<Self> {
        if row >= snapshot.rows {
            return None;
        }

        let mut start = row;
        while start > 0
            && snapshot
                .rows_meta
                .get(usize::from(start))
                .is_some_and(|meta| meta.wrap_continuation)
        {
            start -= 1;
        }

        let mut end = start;
        while end + 1 < snapshot.rows
            && snapshot
                .rows_meta
                .get(usize::from(end))
                .is_some_and(|meta| meta.wrap)
        {
            end += 1;
        }

        let mut map = Self {
            text: String::new(),
            byte_positions: Vec::new(),
        };
        for line_row in start..=end {
            for col in 0..snapshot.cols {
                let Some(cell) = snapshot.cell_at(line_row, col) else {
                    continue;
                };
                let pos = GridPos { col, row: line_row };
                let text = snapshot.cell_text(cell);
                if text.is_empty() {
                    map.push_text("\0", pos);
                } else {
                    map.push_text(text, pos);
                }
            }
        }
        Some(map)
    }

    fn push_text(&mut self, text: &str, pos: GridPos) {
        self.text.push_str(text);
        self.byte_positions
            .extend(std::iter::repeat_n(pos, text.len()));
    }

    fn match_contains_pos(&self, start: usize, end: usize, pos: GridPos) -> bool {
        self.byte_positions[start..end].contains(&pos)
    }

    fn range_for_match(&self, start: usize, end: usize) -> Option<GridRange> {
        let mut mapped = self.byte_positions[start..end].iter().copied();
        let first = mapped.next()?;
        let last = mapped.last().unwrap_or(first);
        Some(GridRange {
            start: first,
            end: last,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use seance_protocol::frame::{CellAttrs, CellColor, NO_HYPERLINK, RowMeta};

    fn snapshot(cols: u16, rows: u16, texts: &[&str]) -> VtSnapshot {
        let mut snapshot = VtSnapshot::empty(cols, rows);
        for text in texts {
            snapshot.push_cell(
                text,
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            );
        }
        snapshot
    }

    fn mods() -> LinkModifiers {
        LinkModifiers {
            super_key: true,
            shift: true,
            ..LinkModifiers::default()
        }
    }

    #[test]
    fn default_path_rule_detects_eza_symlink_target_text() {
        let snapshot = snapshot(
            39,
            1,
            &[
                ".", "h", "u", "s", "h", "l", "o", "g", "i", "n", " ", "-", ">", " ", "d", "o",
                "t", "f", "i", "l", "e", "s", "/", "s", "h", "e", "l", "l", "/", ".", "h", "u",
                "s", "h", "l", "o", "g", "i", "n",
            ],
        );
        let detector = LinkDetector::default_ghostty_like(mods());
        let link = detector
            .link_at(&snapshot, GridPos { col: 14, row: 0 }, mods())
            .unwrap();

        assert_eq!(link.source, LinkSource::DefaultUrlPath);
        assert_eq!(
            link.target,
            LinkTarget::Path("dotfiles/shell/.hushlogin".to_string())
        );
        assert_eq!(
            link.range,
            GridRange {
                start: GridPos { col: 14, row: 0 },
                end: GridPos { col: 38, row: 0 },
            }
        );
    }

    #[test]
    fn bare_dotfile_does_not_match_without_osc8() {
        let snapshot = snapshot(10, 1, &[".", "h", "u", "s", "h", "l", "o", "g", "i", "n"]);
        let detector = LinkDetector::default_ghostty_like(mods());

        assert_eq!(
            detector.link_at(&snapshot, GridPos { col: 0, row: 0 }, mods()),
            None
        );
    }

    #[test]
    fn osc8_wins_over_text_match() {
        let mut snapshot = snapshot(
            23,
            1,
            &[
                "h", "t", "t", "p", "s", ":", "/", "/", "e", "x", "a", "m", "p", "l", "e", ".",
                "c", "o", "m", "/", "x", "y", "z",
            ],
        );
        let idx = snapshot.intern_hyperlink("https://osc8.example");
        snapshot.cells[0].hyperlink_idx = idx;
        let detector = LinkDetector::default_ghostty_like(mods());
        let link = detector
            .link_at(&snapshot, GridPos { col: 0, row: 0 }, mods())
            .unwrap();

        assert_eq!(link.source, LinkSource::Osc8);
        assert_eq!(
            link.target,
            LinkTarget::Url("https://osc8.example".to_string())
        );
    }

    #[test]
    fn modifiers_gate_detection() {
        let snapshot = snapshot(
            15,
            1,
            &[
                "h", "t", "t", "p", "s", ":", "/", "/", "e", ".", "c", "o", "m", "", "",
            ],
        );
        let detector = LinkDetector::default_ghostty_like(mods());

        assert_eq!(
            detector.link_at(
                &snapshot,
                GridPos { col: 0, row: 0 },
                LinkModifiers::default()
            ),
            None
        );
    }

    #[test]
    fn empty_cells_prevent_matches_from_bridging() {
        let snapshot = snapshot(
            15,
            1,
            &[
                "h", "t", "t", "p", "s", ":", "/", "", "/", "e", ".", "c", "o", "m", "",
            ],
        );
        let detector = LinkDetector::default_ghostty_like(mods());

        assert_eq!(
            detector.link_at(&snapshot, GridPos { col: 0, row: 0 }, mods()),
            None
        );
    }

    #[test]
    fn multibyte_text_maps_back_to_cell() {
        let snapshot = snapshot(
            16,
            1,
            &[
                "你", "h", "t", "t", "p", "s", ":", "/", "/", "e", ".", "c", "o", "m", "", "",
            ],
        );
        let detector = LinkDetector::default_ghostty_like(mods());
        let link = detector
            .link_at(&snapshot, GridPos { col: 1, row: 0 }, mods())
            .unwrap();

        assert_eq!(link.range.start, GridPos { col: 1, row: 0 });
    }

    #[test]
    fn wrapped_rows_are_scanned_as_one_logical_line() {
        let mut snapshot = snapshot(
            10,
            2,
            &[
                "s", "r", "c", "/", "c", "o", "n", "f", "i", "g", "/", "u", "r", "l", ".", "z",
                "i", "g", "", "",
            ],
        );
        snapshot.rows_meta = vec![
            RowMeta {
                wrap: true,
                wrap_continuation: false,
            },
            RowMeta {
                wrap: false,
                wrap_continuation: true,
            },
        ];
        let detector = LinkDetector::default_ghostty_like(mods());
        let link = detector
            .link_at(&snapshot, GridPos { col: 1, row: 1 }, mods())
            .unwrap();

        assert_eq!(
            link.range,
            GridRange {
                start: GridPos { col: 0, row: 0 },
                end: GridPos { col: 7, row: 1 },
            }
        );
    }

    #[test]
    fn disabled_default_paths_do_not_match_paths() {
        let snapshot = snapshot(
            25,
            1,
            &[
                "d", "o", "t", "f", "i", "l", "e", "s", "/", "s", "h", "e", "l", "l", "/", ".",
                "h", "u", "s", "h", "l", "o", "g", "i", "n",
            ],
        );
        let detector = LinkDetector::from_options(mods(), true, false).unwrap();

        assert_eq!(
            detector.link_at(&snapshot, GridPos { col: 0, row: 0 }, mods()),
            None
        );
    }

    #[test]
    fn row_with_no_link_leaves_no_hyperlink_sentinel() {
        let snapshot = snapshot(1, 1, &["x"]);
        assert_eq!(snapshot.cells[0].hyperlink_idx, NO_HYPERLINK);
    }

    fn loc(path: &str, line: Option<u32>, col: Option<u32>) -> FileLocation {
        FileLocation {
            path: path.to_owned(),
            line,
            col,
        }
    }

    #[test]
    fn file_url_without_anchor_parses_bare_path() {
        assert_eq!(
            parse_file_location("file:///abs/path"),
            Some(loc("/abs/path", None, None))
        );
    }

    #[test]
    fn file_url_trailing_line_anchor() {
        assert_eq!(
            parse_file_location("file:///abs/path:42"),
            Some(loc("/abs/path", Some(42), None))
        );
    }

    #[test]
    fn file_url_trailing_line_and_col_anchor() {
        assert_eq!(
            parse_file_location("file:///abs/path:42:7"),
            Some(loc("/abs/path", Some(42), Some(7)))
        );
    }

    #[test]
    fn file_url_fragment_line_anchors() {
        assert_eq!(
            parse_file_location("file:///abs/path#42"),
            Some(loc("/abs/path", Some(42), None))
        );
        assert_eq!(
            parse_file_location("file:///abs/path#L42"),
            Some(loc("/abs/path", Some(42), None))
        );
    }

    #[test]
    fn file_url_with_authority_keeps_absolute_path() {
        assert_eq!(
            parse_file_location("file://host/abs/path:9"),
            Some(loc("/abs/path", Some(9), None))
        );
    }

    #[test]
    fn non_file_url_has_no_location() {
        assert_eq!(parse_file_location("https://example.com:8080/x"), None);
    }

    #[test]
    fn link_target_dispatches_by_variant() {
        assert_eq!(
            LinkTarget::Url("https://example.com".into()).file_location(),
            None
        );
        assert_eq!(
            LinkTarget::Path("src/main.rs:42:7".into()).file_location(),
            Some(loc("src/main.rs", Some(42), Some(7)))
        );
    }

    #[test]
    fn all_digit_path_is_not_mistaken_for_anchor() {
        // A path segment that is only digits must survive as the path when
        // stripping it would leave nothing before the colon.
        assert_eq!(
            parse_file_location("file://123"),
            Some(loc("123", None, None))
        );
    }
}
