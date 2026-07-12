//! Content-based pane observation: locate text/patterns in a pane's visible
//! screen, and await content across refreshes.
//!
//! The rmux/Playwright "wait_for_text + snapshot" primitive: instead of
//! polling `capture-pane` and sleeping, an agent asks to be told when a pane's
//! content matches a substring or regular expression. [`ContentMatcher::find`]
//! is the synchronous core — evaluate a matcher against one snapshot —
//! and [`ContentWait`] drives it across refreshes with a deadline, resolving
//! on a match or rejecting on timeout.
//!
//! Matching runs over logical lines: soft-wrapped grid rows are joined into a
//! single line before scanning, so a pattern split across a wrap boundary
//! still matches. Blank cells render as a single space so column offsets in a
//! reported [`GridRange`] line up with the grid.

use std::time::Instant;

use fancy_regex::Regex;
use seance_protocol::agent::PaneSnapshot;
use seance_protocol::frame::{GridPos, VtSnapshot};
use seance_protocol::identity::PaneRef;

use crate::MuxClient;
use crate::links::GridRange;

/// What to look for in a pane's content.
///
/// Not `Clone`/`PartialEq`: a compiled [`Regex`] carries neither, and a
/// matcher is meant to be built once and moved into a [`ContentWait`] or
/// passed by reference to [`MuxClient::find_in_pane`].
pub struct ContentMatcher {
    kind: MatcherKind,
}

enum MatcherKind {
    Substring(String),
    Regex(Regex),
}

impl ContentMatcher {
    /// Match the first occurrence of `needle` as a literal substring.
    pub fn substring(needle: impl Into<String>) -> Self {
        Self {
            kind: MatcherKind::Substring(needle.into()),
        }
    }

    /// Match the first occurrence of a [`fancy_regex`] pattern.
    pub fn regex(pattern: &str) -> Result<Self, fancy_regex::Error> {
        Ok(Self {
            kind: MatcherKind::Regex(Regex::new(pattern)?),
        })
    }

    /// Find the first (top-most, then left-most) match in `snapshot`, or
    /// `None` if the content does not match.
    pub fn find(&self, snapshot: &VtSnapshot) -> Option<ContentMatch> {
        for line in LogicalLine::screen(snapshot) {
            if let Some(m) = self.find_in_line(&line) {
                return Some(m);
            }
        }
        None
    }

    fn find_in_line(&self, line: &LogicalLine) -> Option<ContentMatch> {
        match &self.kind {
            MatcherKind::Substring(needle) => {
                if needle.is_empty() {
                    return None;
                }
                let start = line.text.find(needle.as_str())?;
                let end = start + needle.len();
                line.range_for(start, end).map(|range| ContentMatch {
                    range,
                    text: needle.clone(),
                })
            }
            MatcherKind::Regex(regex) => {
                for candidate in regex.find_iter(&line.text) {
                    let Ok(candidate) = candidate else { continue };
                    if candidate.start() == candidate.end() {
                        continue;
                    }
                    if let Some(range) = line.range_for(candidate.start(), candidate.end()) {
                        return Some(ContentMatch {
                            range,
                            text: candidate.as_str().to_string(),
                        });
                    }
                }
                None
            }
        }
    }
}

/// A located run of content: where it sits on the grid and the matched text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMatch {
    pub range: GridRange,
    pub text: String,
}

/// An in-progress await for pane content. Poll it once per client refresh with
/// the current time; it resolves when the pane's content matches, or times out
/// once `now` passes the deadline.
///
/// The clock is injected rather than read internally, keeping this crate free
/// of ambient time (the host owns the event loop and supplies `Instant::now()`).
pub struct ContentWait {
    pane: PaneRef,
    matcher: ContentMatcher,
    deadline: Instant,
}

impl ContentWait {
    pub fn new(pane: PaneRef, matcher: ContentMatcher, deadline: Instant) -> Self {
        Self {
            pane,
            matcher,
            deadline,
        }
    }

    pub fn pane(&self) -> PaneRef {
        self.pane
    }

    /// Evaluate the wait against the client's current state. A match wins over
    /// the deadline: if the content matches at `now == deadline`, the result
    /// is [`WaitPoll::Matched`], not [`WaitPoll::TimedOut`].
    pub fn poll<D>(&self, client: &MuxClient<D>, now: Instant) -> WaitPoll {
        if let Some(view) = client.pane_view(self.pane)
            && let Some(matched) = view.find_content(&self.matcher)
            && let Some(snapshot) = view.stable_snapshot()
        {
            return WaitPoll::Matched {
                matched,
                snapshot: Box::new(snapshot),
            };
        }
        if now >= self.deadline {
            WaitPoll::TimedOut
        } else {
            WaitPoll::Pending
        }
    }
}

/// Outcome of polling a [`ContentWait`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitPoll {
    /// Content matched. Carries the located run and the frozen snapshot it
    /// matched against, so the caller never re-reads a since-changed pane.
    Matched {
        matched: ContentMatch,
        snapshot: Box<PaneSnapshot>,
    },
    /// No match yet and the deadline has not passed — poll again next refresh.
    Pending,
    /// The deadline passed with no match.
    TimedOut,
}

