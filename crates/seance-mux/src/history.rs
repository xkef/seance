use std::collections::VecDeque;

use seance_protocol::frame::FrameDelta;
use seance_protocol::identity::{PaneRef, ServerSeq};
use seance_protocol::mux::PaneUpdate;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayBatch {
    Replay(Vec<PaneUpdate>),
    Resync { full: PaneUpdate },
}

#[derive(Debug, Clone)]
pub struct PaneFrameHistory {
    pane: PaneRef,
    max_updates: usize,
    updates: VecDeque<PaneUpdate>,
    latest_full: Option<PaneUpdate>,
}

impl PaneFrameHistory {
    pub fn new(pane: PaneRef, max_updates: usize) -> Self {
        Self {
            pane,
            max_updates: max_updates.max(1),
            updates: VecDeque::new(),
            latest_full: None,
        }
    }

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

    pub fn first_seq(&self) -> Option<ServerSeq> {
        self.updates.front().map(|update| update.seq)
    }

    pub fn latest_seq(&self) -> Option<ServerSeq> {
        self.updates.back().map(|update| update.seq)
    }

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

    pub fn pane(&self) -> PaneRef {
        self.pane
    }
}
