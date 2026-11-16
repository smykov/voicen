//! T-142: the Calls window and its commands (spec 007 FR-033, FR-034, FR-036; contracts/ipc.md;
//! research R18; decisions #5, #45, #57, #273, #277, #279) over the shell's app wiring
//! (`build_app`, mock runtime) with the call part started by `voicen_lib::call::start_calls` over
//! fakes. Windows CI only (decision #5): none of this runs on the Linux dev host, so the red and
//! green evidence is the `ci.yml` run on `wip/T-142`.
//!
//! The invariants (docs/tasks/T-142.md analysis): (I2) at most one `calls` window, made on its own
//! opener thread, never waited for by the caller, and an open with `{root, id}` reaches the page as
//! exactly one `calls://open`, also on a window just created (no event before the page's
//! listeners exist); (I3) every command resolves `{root, id}` through core's guard, a bad root or
//! id is `notFound` with no port touched; (I4) copy writes exactly `CallStore::export_bytes(Md)`
//! through the one `Clipboard` port (`WinClipboard::set_text_excluded_from_history`); (I5) export
//! gets its target only from the `SaveDialog` port, a cancel is `{saved: false}` and writes
//! nothing, a pick writes only through `export_to`; (I6) a start and every stop emit one
//! `calls://changed {root, id}` to the `calls` label only. The ACL cases (I1) are in
//! `app_acl.rs`.
//!
//! Signatures this file assumes (the developer's target; none exists yet, so the file fails to
//! compile until they do, and that unresolved import is the red):
//!
//! ```text
//! voicen_lib::call::
//!   CallRef { pub root: String, pub id: String }
//!   open_calls(app: &AppHandle<R>, target: Option<CallRef>)  // posts to the `calls` opener thread, returns at once
//!   trait SaveDialog: Send + Sync
//!       { fn pick(&self, suggested_name: &str, format: voicen_core::call::storage::Format)
//!             -> io::Result<Option<PathBuf>> }               // Ok(None) = cancelled
//!   CallPorts { .., clipboard: Arc<dyn Clipboard>, folder: Arc<dyn FolderOpener>, save: Arc<dyn SaveDialog> }
//! label `calls`, page `/calls`; commands calls_list / calls_get / calls_copy_markdown /
//! calls_export / calls_open_folder (args `{root, id[, format]}`); events `calls://open` and
//! `calls://changed`, payload `{root, id}`; `calls_opened` keeps counting every `open_calls`.
//! The cold-window open (the developer's mechanism, T-142 analysis): the target is held and
//! emitted once, after the window's first `calls_list` (the page sends it only after its
//! listeners exist); on a window that already listed, it is emitted at once.
//! ```
//!
//! Subscribe before the action and match on `{root, id}`, never on "the first event" (F-018);
//! every wait is on state through `call_shell_support` / `timing_support` (F-031).
#![cfg(windows)]

