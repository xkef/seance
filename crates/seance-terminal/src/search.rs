//! Text search within terminal screen content.

/// A search match location in the terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    /// Row of the match start.
    pub start_row: u16,
    /// Column of the match start.
    pub start_col: u16,
    /// Row of the match end (inclusive).
    pub end_row: u16,
    /// Column of the match end (inclusive).
    pub end_col: u16,
}

/// Persistent search state for a terminal.
///
/// Caches the query and match list. Call [`search`] to find all matches
/// on the current screen content, then use [`next`]/[`prev`] to cycle.
pub struct SearchState {
    query: String,
    matches: Vec<SearchMatch>,
    current: Option<usize>,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current: None,
        }
    }

    /// Run a new search against screen content. Returns the number of matches.
    ///
    /// `screen` is the full screen text (from `dump_screen`), and `cols`
    /// is the grid width so we can convert linear offsets to grid positions.
    pub fn search(&mut self, query: &str, screen: &str, cols: u16) -> usize {
        self.query = query.to_string();
        self.matches.clear();
        self.current = None;

        if query.is_empty() {
            return 0;
        }

        let query_lower = query.to_lowercase();
        let lines: Vec<&str> = screen.lines().collect();

        for (row_idx, line) in lines.iter().enumerate() {
            let line_lower = line.to_lowercase();
            let mut search_start = 0;

            while let Some(offset) = line_lower[search_start..].find(&query_lower) {
                let col = search_start + offset;
                let end_col = col + query.len() - 1;

                self.matches.push(SearchMatch {
                    start_row: row_idx as u16,
                    start_col: col.min(cols as usize - 1) as u16,
                    end_row: row_idx as u16,
                    end_col: end_col.min(cols as usize - 1) as u16,
                });

                search_start = col + 1;
            }
        }

        if !self.matches.is_empty() {
            self.current = Some(0);
        }

        self.matches.len()
    }

    /// Advance to the next match, wrapping around.
    pub fn next(&mut self) -> Option<SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }
        let idx = match self.current {
            Some(i) => (i + 1) % self.matches.len(),
            None => 0,
        };
        self.current = Some(idx);
        Some(self.matches[idx])
    }

    /// Move to the previous match, wrapping around.
    pub fn prev(&mut self) -> Option<SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }
        let idx = match self.current {
            Some(0) | None => self.matches.len() - 1,
            Some(i) => i - 1,
        };
        self.current = Some(idx);
        Some(self.matches[idx])
    }

    /// The currently highlighted match, if any.
    pub fn current_match(&self) -> Option<SearchMatch> {
        self.current.map(|i| self.matches[i])
    }

    /// All matches from the most recent search.
    pub fn matches(&self) -> &[SearchMatch] {
        &self.matches
    }

    /// The active search query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Clear the search state entirely.
    pub fn clear(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.current = None;
    }
}
