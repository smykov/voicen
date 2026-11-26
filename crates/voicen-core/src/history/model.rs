//! The history's data shapes (spec 005 data-model.md, contracts/core-history.md
//! "Types"). The serde names are the contract's: they are both the stored document's
//! (`history.json`, schema 1) and the view's.

use serde::{Deserialize, Serialize};

use crate::settings::{EngineKind as SettingsEngine, Settings};

/// The engine a dictation ran with, as the history names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// The OpenAI-compatible API (`settings::EngineKind::Api`).
    Api,
    /// The built-in whisper.cpp engine (`settings::EngineKind::BuiltinLocal`).
    Local,
    /// A local OpenAI-compatible server (`settings::EngineKind::LocalServer`).
    Server,
}

/// The engine and its model name; the model may be empty (a local server without a
/// configured model).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngineLabel {
    pub kind: EngineKind,
    pub model: String,
}

impl EngineLabel {
    /// The label of the engine `settings` selects, with that engine's model name from
    /// the same snapshot (the one engine enum is the settings', P-010). `None` for
    /// `settings::EngineKind::None`, which never delivers (`engine_for` refuses it).
    pub fn of(settings: &Settings) -> Option<EngineLabel> {
        let (kind, model) = match settings.engine {
            SettingsEngine::None => return None,
            SettingsEngine::Api => (EngineKind::Api, settings.api.model.clone()),
            SettingsEngine::BuiltinLocal => (
                EngineKind::Local,
                settings.builtin_local.model_id.clone().unwrap_or_default(),
            ),
            SettingsEngine::LocalServer => {
                (EngineKind::Server, settings.local_server.model.clone())
            }
        };
        Some(EngineLabel { kind, model })
    }
}

/// One delivered final transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    pub text: String,
    /// UTC milliseconds since the Unix epoch when the text was ready; display only.
    pub ready_at_ms: i64,
    pub engine: EngineLabel,
}

/// A disk failure the window shows until the next successful write or deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryNotice {
    SaveFailed,
    DeleteFailed,
}

/// What the History window shows: newest first, empty while disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryView {
    pub enabled: bool,
    pub entries: Vec<HistoryEntry>,
    pub notice: Option<HistoryNotice>,
}

/// The stored document without its schema number (the store writes `schema: 1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredHistory {
    pub next_id: u64,
    /// Newest first; ids strictly decreasing.
    pub entries: Vec<HistoryEntry>,
}