mod call_shell_support;
mod timing_support;
mod win32_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use call_shell_support::{
    eventually, holds_for_a_while, plain_opener, CallRig, FakeFolder, FakeSave,
};
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::menu::{MenuEvent, MenuId};
use tauri::test::{get_ipc_response, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::{Listener, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use voicen_core::call::storage::{wire_root, Format};
use voicen_core::call::{CallSource, CallStore};
use voicen_core::platform::{Clipboard, FakeClipboard};
use voicen_core::settings::gate::SettingsTab;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::tray::TrayAction;
use voicen_lib::call::{self, CallRef, CallRequest};
use voicen_lib::settings_window;
use voicen_lib::tray;
use voicen_lib::win::clipboard::WinClipboard;
use win32_support::{put_text, read_clipboard, serial, ClipboardState};

const WIRE_FIXTURE: &str = include_str!("../../e2e/fixtures/calls-wire.json");
const LABEL: &str = "calls";
const CHANGED: &str = "calls://changed";
const OPEN: &str = "calls://open";
const READY_ID: &str = "2026-10-29_100000";
const DAMAGED_ID: &str = "2026-10-26_081507-2";

/// `transcript.json` of the fixture's ready call (the text of core's storage tests).
const TRANSCRIPT_TEXT: &str = concat!(
    r#"{"schema":1,"call_id":"2026-10-29_100000","title":"Own title","#,
    r#""started_at":"2026-10-29T10:00:00+03:00","duration_ms":3600000,"#,
    r#""speakers":[{"id":"me","name":"X"},{"id":"s1","name":"Y"}],"#,
    r#""segments":[{"start_ms":0,"end_ms":2000,"track":"mic","speaker":"me","lang":"en","text":"Hello there","source":"final"},"#,
    r#"{"start_ms":1500,"end_ms":4000,"track":"system","speaker":"s1","lang":null,"text":"Hi","source":"final"}],"#,
    r#""engines":{"transcription":"api:whisper-1","diarization":null}}"#
);

// ---- helpers ------------------------------------------------------------------------------

fn fixture() -> Value {
    serde_json::from_str(WIRE_FIXTURE).expect("calls-wire.json is JSON")
}

fn menu_item(action: TrayAction) -> MenuEvent {
    MenuEvent {
        id: MenuId::new(action.as_str()),
    }
}

/// The ports of one rig and the rig.
struct Ipc {
    rig: CallRig,
    clipboard: Arc<FakeClipboard>,
    folder: Arc<FakeFolder>,
    save: Arc<FakeSave>,
    root: PathBuf,
}

/// A started call part over a fake clipboard, folder opener and save dialog.
fn ipc() -> Ipc {
    let rig = CallRig::bare();
    let clipboard = Arc::new(FakeClipboard::new());
    let folder = Arc::new(FakeFolder::default());
    let save = Arc::new(FakeSave::default());
    rig.start_with(
        Arc::new(call_shell_support::WavSource::new()),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
        clipboard.clone(),
        folder.clone(),
        save.clone(),
    );
    let root = rig.dir.path().join("calls");
    Ipc {
        rig,
        clipboard,
        folder,
        save,
        root,
    }
}

impl Ipc {
    fn handle(&self) -> &tauri::AppHandle<MockRuntime> {
        self.rig.handle()
    }

    fn root_text(&self) -> String {
        wire_root(&self.root)
    }

    fn reference(&self, id: &str) -> CallRef {
        CallRef {
            root: self.root_text(),
            id: id.to_string(),
        }
    }

    fn windows(&self) -> usize {
        self.handle().webview_windows().len()
    }

    /// Opens the Calls window through the product's route and waits for it.
    #[track_caller]
    fn open_window(&self, target: Option<CallRef>) -> WebviewWindow<MockRuntime> {
        call::open_calls(self.handle(), target);
        self.wait_window()
    }

    #[track_caller]
    fn wait_window(&self) -> WebviewWindow<MockRuntime> {
        assert!(
            eventually(|| self.handle().get_webview_window(LABEL).is_some()),
            "the `{LABEL}` window did not appear"
        );
        self.handle()
            .get_webview_window(LABEL)
            .expect("the calls window")
    }

    /// The only `.voicen-tmp` leftovers anywhere under `dir`.
    fn tmp_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(dir) else {
            return out;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(Ipc::tmp_files(&path));
            } else if path.to_string_lossy().contains("voicen-tmp") {
                out.push(path);
            }
        }
        out
    }

    /// Records one real call (the tone source), then makes of it the fixture's ready call
    /// `READY_ID` (its meta, its transcript) under the calls root, and removes the recorded
    /// folder. Returns nothing: the call is read back through the commands.
    #[track_caller]
    fn plant_ready(&self) {
        let recorded = fs::read_dir(&self.root)
            .expect("the calls root")
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .expect("a recorded call folder (call `record_one` first)");
        let target = self.root.join(READY_ID);
        fs::create_dir_all(&target).expect("ready folder");
        for file in ["mic.opus", "system.opus"] {
            fs::copy(recorded.join(file), target.join(file)).expect("copy track");
        }
        let mut meta: Value =
            serde_json::from_slice(&fs::read(recorded.join("meta.json")).expect("meta.json"))
                .expect("meta is JSON");
        meta["id"] = json!(READY_ID);
        meta["title"] = json!("Weekly sync");
        meta["started_at"] = json!("2026-10-29T10:00:00+03:00");
        meta["duration_ms"] = json!(3_600_000);
        meta["state"] = json!("ready");
        meta["failure"] = Value::Null;
        meta["stop_reason"] = Value::Null;
        meta["speakers"] = json!([
            {"id": "me", "name": "Me"},
            {"id": "s1", "name": "Other party"}
        ]);
        fs::write(target.join("meta.json"), meta.to_string()).expect("write meta");
        fs::write(target.join("transcript.json"), TRANSCRIPT_TEXT).expect("write transcript");
        fs::remove_dir_all(&recorded).expect("remove the recorded folder");
    }

    /// A damaged call: a folder with a track and no meta.json.
    fn plant_damaged(&self) {
        let dir = self.root.join(DAMAGED_ID);
        fs::create_dir_all(&dir).expect("damaged folder");
        fs::write(dir.join("mic.opus"), b"x").expect("track");
    }

    /// `export_bytes` of the ready call, the bytes copy and export share (core's own call).
    fn expected_bytes(&self, format: Format) -> Vec<u8> {
        let store = self
            .handle()
            .try_state::<Arc<CallStore>>()
            .expect("the call part manages the store");
        let calls = self.rig.service.snapshot().calls.clone();
        store
            .export_bytes(&calls, &self.root_text(), READY_ID, format)
            .unwrap_or_else(|e| panic!("export_bytes of the planted call: {e:?}"))
    }
}

