//! The transcript history (spec 005; FR-16, NFR-06; T-022): the last N delivered final
//! texts, kept locally, copied again, cleared, and deleted when history is turned off.
//!
//! Invariant (T-022 analysis): a text enters the history only through the one
//! `TranscriptSink::delivered` call in `Pipeline::release`'s delivered branch (the
//! production sink is [`HistoryRecorder`]), and the history file is written or deleted
//! only through [`History`], which never writes while disabled (the only disk operation
//! while off is a deletion) and leaves no `history.json*` after off or a clear. No
//! history text appears in a [`HistoryError`], a log line or a `DictationEvent`.
//!
//! - [`History`]: the in-memory list over a [`HistoryStore`] (contracts/core-history.md).
//! - [`FileHistoryStore`]: `history.json` in the data directory, written atomically.
//! - [`HistoryRecorder`]: the shared, locked `History` the pipeline records into and the
//!   shell's commands read; it stamps the clock time and announces every change.
//! - [`follow_settings`]: applies each saved settings snapshot's `history.*`.
//!
//! No network: this module has no HTTP dependency (C12).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod error;
mod model;
mod recorder;
mod store;

use std::sync::mpsc::Receiver;
use std::sync::Arc;

pub use error::{HistoryError, HistoryErrorKind, HistoryOp};
pub use model::{EngineKind, EngineLabel, HistoryEntry, HistoryNotice, HistoryView, StoredHistory};
pub use recorder::HistoryRecorder;
pub use store::{FileHistoryStore, HistoryStore, HISTORY_FILE, HISTORY_TMP_FILE};

use crate::settings::Settings;

/// A disk operation that failed and is retried by the next change (FR-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// The stored document differs from memory: write it.
    Write,
    /// Stored text must go: delete the document.
    Delete,
}

/// The history list: newest first, at most `size` entries, in sync with its store
/// after every successful disk operation (C1).
pub struct History<S: HistoryStore> {
    store: S,
    enabled: bool,
    size: usize,
    entries: Vec<HistoryEntry>,
    next_id: u64,
    pending: Option<Pending>,
    notice: Option<HistoryNotice>,
}

impl<S: HistoryStore> History<S> {
    /// Start-up (FR-009, FR-010): `enabled = false` deletes what is stored (the
    /// interrupted tmp too) and never loads; `enabled = true` loads, and a corrupt or
    /// unreadable document gives an empty history and the error (no backup). A loaded
    /// document above `size` is trimmed and rewritten. At most one error is returned,
    /// for the caller's log.
    pub fn open(store: S, enabled: bool, size: u8) -> (Self, Option<HistoryError>) {
        let mut history = History {
            store,
            enabled,
            size: usize::from(size),
            entries: Vec::new(),
            next_id: 1,
            pending: None,
            notice: None,
        };
        if !enabled {
            let error = history.erase();
            return (history, error);
        }
        let error = match history.store.load() {
            Ok(None) => None,
            Ok(Some(doc)) => {
                history.next_id = doc.next_id;
                history.entries = doc.entries;
                if history.entries.len() > history.size {
                    history.entries.truncate(history.size);
                    history.write()
                } else {
                    None
                }
            }
            Err(error) => Some(error),
        };
        (history, error)
    }

