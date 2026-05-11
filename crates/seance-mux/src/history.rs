use std::collections::VecDeque;

use seance_protocol::frame::FrameDelta;
use seance_protocol::identity::{PaneRef, ServerSeq};
use seance_protocol::mux::PaneUpdate;

/// What [`PaneFrameHistory::replay_since`] returns — either a
/// contiguous list of recent updates the client missed, or a
/// full-snapshot resync.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayBatch {
    /// Updates strictly newer than the caller's last seen sequence, in
    /// order. May be empty if the client is already up to date.
    Replay(#[allow(missing_docs)] Vec<PaneUpdate>),
    /// History could not cover the requested gap; caller must accept
    /// the supplied full snapshot and discard prior state.
    Resync {
        #[allow(missing_docs)]
        full: PaneUpdate,
    },
}

/// Bounded ring of recent [`PaneUpdate`]s for one pane, plus the most
/// recent full snapshot. The server uses it to answer resume requests
/// from a reconnecting client without replaying from scratch.
#[derive(Debug, Clone)]
pub struct PaneFrameHistory {
    pane: PaneRef,
    max_updates: usize,
    updates: VecDeque<PaneUpdate>,
    latest_full: Option<PaneUpdate>,
}

impl PaneFrameHistory {
    /// New empty history for `pane`. `max_updates` is clamped to at
    /// least 1; older updates are discarded when the bound is reached.
    pub fn new(pane: PaneRef, max_updates: usize) -> Self {
        Self {
            pane,
            max_updates: max_updates.max(1),
            updates: VecDeque::new(),
            latest_full: None,
        }
    }

    /// Record an update. Full frames are also retained separately so a
    /// later resync can fall back to them when the ring rolled over.
    pub fn push(&mut self, update: PaneUpdate) {
        if update
            .frame
            .as_ref()
            .is_some_and(|frame| matches!(frame, FrameDelta::Full { .. }))
        {
            self.latest_full = Some(update.clone());
        }
        self.updates.push_back(update);
        while self.updates.len() > self.max_updates {
            self.updates.pop_front();
        }
    }

    /// Sequence of the oldest update still in the ring.
    pub fn first_seq(&self) -> Option<ServerSeq> {
        self.updates.front().map(|update| update.seq)
    }

    /// Sequence of the most recent update.
    pub fn latest_seq(&self) -> Option<ServerSeq> {
        self.updates.back().map(|update| update.seq)
    }

    /// Build a resume payload for a client whose last applied sequence
    /// is `last_seen`. `None` indicates a fresh client; the function
    /// falls back to a full-snapshot resync when the ring cannot cover
    /// the gap.
    pub fn replay_since(&self, last_seen: Option<ServerSeq>) -> Option<ReplayBatch> {
        match last_seen {
            None => self
                .latest_full
                .clone()
                .map(|full| ReplayBatch::Resync { full }),
            Some(seq) => {
                if self.updates.is_empty() {
                    return self
                        .latest_full
                        .clone()
                        .map(|full| ReplayBatch::Resync { full });
                }
                let first = self.first_seq()?;
                let latest = self.latest_seq()?;
                if seq.0 >= latest.0 {
                    return Some(ReplayBatch::Replay(Vec::new()));
                }
                if seq.0 < first.0.saturating_sub(1) {
                    return self
                        .latest_full
                        .clone()
                        .map(|full| ReplayBatch::Resync { full });
                }

                Some(ReplayBatch::Replay(
                    self.updates
                        .iter()
                        .filter(|update| update.seq.0 > seq.0)
                        .cloned()
                        .collect(),
                ))
            }
        }
    }

    /// Pane this history belongs to.
    pub fn pane(&self) -> PaneRef {
        self.pane
    }
}