/// Records one short call on a rig whose part was started over the tone source, so a recorded
/// folder exists to be turned into the ready call.
fn recorded_ipc() -> Ipc {
    recorded_with(None)
}

/// [`recorded_ipc`] with `clipboard` as the part's port (the fake one when `None`).
fn recorded_with(clipboard_port: Option<Arc<dyn Clipboard>>) -> Ipc {
    let source = call_shell_support::WavSource::new();
    let rig = CallRig::bare();
    let clipboard = Arc::new(FakeClipboard::new());
    let folder = Arc::new(FakeFolder::default());
    let save = Arc::new(FakeSave::default());
    rig.start_with(
        source.clone(),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
        clipboard_port.unwrap_or_else(|| clipboard.clone()),
        folder.clone(),
        save.clone(),
    );
    let root = rig.dir.path().join("calls");
    let ipc = Ipc {
        rig,
        clipboard,
        folder,
        save,
        root,
    };
    call::request(ipc.handle(), CallRequest::Record(CallSource::Switch));
    assert!(eventually(|| ipc.rig.recording()), "the call did not start");
    source.pump();
    ipc.rig.stop_call();
    assert!(eventually(|| !ipc.rig.recording()), "the call did not stop");
    ipc
}

/// Invokes `cmd` from `window`, from the window's own URL, like its page does.
fn invoke(window: &WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Result<Value, Value> {
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
fn ok(window: &WebviewWindow<MockRuntime>, cmd: &str, args: Value) -> Value {
    invoke(window, cmd, args).unwrap_or_else(|e| panic!("{cmd} rejected: {e}"))
}

/// Every payload of `event` heard through `target`, in order.
fn payloads(target: &impl Listener<MockRuntime>, event: &str) -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    target.listen(event, move |e| {
        let _ = tx.send(serde_json::from_str(e.payload()).expect("event payload is JSON"));
    });
    rx
}

/// [`payloads`] on the whole app: hears every emit, whatever its target.
fn any_payloads(app: &tauri::AppHandle<MockRuntime>, event: &str) -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    app.listen_any(event, move |e| {
        let _ = tx.send(serde_json::from_str(e.payload()).expect("event payload is JSON"));
    });
    rx
}

/// Moves what `rx` holds into `heard`.
fn drain(rx: &Receiver<Value>, heard: &mut Vec<Value>) {
    loop {
        match rx.try_recv() {
            Ok(v) => heard.push(v),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        }
    }
}

fn count_of(heard: &[Value], want: &Value) -> usize {
    heard.iter().filter(|v| *v == want).count()
}

// ---- the window ---------------------------------------------------------------------------