/// One logical line of the screen — a wrap chain of grid rows joined into a
/// single string, with a byte→[`GridPos`] map so a match offset resolves back
/// to grid coordinates. Blank cells contribute a single space.
struct LogicalLine {
    text: String,
    byte_positions: Vec<GridPos>,
}

impl LogicalLine {
    /// Every logical line of `snapshot`, top to bottom. A row that is a wrap
    /// continuation of the row above is folded into that line rather than
    /// starting its own.
    fn screen(snapshot: &VtSnapshot) -> Vec<LogicalLine> {
        let mut lines = Vec::new();
        let mut row = 0;
        while row < snapshot.rows {
            let (start, end) = snapshot.wrap_chain(row);
            lines.push(LogicalLine::for_chain(snapshot, start, end));
            row = end + 1;
        }
        lines
    }

    fn for_chain(snapshot: &VtSnapshot, start: u16, end: u16) -> LogicalLine {
        let mut line = LogicalLine {
            text: String::new(),
            byte_positions: Vec::new(),
        };
        for row in start..=end {
            for col in 0..snapshot.cols {
                let pos = GridPos { col, row };
                let text = snapshot
                    .cell_at(row, col)
                    .map(|cell| snapshot.cell_text(cell));
                match text {
                    Some(text) if !text.is_empty() => line.push(text, pos),
                    _ => line.push(" ", pos),
                }
            }
        }
        line
    }

    fn push(&mut self, text: &str, pos: GridPos) {
        self.text.push_str(text);
        self.byte_positions
            .extend(std::iter::repeat_n(pos, text.len()));
    }

    fn range_for(&self, start: usize, end: usize) -> Option<GridRange> {
        let mut mapped = self.byte_positions.get(start..end)?.iter().copied();
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
    use seance_protocol::frame::{CellAttrs, CellColor, RowMeta, VtSnapshot};

    fn snapshot(cols: u16, rows: u16, cells: &[&str]) -> VtSnapshot {
        let mut snap = VtSnapshot::empty(cols, rows);
        snap.generation = 1;
        for text in cells {
            snap.push_cell(
                text,
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            );
        }
        snap
    }

    #[test]
    fn substring_matches_on_a_single_row() {
        let snap = snapshot(5, 1, &["h", "e", "l", "l", "o"]);
        let matcher = ContentMatcher::substring("ell");
        let m = matcher.find(&snap).unwrap();
        assert_eq!(m.text, "ell");
        assert_eq!(
            m.range,
            GridRange {
                start: GridPos { col: 1, row: 0 },
                end: GridPos { col: 3, row: 0 },
            }
        );
    }

    #[test]
    fn substring_reports_the_top_most_row() {
        let snap = snapshot(3, 2, &["a", "b", "c", "b", "a", "r"]);
        let m = ContentMatcher::substring("a").find(&snap).unwrap();
        assert_eq!(m.range.start, GridPos { col: 0, row: 0 });
    }

    #[test]
    fn empty_substring_never_matches() {
        let snap = snapshot(2, 1, &["a", "b"]);
        assert!(ContentMatcher::substring("").find(&snap).is_none());
    }

    #[test]
    fn no_match_returns_none() {
        let snap = snapshot(3, 1, &["a", "b", "c"]);
        assert!(ContentMatcher::substring("xyz").find(&snap).is_none());
    }

    #[test]
    fn regex_matches_and_maps_the_range() {
        let snap = snapshot(7, 1, &["e", "r", "r", ":", " ", "4", "2"]);
        let matcher = ContentMatcher::regex(r"\d+").unwrap();
        let m = matcher.find(&snap).unwrap();
        assert_eq!(m.text, "42");
        assert_eq!(
            m.range,
            GridRange {
                start: GridPos { col: 5, row: 0 },
                end: GridPos { col: 6, row: 0 },
            }
        );
    }

    #[test]
    fn wrapped_rows_match_as_one_logical_line() {
        let mut snap = snapshot(3, 2, &["f", "o", "o", "b", "a", "r"]);
        snap.rows_meta = vec![
            RowMeta {
                wrap: true,
                wrap_continuation: false,
            },
            RowMeta {
                wrap: false,
                wrap_continuation: true,
            },
        ];
        let m = ContentMatcher::substring("oob").find(&snap).unwrap();
        assert_eq!(
            m.range,
            GridRange {
                start: GridPos { col: 1, row: 0 },
                end: GridPos { col: 0, row: 1 },
            }
        );
    }

    #[test]
    fn blank_cells_render_as_spaces_between_words() {
        let snap = snapshot(5, 1, &["h", "i", "", "y", "o"]);
        let m = ContentMatcher::substring("hi y").find(&snap).unwrap();
        assert_eq!(m.range.start, GridPos { col: 0, row: 0 });
        assert_eq!(m.range.end, GridPos { col: 3, row: 0 });
    }

    #[test]
    fn multibyte_cell_maps_back_to_its_column() {
        let snap = snapshot(3, 1, &["你", "O", "K"]);
        let m = ContentMatcher::substring("OK").find(&snap).unwrap();
        assert_eq!(m.range.start, GridPos { col: 1, row: 0 });
        assert_eq!(m.range.end, GridPos { col: 2, row: 0 });
    }
}
