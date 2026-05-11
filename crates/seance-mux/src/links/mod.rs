//! Link detection on top of [`seance_protocol::frame::VtSnapshot`].
//!
//! The detector inspects an OSC 8 hyperlink run first, then walks the
//! logical line at the cursor against an optional set of user-supplied
//! [`LinkRule`]s and a Ghostty-derived URL+path regex. The first match
//! that contains the cursor wins.

mod default_url;

use fancy_regex::Regex;
use seance_protocol::frame::{GridPos, VtSnapshot};

/// Modifier-key requirement for a link to be activatable. A field set
/// to `true` requires that modifier; `false` means "don't care". Use
/// [`Self::matches`] to compare against the live keyboard state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkModifiers {
    /// Super (Command on macOS, Windows/Super elsewhere).
    pub super_key: bool,
    #[allow(missing_docs)]
    pub ctrl: bool,
    #[allow(missing_docs)]
    pub alt: bool,
    #[allow(missing_docs)]
    pub shift: bool,
}

impl LinkModifiers {
    /// Whether `actual` satisfies the requirement encoded in `self`.
    /// Each requested modifier in `self` must also be set in `actual`;
    /// extra modifiers in `actual` are ignored.
    pub fn matches(self, actual: Self) -> bool {
        (!self.super_key || actual.super_key)
            && (!self.ctrl || actual.ctrl)
            && (!self.alt || actual.alt)
            && (!self.shift || actual.shift)
    }
}

/// Action the detector reports a matched link supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAction {
    /// Open the link in the OS default handler (browser, file viewer,
    /// …).
    Open,
}

/// When the renderer should highlight a detected link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkHighlight {
    /// Highlight whenever the cursor hovers a link.
    Hover,
    /// Highlight only when the cursor hovers and the activation
    /// modifiers are held.
    HoverMods,
    /// Always highlight (regardless of cursor position).
    Always,
    /// Always highlight when the activation modifiers are held.
    AlwaysMods,
}

/// Where a detected link came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSource {
    /// OSC 8 hyperlink emitted by the shell or program.
    Osc8,
    /// Match from one of the user-supplied [`LinkRule`]s.
    Rule {
        /// Index into the rule list passed to
        /// [`LinkDetector::set_rules`].
        index: usize,
    },
    /// Match from the default Ghostty-derived URL/path regex.
    DefaultUrlPath,
}

/// Resolved target of a [`DetectedLink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Scheme-prefixed URL (e.g. `https://…`, `mailto:…`).
    Url(#[allow(missing_docs)] String),
    /// Filesystem path candidate.
    Path(#[allow(missing_docs)] String),
}

/// Inclusive grid range covering one detected link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridRange {
    #[allow(missing_docs)]
    pub start: GridPos,
    #[allow(missing_docs)]
    pub end: GridPos,
}

impl GridRange {
    /// Whether `pos` falls inside this range. Multi-row ranges run
    /// from `start.col` on `start.row` through `end.col` on `end.row`.
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

/// One detected link: where it sits on the grid, what it points at,
/// and which detection path produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedLink {
    #[allow(missing_docs)]
    pub range: GridRange,
    #[allow(missing_docs)]
    pub target: LinkTarget,
    #[allow(missing_docs)]
    pub source: LinkSource,
}

/// User-supplied regex rule for link detection. Built via
/// [`Self::new`]; installed on a [`LinkDetector`] via
/// [`LinkDetector::set_rules`].
pub struct LinkRule {
    regex: Regex,
    action: LinkAction,
    highlight: LinkHighlight,
    source: LinkSource,
}

impl LinkRule {
    /// Compile `pattern` as a fancy-regex and tag matches with
    /// `LinkSource::Rule { index }`.
    pub fn new(index: usize, pattern: &str) -> Result<Self, fancy_regex::Error> {
        Ok(Self {
            regex: Regex::new(pattern)?,
            action: LinkAction::Open,
            highlight: LinkHighlight::HoverMods,
            source: LinkSource::Rule { index },
        })
    }
}

/// Stateful link detector: holds activation modifiers, an optional
/// list of user rules, and the default URL+path regex. Drive it via
/// [`Self::link_at`] from the input/render path.
pub struct LinkDetector {
    activation_mods: LinkModifiers,
    rules: Vec<LinkRule>,
    default_rule: Option<Regex>,
}

impl LinkDetector {
    /// Detector with the bundled Ghostty-derived URL+path regex
    /// enabled for both URLs and paths. Panics on internal regex
    /// compile failure (the pattern is exercised by tests).
    pub fn default_ghostty_like(mods: LinkModifiers) -> Self {
        Self::from_options(mods, true, true).expect("default link regex should compile")
    }

    /// Detector that selectively enables the default URL and/or path
    /// branches. Returns `Err` only if the bundled regex fails to
    /// compile.
    pub fn from_options(
        activation_mods: LinkModifiers,
        urls: bool,
        paths: bool,
    ) -> Result<Self, fancy_regex::Error> {
        Ok(Self {
            activation_mods,
            rules: Vec::new(),
            default_rule: default_url::compile(urls, paths)?,
        })
    }

    /// Replace the configured user rules. Rules are tried in order,
    /// before the default URL/path regex.
    pub fn set_rules(&mut self, rules: Vec<LinkRule>) {
        self.rules = rules;
    }

    /// Find a link at `pos` under `mods`. Returns `None` if `mods` do
    /// not satisfy the configured activation modifiers, if `pos` is
    /// out of range, or if no detector branch matched.
    pub fn link_at(
        &self,
        snapshot: &VtSnapshot,
        pos: GridPos,
        mods: LinkModifiers,
    ) -> Option<DetectedLink> {
        if !self.activation_mods.matches(mods) {
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
}