#[test]
fn the_tray_calls_item_opens_one_calls_window_and_a_second_click_no_second() {
    // Acceptance 1 (I2, T054): a click on "Calls" makes the `calls` window at `/calls` through the
    // opener thread, and the caller does not wait for it. A second click makes no second window
    // (the count of windows stays) and the request is still counted. Bite: `open_calls` left a
    // counter (no window); a window built per request (a duplicate-label error, or a second
    // window); the window built on the caller's thread.
    let _serial = serial();
    let ipc = ipc();
    assert_eq!(ipc.handle().get_webview_window(LABEL).map(|_| ()), None);

    tray::on_menu_event(ipc.handle(), menu_item(TrayAction::OpenCalls));
    let window = ipc.wait_window();
    assert_eq!(window.url().expect("url").path(), "/calls");
    let with_calls = ipc.windows();

    tray::on_menu_event(ipc.handle(), menu_item(TrayAction::OpenCalls));
    assert!(
        eventually(|| call::calls_opened(ipc.handle()) == 2),
        "the second click did not reach open_calls"
    );
    assert!(
        holds_for_a_while(|| ipc.windows() == with_calls),
        "a second click made another window"
    );
}

#[test]
fn an_open_with_a_target_on_a_fresh_window_emits_calls_open_once_after_the_first_list() {
    // Acceptance 1 (I2, T055): `open_calls(Some({root,id}))` on a window that does not exist yet
    // delivers `calls://open {root,id}` exactly once, and not before the page's listeners exist:
    // the page sends its first `calls_list` only after registering them, so nothing is emitted
    // while the window is merely built, the event comes after that first list, and a second list
    // adds none. The subscription is made first and the payload matched (F-018). Bite: emit
    // right after the window is created (heard before the first list); the pending target never
    // emitted; emitted on every `calls_list`; emitted twice (`emit` and `emit_to`).
    let _serial = serial();
    let ipc = ipc();
    let want = json!({ "root": ipc.root_text(), "id": READY_ID });
    let heard_any = any_payloads(ipc.handle(), OPEN);

    let window = ipc.open_window(Some(ipc.reference(READY_ID)));
    let mut heard = Vec::new();
    assert!(
        holds_for_a_while(|| {
            drain(&heard_any, &mut heard);
            heard.is_empty()
        }),
        "calls://open was emitted before the page's first calls_list: {heard:?}"
    );

    ok(&window, "calls_list", json!({}));
    assert!(
        eventually(|| {
            drain(&heard_any, &mut heard);
            count_of(&heard, &want) >= 1
        }),
        "no calls://open {want} after the first calls_list; heard {heard:?}"
    );
    ok(&window, "calls_list", json!({}));
    assert!(
        holds_for_a_while(|| {
            drain(&heard_any, &mut heard);
            heard == vec![want.clone()]
        }),
        "calls://open must be heard exactly once: {heard:?}"
    );
}

#[test]
fn an_open_with_a_target_on_a_listening_window_emits_at_once_and_a_bare_open_none() {
    // Acceptance 1 (I2, T055): when the window already listed (its listeners exist), a targeted
    // open emits exactly one `calls://open` without another list, and creates no second window;
    // an open without a target emits nothing. Bite: a pending target that waits for a list that
    // never comes again; a bare open that emits an empty target; a second window.
    let _serial = serial();
    let ipc = ipc();
    let window = ipc.open_window(None);
    ok(&window, "calls_list", json!({}));
    let windows = ipc.windows();
    let heard_any = any_payloads(ipc.handle(), OPEN);
    let mut heard = Vec::new();

    call::open_calls(ipc.handle(), None);
    assert!(
        eventually(|| call::calls_opened(ipc.handle()) == 2),
        "the bare open was not posted"
    );
    let want = json!({ "root": ipc.root_text(), "id": READY_ID });
    call::open_calls(ipc.handle(), Some(ipc.reference(READY_ID)));
    assert!(
        eventually(|| {
            drain(&heard_any, &mut heard);
            count_of(&heard, &want) >= 1
        }),
        "no calls://open for the warm window; heard {heard:?}"
    );
    assert!(
        holds_for_a_while(|| {
            drain(&heard_any, &mut heard);
            heard == vec![want.clone()] && ipc.windows() == windows
        }),
        "exactly one calls://open and no new window: {heard:?}"
    );
}

