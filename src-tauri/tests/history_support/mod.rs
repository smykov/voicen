//! T-022 (review 1 #1): the shared rig of `history_ipc.rs` and `history_wiring.rs`: the shell's
//! app wiring (`build_app` over `win32_support::AppRig`, mock runtime) with the history part
//! started by `voicen_lib::history::start_history` over a fake clipboard, a History window
//! opened through the product's route, and the readers of what the shell did (the files in the
//! history dir, the log lines, the command answers). Windows only, like the tests that include it
//! (decision #5). The including file declares `mod timing_support; mod win32_support;
//! mod history_support;`.
//!
//! Entries are made through the pipeline's own `TranscriptSink` (`history::transcript_sink`,
//! the managed recorder once `start_history` ran), so no test reaches into the recorder; the
//! wiring test (`history_wiring.rs`) makes them with a real delivered job instead.
//!
//! Fake data only: `example.com`, `sk-test-...` keys (the rig's), obvious canary texts.
#![cfg(windows)]
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{AppHandle, Manager, WebviewWindow};
use voicen_core::history::{EngineKind as HistoryEngine, EngineLabel};
use voicen_core::platform::{Clipboard, FakeClipboard};
use voicen_core::settings::service::{SaveOutcome, SaveRequest, SettingsService};
use voicen_core::settings::EngineKind;
use voicen_lib::history::{self, LABEL};

use crate::win32_support::{eventually, AppRig};

/// What the leftover files of an earlier run hold: the sentinel must be gone with them.
pub const LEFTOVER: &str = "LEFTOVER-SENTINEL-7b3e1c";

/// One app with the history part started over a fake clipboard.
pub struct HistoryRig {
    pub rig: AppRig,
    pub clipboard: Arc<FakeClipboard>,
    /// The directory given to `start_history` (a sub-folder of the rig's data dir).
    pub data: PathBuf,
}

impl HistoryRig {
    /// An app whose history part is not started yet, and the directory it will be given.
    pub fn unstarted() -> HistoryRig {
        let rig = AppRig::new(EngineKind::Api);
        let data = rig.dir.path().join("history-data");
        fs::create_dir_all(&data).expect("the history dir");
        HistoryRig {
            rig,
            clipboard: Arc::new(FakeClipboard::new()),
            data,
        }
    }

    /// An app with the history part started (history on, the defaults).
    pub fn started() -> HistoryRig {
        let this = HistoryRig::unstarted();
        this.start().expect("start_history");
        this
    }

    /// `start_history` over this rig's directory and fake clipboard.
    pub fn start(&self) -> std::io::Result<()> {
        self.start_with(Arc::clone(&self.clipboard) as Arc<dyn Clipboard>)
    }

    /// `start_history` over this rig's directory and the given clipboard writer.
    pub fn start_with(&self, clipboard: Arc<dyn Clipboard>) -> std::io::Result<()> {
        history::start_history(self.handle(), self.data.clone(), clipboard)
    }

    pub fn handle(&self) -> &AppHandle<MockRuntime> {
        self.rig.app.handle()
    }

    pub fn service(&self) -> &Arc<SettingsService> {
        &self.rig.service
    }

    /// Saves `history.enabled = enabled` through the real `SettingsService::save`.
    #[track_caller]
    pub fn save_history_enabled(&self, enabled: bool) {
        let mut settings = (*self.service().snapshot()).clone();
        settings.history.enabled = enabled;
        match self.service().save(SaveRequest {
            settings,
            keys: Default::default(),
        }) {
            SaveOutcome::Saved { .. } => {}
            other => panic!("premise: saving history.enabled={enabled} was refused: {other:?}"),
        }
    }

    /// Delivers `text` into the history the way the pipeline does: through the pipeline's
    /// `TranscriptSink` of the app.
    pub fn deliver(&self, text: &str) {
        history::transcript_sink(self.handle()).delivered(text, label());
    }

    /// The names of the `history.json*` files in the history dir, sorted.
    pub fn history_files(&self) -> Vec<String> {
        history_files(&self.data)
    }

    /// The log lines whose message starts with `head` (`<timestamp> <LEVEL> <head> ...`).
    pub fn lines_headed(&self, head: &str) -> Vec<String> {
        crate::win32_support::headed(&self.rig.log_lines(), head)
    }

    /// Opens the History window through the product's route and waits for it.
    #[track_caller]
    pub fn open_window(&self) -> WebviewWindow<MockRuntime> {
        history::open_history(self.handle());
        self.wait_window()
    }

    #[track_caller]
    pub fn wait_window(&self) -> WebviewWindow<MockRuntime> {
        assert!(
            eventually(|| self.handle().get_webview_window(LABEL).is_some()),
            "the `{LABEL}` window did not appear"
        );
        self.handle()
            .get_webview_window(LABEL)
            .expect("the history window")
    }

    /// The windows of the app whose label is `LABEL`.
    pub fn history_windows(&self) -> usize {
        self.handle()
            .webview_windows()
            .keys()
            .filter(|label| label.as_str() == LABEL)
            .count()
    }
}

/// The engine label of every test entry.
pub fn label() -> EngineLabel {
    EngineLabel {
        kind: HistoryEngine::Api,
        model: "whisper-1".to_string(),
    }
}

/// The `history.json*` file names in `dir`, sorted (none when the dir is not there).
pub fn history_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with("history.json"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Invokes `cmd` from `window`, from the window's own URL, like its page does.
pub fn invoke(window: &WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Result<Value, Value> {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: window.url().expect("window url"),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().expect("response is JSON"))
}

/// `invoke` that must resolve.
#[track_caller]
pub fn ok(window: &WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Value {
    invoke(window, cmd, args).unwrap_or_else(|e| panic!("{cmd} rejected: {e}"))
}

/// `history_get` as the page sees it.
#[track_caller]
pub fn get(window: &WebviewWindow<MockRuntime>) -> Value {
    ok(window, "history_get", json!({}))
}

/// The texts of a `history_get` answer, newest first.
pub fn texts_of(view: &Value) -> Vec<String> {
    view["entries"]
        .as_array()
        .expect("entries is an array")
        .iter()
        .map(|e| e["text"].as_str().expect("entry text").to_string())
        .collect()
}

/// The id of the entry whose text is `text` in a `history_get` answer.
#[track_caller]
pub fn id_of(view: &Value, text: &str) -> u64 {
    view["entries"]
        .as_array()
        .expect("entries is an array")
        .iter()
        .find(|e| e["text"] == json!(text))
        .and_then(|e| e["id"].as_u64())
        .unwrap_or_else(|| panic!("no entry with that text in {view}"))
}