    /// FR-001/FR-002: a no-op while disabled or for a blank `text`. Otherwise the entry
    /// goes first, the list is trimmed to `size` and written. A disk failure keeps the
    /// entry, sets `SaveFailed` and is returned for logging; the caller never fails.
    pub fn record(
        &mut self,
        text: String,
        ready_at_ms: i64,
        engine: EngineLabel,
    ) -> Option<HistoryError> {
        if !self.enabled || text.trim().is_empty() {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.entries.insert(
            0,
            HistoryEntry {
                id,
                text,
                ready_at_ms,
                engine,
            },
        );
        self.entries.truncate(self.size);
        self.write()
    }

    /// FR-007: empties the list and deletes the document (enabled or disabled). While
    /// enabled, a failed deletion falls back to writing an empty document; when that
    /// fails too (or while disabled), the notice is `DeleteFailed` and the next change
    /// retries.
    pub fn clear(&mut self) -> Option<HistoryError> {
        self.entries.clear();
        self.erase()
    }

    /// FR-003/FR-008, from the settings' apply: off clears and deletes (off wins over a
    /// size change in the same call); on from off starts empty and writes nothing until
    /// the next record; a lowered size trims and writes.
    pub fn apply_settings(&mut self, enabled: bool, size: u8) -> Option<HistoryError> {
        self.size = usize::from(size);
        if !enabled {
            let was_enabled = std::mem::replace(&mut self.enabled, false);
            self.entries.clear();
            return if was_enabled || self.pending.is_some() {
                self.erase()
            } else {
                None
            };
        }
        if !self.enabled {
            // Off -> on: the list is already empty; nothing is written now.
            self.enabled = true;
            return None;
        }
        let trimmed = self.entries.len() > self.size;
        self.entries.truncate(self.size);
        if trimmed || self.pending.is_some() {
            self.write()
        } else {
            None
        }
    }

    /// FR-006: the full text of the listed entry `id`; `None` for any other id.
    pub fn text_of(&self, id: u64) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.text.as_str())
    }

    /// FR-005/FR-012: the current view; never touches the store.
    pub fn view(&self) -> HistoryView {
        HistoryView {
            enabled: self.enabled,
            entries: self.entries.clone(),
            notice: self.notice,
        }
    }

    /// Writes `{next_id, entries}` (enabled only). A pending deletion is tried first,
    /// so stored text goes even if this write then fails; the write itself replaces the
    /// whole document either way, so its result decides.
    fn write(&mut self) -> Option<HistoryError> {
        if !self.enabled {
            return None;
        }
        if self.pending == Some(Pending::Delete) {
            // The save below replaces the whole document, so a failed deletion here
            // leaves no old text behind once it succeeds; its error is the save's.
            let _ = self.store.delete();
        }
        let doc = StoredHistory {
            next_id: self.next_id,
            entries: self.entries.clone(),
        };
        match self.store.save(&doc) {
            Ok(()) => {
                self.pending = None;
                self.notice = None;
                None
            }
            Err(error) => {
                self.pending = Some(Pending::Write);
                self.notice = Some(HistoryNotice::SaveFailed);
                Some(error)
            }
        }
    }

    /// Deletes the document. While enabled a failed deletion falls back to saving an
    /// empty document (no text stays on disk); while disabled nothing is ever written.
    /// When the text may still be stored, the notice is `DeleteFailed`, the deletion
    /// stays pending and the deletion's error is returned.
    fn erase(&mut self) -> Option<HistoryError> {
        let error = match self.store.delete() {
            Ok(()) => {
                self.pending = None;
                self.notice = None;
                return None;
            }
            Err(error) => error,
        };
        if self.enabled {
            let empty = StoredHistory {
                next_id: self.next_id,
                entries: Vec::new(),
            };
            if self.store.save(&empty).is_ok() {
                self.pending = None;
                self.notice = None;
                return None;
            }
        }
        self.pending = Some(Pending::Delete);
        self.notice = Some(HistoryNotice::DeleteFailed);
        Some(error)
    }
}

/// The settings follower (FR-003, FR-008; spec 005 T022): applies `history.enabled` and
/// `history.size` of every snapshot `rx` delivers (`SettingsService::subscribe()`)
/// through [`HistoryRecorder::apply_settings`]. Blocks until the channel is closed; the
/// shell runs it on its own thread.
pub fn follow_settings<S: HistoryStore>(
    recorder: Arc<HistoryRecorder<S>>,
    rx: Receiver<Arc<Settings>>,
) {
    for settings in rx {
        recorder.apply_settings(settings.history.enabled, settings.history.size);
    }
}

/// `history.size` (validated 1..=100 by the settings) as the history's `u8`; a value
/// the validation never lets through saturates.
pub fn size_of(size: u32) -> u8 {
    u8::try_from(size).unwrap_or(u8::MAX)
}