// ---- the five commands --------------------------------------------------------------------

#[test]
fn list_and_get_answer_the_wire_fixture_shapes_for_a_planted_call() {
    // Acceptance 2 (T054, P-010): `calls_list` and `calls_get` of a planted ready call equal
    // `e2e/fixtures/calls-wire.json` (the fixture's ready entry and detail, root being the
    // test's), a damaged folder lists as `damaged` with `durationMs: null`, and a missing call
    // is the fixture's `notFound`. Bite: a field renamed or snake_cased, a state spelled another
    // way, `failReason` serialised as null, a command that answers its own shape instead of
    // core's builders.
    let _serial = serial();
    let ipc = recorded_ipc();
    ipc.plant_ready();
    let window = ipc.open_window(None);
    let fixture = fixture();
    let root = ipc.root_text();

    let mut ready = fixture["list"]["calls"][3].clone();
    ready["root"] = json!(root);
    assert_eq!(
        ready["id"],
        json!(READY_ID),
        "premise: the fixture's ready entry"
    );
    assert_eq!(
        ok(&window, "calls_list", json!({})),
        json!({ "calls": [ready.clone()], "unavailableRoots": [] })
    );

    let mut detail = fixture["detail"].clone();
    detail["summary"]["root"] = json!(root);
    assert_eq!(
        ok(
            &window,
            "calls_get",
            json!({ "root": root, "id": READY_ID })
        ),
        detail
    );
    assert_eq!(
        invoke(
            &window,
            "calls_get",
            json!({ "root": root, "id": "2026-10-01_000000" })
        ),
        Err(fixture["errors"]["notFound"].clone()),
        "a gone call"
    );
}

#[test]
fn a_foreign_root_or_a_bad_id_is_not_found_and_no_port_is_touched() {
    // Acceptance 2 (I3, P-009, T054): every command answers the fixture's `notFound` for a root
    // that settings do not list, a real folder included, and for an id outside the grammar, and
    // the fake opener, clipboard and save dialog are never called (the dialog is not even
    // shown). Bite: a command that joins the wire root as a path; one that checks the disk
    // before the guard; the dialog shown before the guard; the opener called on a guess.
    let _serial = serial();
    let ipc = recorded_ipc();
    ipc.plant_ready();
    let window = ipc.open_window(None);
    let stranger = TempDir::new();
    fs::create_dir(stranger.path().join(READY_ID)).expect("stranger call folder");
    let not_found = fixture()["errors"]["notFound"].clone();
    let foreign = wire_root(stranger.path());
    let root = ipc.root_text();

    let cases = [
        json!({ "root": foreign, "id": READY_ID }),
        json!({ "root": root, "id": ".." }),
        json!({ "root": root, "id": "../calls" }),
        json!({ "root": "", "id": READY_ID }),
    ];
    for args in &cases {
        for cmd in ["calls_get", "calls_copy_markdown", "calls_open_folder"] {
            assert_eq!(
                invoke(&window, cmd, args.clone()),
                Err(not_found.clone()),
                "{cmd} {args}"
            );
        }
        let mut export = args.clone();
        export["format"] = json!("md");
        assert_eq!(
            invoke(&window, "calls_export", export),
            Err(not_found.clone()),
            "{args}"
        );
    }
    assert_eq!(
        ipc.folder.opened(),
        Vec::<PathBuf>::new(),
        "the opener was called"
    );
    assert_eq!(
        ipc.clipboard.texts(),
        Vec::<String>::new(),
        "the clipboard was written"
    );
    assert_eq!(ipc.save.asked(), vec![], "the save dialog was shown");
}

#[test]
fn open_folder_opens_the_guarded_folder_also_of_a_damaged_call() {
    // Acceptance 2 (I3, FR-033, T054): `calls_open_folder` answers null and the fake opener gets
    // exactly `<root>/<id>` for a ready call and for a damaged one (FR-033 offers it on damaged
    // rows); a gone call is `notFound` and adds no open. Bite: the folder taken from a lookup
    // that refuses a damaged call (`ioFailed`); the opener given the root or the wire text.
    let _serial = serial();
    let ipc = recorded_ipc();
    ipc.plant_ready();
    ipc.plant_damaged();
    let window = ipc.open_window(None);
    let root = ipc.root_text();

    for id in [READY_ID, DAMAGED_ID] {
        assert_eq!(
            ok(
                &window,
                "calls_open_folder",
                json!({ "root": root, "id": id })
            ),
            Value::Null,
            "{id}"
        );
    }
    assert_eq!(
        ipc.folder.opened(),
        vec![ipc.root.join(READY_ID), ipc.root.join(DAMAGED_ID)]
    );
    assert_eq!(
        invoke(
            &window,
            "calls_open_folder",
            json!({ "root": root, "id": "2026-10-01_000000" })
        ),
        Err(fixture()["errors"]["notFound"].clone())
    );
    assert_eq!(ipc.folder.opened().len(), 2, "a gone call opened nothing");
}

#[test]
fn an_app_without_a_call_part_answers_an_empty_list_and_not_found() {
    // T-142 analysis (open design points): `build_app` alone manages no call part; a `calls`
    // window made by hand there gets an empty list and `notFound`, not a panic or a refusal of
    // every invoke (`State<..>` would refuse). Bite: the commands taking `State<Arc<CallStore>>`.
    let _serial = serial();
    let rig = CallRig::bare();
    let window = WebviewWindowBuilder::new(rig.handle(), LABEL, WebviewUrl::App("calls".into()))
        .build()
        .expect("a calls window");
    assert_eq!(
        ok(&window, "calls_list", json!({})),
        json!({ "calls": [], "unavailableRoots": [] })
    );
    assert_eq!(
        invoke(
            &window,
            "calls_get",
            json!({ "root": "C:\\Calls", "id": READY_ID })
        ),
        Err(fixture()["errors"]["notFound"].clone())
    );
}

// ---- copy ---------------------------------------------------------------------------------

#[test]
fn copy_writes_the_markdown_once_through_the_one_clipboard_port() {
    // Acceptance 4 (I4, R18, T054): `calls_copy_markdown` answers null and the clipboard port the
    // part was given receives exactly one `set_text_excluded_from_history`, with exactly
    // `CallStore::export_bytes(Md)` of the call. A clipboard that fails is `ioFailed` and the
    // call is still one write (no retry path of its own, no second writer); a damaged call (no
    // transcript) is `ioFailed` and writes nothing. Bite: a second clipboard writer in the call
    // part, the text built by another renderer, an error swallowed into null.
    let _serial = serial();
    let ipc = recorded_ipc();
    ipc.plant_ready();
    ipc.plant_damaged();
    let window = ipc.open_window(None);
    let root = ipc.root_text();
    let md = String::from_utf8(ipc.expected_bytes(Format::Md)).expect("Markdown is UTF-8");
    assert!(
        md.contains("Hello there") && md.contains("Weekly sync"),
        "premise: {md}"
    );

    assert_eq!(
        ok(
            &window,
            "calls_copy_markdown",
            json!({ "root": root, "id": READY_ID })
        ),
        Value::Null
    );
    assert_eq!(ipc.clipboard.texts(), vec![md.clone()]);

    ipc.clipboard.set_fail(true);
    let failed = invoke(
        &window,
        "calls_copy_markdown",
        json!({ "root": root, "id": READY_ID }),
    )
    .expect_err("a failing clipboard is an error");
    assert_eq!(failed["code"], json!("ioFailed"), "{failed}");
    assert_eq!(
        ipc.clipboard.texts(),
        vec![md.clone(), md],
        "one more write, no retry"
    );
    ipc.clipboard.set_fail(false);

    let damaged = invoke(
        &window,
        "calls_copy_markdown",
        json!({ "root": root, "id": DAMAGED_ID }),
    )
    .expect_err("a damaged call cannot be copied");
    assert_eq!(damaged["code"], json!("ioFailed"), "{damaged}");
    assert_eq!(
        ipc.clipboard.texts().len(),
        2,
        "a damaged call wrote nothing"
    );
}

#[test]
fn copy_through_the_real_clipboard_carries_the_text_and_the_three_exclusion_formats() {
    // Acceptance 4 (I4, FR-10/FR-022, T054): with the real `WinClipboard` as the call part's
    // port, the clipboard holds the Markdown as CF_UNICODETEXT and the three FR-10 formats
    // (`ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory` = 0,
    // `CanUploadToCloudClipboard` = 0), read back with the test's own reader (tests/clipboard.rs).
    // Bite: a call-part writer that sets the text only (e.g. `arboard`-style); a format missing.
    let _serial = serial();
    put_text("an earlier owner's text");
    let ipc = recorded_with(Some(Arc::new(WinClipboard::new())));
    ipc.plant_ready();
    let window = ipc.open_window(None);
    let md = String::from_utf8(ipc.expected_bytes(Format::Md)).expect("Markdown is UTF-8");

    assert_eq!(
        ok(
            &window,
            "calls_copy_markdown",
            json!({ "root": ipc.root_text(), "id": READY_ID })
        ),
        Value::Null
    );
    assert_eq!(
        read_clipboard(),
        ClipboardState {
            text: Some(md),
            exclude_from_monitor: true,
            can_include_in_history: Some(0),
            can_upload_to_cloud: Some(0),
        }
    );
}

// ---- export -------------------------------------------------------------------------------

#[test]
fn a_cancelled_save_dialog_returns_saved_false_and_writes_nothing() {
    // Acceptance 3 (failure branch, I5, T054): the dialog is asked once (Markdown), answers
    // "cancelled" (`None`), and `calls_export` answers `{saved: false}`; the target folder and
    // the calls root are byte-for-byte what they were, no `*.voicen-tmp` anywhere. Bite: a
    // cancel turned into an error or into `{saved: true}`; `export_to` called with a default
    // target (a file or a tmp appears); the bytes written to the root.
    let _serial = serial();
    let ipc = recorded_ipc();
    ipc.plant_ready();
    let window = ipc.open_window(None);
    let out = TempDir::new();
    let listing = |dir: &Path| {
        let mut v: Vec<String> = fs::read_dir(dir)
            .expect("list")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    let (root_before, out_before) = (listing(&ipc.root), listing(out.path()));
    ipc.save.answer(None);

    assert_eq!(
        ok(
            &window,
            "calls_export",
            json!({ "root": ipc.root_text(), "id": READY_ID, "format": "md" })
        ),
        json!({ "saved": false })
    );
    let asked = ipc.save.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].1, Format::Md);
    assert!(!asked[0].0.is_empty(), "a suggested file name");
    assert_eq!(listing(out.path()), out_before, "the target folder changed");
    assert_eq!(listing(&ipc.root), root_before, "the calls root changed");
    assert_eq!(Ipc::tmp_files(&ipc.root), Vec::<PathBuf>::new());
    assert_eq!(Ipc::tmp_files(out.path()), Vec::<PathBuf>::new());
}

#[test]
fn a_picked_target_gets_exactly_the_export_bytes_of_the_format() {
    // Acceptance 2 (I5, T054): a pick of `<out>/a.md` writes `export_bytes(Md)`, a pick of
    // `<out>/b.json` writes `export_bytes(Json)` (the asked format reaches the dialog and the
    // bytes), each answers `{saved: true}`, and nothing but the two targets is in `out`. A
    // target in a folder that is not there is `ioFailed` and leaves nothing (no folder, no tmp).
    // Bite: the format ignored (Markdown for json); the bytes written outside `export_to` (a tmp
    // left); an error turned into `{saved: true}`.
    let _serial = serial();
    let ipc = recorded_ipc();
    ipc.plant_ready();
    let window = ipc.open_window(None);
    let out = TempDir::new();
    let root = ipc.root_text();

    for (name, format, wire) in [("a.md", Format::Md, "md"), ("b.json", Format::Json, "json")] {
        ipc.save.answer(Some(out.path().join(name)));
        assert_eq!(
            ok(
                &window,
                "calls_export",
                json!({ "root": root, "id": READY_ID, "format": wire })
            ),
            json!({ "saved": true }),
            "{name}"
        );
        assert_eq!(
            fs::read(out.path().join(name)).expect("the target"),
            ipc.expected_bytes(format),
            "{name}"
        );
        assert_eq!(ipc.save.asked().last().map(|a| a.1), Some(format));
    }
    let mut names: Vec<String> = fs::read_dir(out.path())
        .expect("list")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["a.md".to_string(), "b.json".to_string()]);

    ipc.save
        .answer(Some(out.path().join("missing").join("c.md")));
    let failed = invoke(
        &window,
        "calls_export",
        json!({ "root": root, "id": READY_ID, "format": "md" }),
    )
    .expect_err("a target in a missing folder fails");
    assert_eq!(failed["code"], json!("ioFailed"), "{failed}");
    assert!(!out.path().join("missing").exists(), "no folder was made");
    assert_eq!(Ipc::tmp_files(out.path()), Vec::<PathBuf>::new());
}

// ---- calls://changed ----------------------------------------------------------------------

#[test]
fn a_start_and_a_stop_emit_one_changed_each_to_the_calls_window_only() {
    // Acceptance 2 (I6, T054): through `call::request`, a start emits one `calls://changed
    // {root, id}` of the new call's folder and the stop one more, heard by the `calls` window and
    // not by the `settings` window (`emit_to`, not `emit`); the app-wide listener hears exactly
    // two. Subscribed before the action, matched on `{root, id}` (F-018); the waits are on the
    // session state and the events (F-031). Bite: no emit at the start or at a stop; `app.emit`
    // to every window; the id of another call; an emit under the session lock that waits for the
    // listener; two emits per stop.
    let _serial = serial();
    let source = call_shell_support::WavSource::new();
    let rig = CallRig::bare();
    rig.start(
        source.clone(),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
    );
    let root = rig.dir.path().join("calls");
    let calls_window = {
        call::open_calls(rig.handle(), None);
        assert!(
            eventually(|| rig.handle().get_webview_window(LABEL).is_some()),
            "no calls window"
        );
        rig.handle()
            .get_webview_window(LABEL)
            .expect("calls window")
    };
    settings_window::open(rig.handle(), SettingsTab::Engine, None).expect("settings window");
    let settings = rig
        .handle()
        .get_webview_window(settings_window::LABEL)
        .expect("settings window");
    let on_calls = payloads(&calls_window, CHANGED);
    let on_settings = payloads(&settings, CHANGED);
    let on_app = any_payloads(rig.handle(), CHANGED);
    let (mut calls_heard, mut settings_heard, mut app_heard) = (Vec::new(), Vec::new(), Vec::new());

    call::request(rig.handle(), CallRequest::Record(CallSource::Switch));
    assert!(eventually(|| rig.recording()), "the call did not start");
    let id = fs::read_dir(&root)
        .expect("the calls root")
        .flatten()
        .find(|e| e.path().is_dir())
        .expect("the call folder")
        .file_name()
        .to_string_lossy()
        .into_owned();
    let want = json!({ "root": wire_root(&root), "id": id });
    assert!(
        eventually(|| {
            drain(&on_calls, &mut calls_heard);
            count_of(&calls_heard, &want) >= 1
        }),
        "no calls://changed {want} at the start; heard {calls_heard:?}"
    );

    source.pump();
    rig.stop_call();
    assert!(eventually(|| !rig.recording()), "the call did not stop");
    assert!(
        eventually(|| {
            drain(&on_calls, &mut calls_heard);
            count_of(&calls_heard, &want) >= 2
        }),
        "no second calls://changed at the stop; heard {calls_heard:?}"
    );
    assert!(
        holds_for_a_while(|| {
            drain(&on_calls, &mut calls_heard);
            drain(&on_settings, &mut settings_heard);
            drain(&on_app, &mut app_heard);
            count_of(&calls_heard, &want) == 2 && settings_heard.is_empty()
        }),
        "calls heard {calls_heard:?}, settings heard {settings_heard:?}: want {want} twice and \
         nothing for settings"
    );
    assert_eq!(
        app_heard,
        vec![want.clone(), want],
        "the app-wide listener hears exactly the two emits"
    );
}
