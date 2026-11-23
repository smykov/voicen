//! T-145 (spec 007 FR-007, FR-008, FR-017; research R14; quickstart 4-6; invariants I1-I5 of
//! docs/tasks/T-145.md): how a call recording ends when the app ends (tray Exit, RunEvent::Exit,
//! WM_ENDSESSION and the installer's Restart Manager close, which is the same path), when the
//! machine sleeps, and how a call that did not end cleanly is read back at the next start.
//! Windows CI only (decision #5): the Linux gate only compile-checks this file, and every test
//! here is red on `main` (it does not compile: the names below do not exist yet), so the red
//! observation is the Windows CI run of `wip/T-145`.
//!
//! API pinned for the developer (`analysis.seam`; the developer may not rename it, the tests
//! follow the seam):
//!
//! ```text
//! voicen_lib::call::
//!   CallRequest::{Exit, ExitConfirmed, Sleep, Resumed}   // plus Record, Stop, RefuseSwitch
//!   trait ExitConfirm: Send + Sync { fn ask(&self, text: &str) -> bool }   // true = Yes
//!   CallPorts { .., exit_confirm: Arc<dyn ExitConfirm> }  // the question's port (SaveDialog pattern);
//!                                                       // `text` = `exit.recording.question` in the UI
//!                                                       // language
//!   finalize_on_exit<R: Runtime>(app: &AppHandle<R>)     // run by `on_run_event`'s Exit arm before the
//!                                                       // session end: gate -> Exiting, then
//!                                                       // `CallSession::stop(SystemEnd)`; no-op without
//!                                                       // a call part (the tests reach it through
//!                                                       // `voicen_lib::on_run_event`)
//! win::hotkey CallPort gains on_suspend / on_resume       // reached through the hotkey window's
//!                                                       // WM_POWERBROADCAST (suspend; resume only, not
//!                                                       // unlock): `call::request(Sleep | Resumed)`
//! start_calls: the call worker's first job is `CallStore::recover` over the one managed store; every
//!   `Recovery.queue` folder goes to `PassQueue::enqueue`, oldest first
//! ```
//!
//! How the tests observe things (all on state, none on display text):
//! - the meta (`CallMeta`: state, stop_reason) and the track scans (`ReadEnd::Complete`) of the call
//!   folder;
//! - "enqueued" = the folder's meta leaves `ready-to-transcribe`: the rig has no engine key, so the
//!   queued pass runs and ends `failed` (`engine_not_set_up`). "Nothing enqueued" = the meta stays
//!   `ready-to-transcribe` for a bounded window (a stall can only make that pass);
//! - the one shared `FakeNotifier` (offers) and the log (`call_stopped`, `call_recovered`, `exited`).
//!
//! The 2 s budget of the exit stop (Acceptance 2): `timing_support` has no ceiling on a measured time
//! (T-201: a clock reading decides only a lower bound), so no test asserts one. The time of
//! `on_run_event`'s `RunEvent::Exit` arm is measured and printed (`finalize took ...`; the R14 record
//! cites it from the CI log), and what the 2 s protects is asserted as an outcome: in the
//! WM_ENDSESSION child the app ends inside the handler, and the call is finalized and its
//! `call_stopped` line written before the `exited` line.
//!
//! Three stages: a real Wry loop for the Exit paths (the `lifecycle.rs` pattern: `run_return`, a
//! driver thread, the loop only ended from a main-thread task), the hotkey thread of `AppRig` for
//! the power events (the `call_hotkey.rs` pattern) and child processes (the `crash.rs` pattern)
//! for WM_ENDSESSION and the forced kill. All tests share one serial lock (one tray, one set of
//! hotkeys, one WebView2 profile at a time).
#![cfg(windows)]

mod call_shell_support;
mod timing_support;
mod win32_support;

use std::ffi::{c_void, OsString};
use std::fs::{self, File};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use call_shell_support::{
    eventually as pumping_eventually, plain_opener, CallRig, FakeExit, FakeFolder, FakeSave,
    WavSource,
};
use tauri::menu::{MenuEvent, MenuId};
use tauri::test::{mock_context, noop_assets};
use tauri::{App, AppHandle, Context, Manager, RunEvent, Wry};
use timing_support::HANG;
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::call::codec::{scan, ReadEnd};
use voicen_core::call::session::CallSession;
use voicen_core::call::storage::write_meta;
use voicen_core::call::{CallMeta, CallSource, CallState, StopReason};
use voicen_core::clock::SystemMonotonic;
use voicen_core::diag::Log;
use voicen_core::i18n::{self, MessageId, EXIT_RECORDING_QUESTION, NOTICE_STOPPED_SLEEP};
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::platform::{
    FakeAudioSource, FakeClipboard, FakeIndicator, FakeNotifier, FakePaster,
};
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::LoadOutcome;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::tray::TrayAction;
use voicen_lib::call::{self, CallPorts, CallRequest};
use voicen_lib::dictation::{start_dictation_with, DictationHandle, DictationPorts};
use voicen_lib::settings_ipc::load_settings;
use voicen_lib::tray;
use win32_support::{
    engine_factory, eventually, headed, holds_for, log_lines, serial, AppRig, TextEngine, SETTLE,
};
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetClassNameW, GetWindowThreadProcessId, SendMessageW,
    PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND, WM_ENDSESSION, WM_POWERBROADCAST, WM_QUERYENDSESSION,
    WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};

/// A test identifier: WebView2's profile goes under it, not under the release one.
const IDENTIFIER: &str = "dev.voicen.test.call-lifecycle";

/// The exe ends with code 3 if one test's loop has not returned by then.
const WATCHDOG: Duration = Duration::from_secs(180);
/// How long the driver waits for a step that is not a window build (the loop ending).
const BUDGET: Duration = Duration::from_secs(30);
/// How long a main-thread task may take on a running loop.
const ROUND_TRIP: Duration = Duration::from_secs(5);
/// The fallback code of a loop that was still running after the exit: the test refuses it.
const FALLBACK_CODE: i32 = 70;
/// A bounded negative: how long "nothing happens" is watched.
const QUIET: Duration = Duration::from_millis(500);
/// A child that ends itself must be gone by then.
const CHILD_BUDGET: Duration = Duration::from_secs(90);

const MODE_VAR: &str = "VOICEN_LIFECYCLE_CHILD";
const DIR_VAR: &str = "VOICEN_LIFECYCLE_DIR";
const TAO_TARGET: &str = "Tao Thread Event Target";

/// The question the tray's Exit shows, in the language the settings carry (English by default).
fn question(lang: i18n::UiLanguage) -> String {
    i18n::text(lang, EXIT_RECORDING_QUESTION, &[])
}

// ---- the call folder ------------------------------------------------------------------------

/// The one call folder under `root`.
#[track_caller]
fn only_folder(root: &Path) -> PathBuf {
    let folders: Vec<PathBuf> = fs::read_dir(root)
        .expect("the calls root")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(
        folders.len(),
        1,
        "one call folder under the root: {folders:?}"
    );
    folders.into_iter().next().expect("one folder")
}

#[track_caller]
fn meta_of(dir: &Path) -> CallMeta {
    serde_json::from_slice(&fs::read(dir.join("meta.json")).expect("meta.json"))
        .expect("meta.json parses")
}

/// Both track files end in their EOS page (a finished stream, not a cut one).
#[track_caller]
fn assert_tracks_complete(dir: &Path) {
    for file in ["mic.opus", "system.opus"] {
        let scanned = scan(File::open(dir.join(file)).expect(file)).expect("a readable track");
        assert_eq!(
            scanned.end,
            ReadEnd::Complete,
            "{file} is finished (EOS written)"
        );
    }
}

/// The index of the first log line whose head is `head`.
fn head_index(lines: &[String], head: &str) -> Option<usize> {
    lines
        .iter()
        .position(|l| l.split_whitespace().nth(2) == Some(head))
}

// ---- the real-runtime app --------------------------------------------------------------------

/// Ends the exe with code 3 unless dropped within `WATCHDOG`.
struct Watchdog(Option<mpsc::Sender<()>>);

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.0.take();
    }
}

fn watchdog(test: &'static str) -> Watchdog {
    let (tx, rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        if let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(WATCHDOG) {
            eprintln!("call_lifecycle watchdog: `{test}`: the loop did not return; ending the exe");
            std::process::exit(3);
        }
    });
    Watchdog(Some(tx))
}

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn no_autostart() -> Arc<dyn Autostart> {
    Arc::new(FakeAutostart::new())
}

fn idle_models(data: &Path) -> Arc<LocalModels> {
    let (models, cleanup) = LocalModels::open(
        data.join("models"),
        FakeDisk::with_available(u64::MAX),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup over a missing models dir");
    Arc::new(models)
}

/// The app as `run()` builds it (`build_app`) on the real Wry runtime, allowed to run on this
/// (libtest) thread, after an earlier start so `Ready` opens no window; its log in `<dir>/logs`.
fn wry_app_in(dir: &Path, log: Arc<Log>) -> (App<Wry>, LoadOutcome) {
    let load = |log: &Log| {
        load_settings(
            dir.to_path_buf(),
            no_keys(),
            no_autostart(),
            Arc::new(ModelStore::new(dir.join("models"), MODELS)),
            None,
            log,
        )
    };
    let _ = load(&log);
    let (service, outcome) = load(&log);
    assert!(
        matches!(outcome, LoadOutcome::Loaded(_)),
        "premise: a loaded start: {outcome:?}"
    );
    let mut context: Context<Wry> = mock_context(noop_assets());
    context.config_mut().identifier = IDENTIFIER.to_string();
    let app = voicen_lib::build_app(
        tauri::Builder::default().any_thread(),
        context,
        service,
        idle_models(dir),
        log,
    )
    .expect("the Wry app builds");
    (app, outcome)
}

fn wry_app(dir: &Path) -> (App<Wry>, LoadOutcome) {
    wry_app_in(
        dir,
        voicen_lib::diag::start(dir.join("logs"), Box::new(|_| {})),
    )
}

/// The call part over `source`, a temp calls root and the given fakes (what `run()` starts, with
/// the platform ports replaced).
fn start_call_part(
    handle: &AppHandle<Wry>,
    root: PathBuf,
    source: Arc<WavSource>,
    exit: Arc<FakeExit>,
    notifier: Arc<FakeNotifier>,
) {
    fs::create_dir_all(&root).expect("the calls root");
    call::start_calls(
        handle,
        CallPorts {
            source,
            disk: FakeDisk::with_available(100_000_000_000),
            mono: Arc::new(SystemMonotonic),
            open_track: plain_opener(),
            notifier,
            calls_root: root,
            clipboard: Arc::new(FakeClipboard::new()),
            folder: Arc::new(FakeFolder::default()),
            save: Arc::new(FakeSave::default()),
            credentials: no_keys(),
            detector: None,
            exit_confirm: exit,
        },
    )
    .expect("the call part starts");
}

/// What a Wry test holds: its data dir, the fakes and the call part's root.
struct Wc {
    dir: TempDir,
    source: Arc<WavSource>,
    exit: Arc<FakeExit>,
    notifier: Arc<FakeNotifier>,
}

impl Wc {
    fn root(&self) -> PathBuf {
        self.dir.path().join("calls")
    }

    fn lines(&self) -> Vec<String> {
        log_lines(&self.dir.path().join("logs"))
    }

    /// The `call_stopped` lines, once the writer has flushed `want` of them.
    fn stopped(&self, want: usize) -> Vec<String> {
        timing_support::eventually(|| headed(&self.lines(), "call_stopped").len() >= want);
        headed(&self.lines(), "call_stopped")
    }
}

/// A Wry app with the call part started, and the handle's fakes.
fn wry_call_app() -> (Wc, App<Wry>, LoadOutcome) {
    let dir = TempDir::new();
    let (app, outcome) = wry_app(dir.path());
    let wc = Wc {
        dir,
        source: WavSource::new(),
        exit: Arc::new(FakeExit::default()),
        notifier: Arc::new(FakeNotifier::new()),
    };
    start_call_part(
        app.handle(),
        wc.root(),
        wc.source.clone(),
        wc.exit.clone(),
        wc.notifier.clone(),
    );
    (wc, app, outcome)
}

/// Runs the loop with `on_run_event` (as `run()` does), measures the `RunEvent::Exit` arm and
/// prints it, and starts `driver` on its own thread once `Ready` was handled. Returns
/// `run_return`'s code and the driver's report.
fn run_with_driver<T: Send + 'static>(
    app: App<Wry>,
    outcome: LoadOutcome,
    driver: impl FnOnce(AppHandle<Wry>) -> T + Send + 'static,
) -> (i32, Option<T>) {
    let (tx, rx) = mpsc::channel();
    let mut driver = Some(driver);
    let code = app.run_return(move |handle, event| {
        let ready = matches!(event, RunEvent::Ready);
        if matches!(event, RunEvent::Exit) {
            let (_, took) = timing_support::measure(|| {
                voicen_lib::on_run_event(handle, event, &outcome, false);
            });
            eprintln!("call_lifecycle: finalize took {took:?} (RunEvent::Exit arm)");
            return;
        }
        voicen_lib::on_run_event(handle, event, &outcome, false);
        if ready {
            if let Some(driver) = driver.take() {
                let handle = handle.clone();
                let tx = tx.clone();
                thread::spawn(move || {
                    let _ = tx.send(driver(handle));
                });
            }
        }
    });
    (code, rx.recv_timeout(BUDGET).ok())
}

/// Ends the loop with `code` from inside a main-thread task (never `AppHandle::exit` from a
/// thread on a loop that may have ended: tauri then falls back to `process::exit`).
fn end_loop(handle: &AppHandle<Wry>, code: i32) {
    let inner = handle.clone();
    let _ = handle.run_on_main_thread(move || inner.exit(code));
}

/// Ends the loop with the fallback code after `BUDGET` (the test refuses that code).
fn fallback_end(handle: &AppHandle<Wry>) {
    let handle = handle.clone();
    thread::spawn(move || {
        thread::sleep(BUDGET);
        end_loop(&handle, FALLBACK_CODE);
    });
}

/// True when a task posted to the main thread ran within `ROUND_TRIP`: the loop is running.
fn main_round_trip(handle: &AppHandle<Wry>) -> bool {
    let (tx, rx) = mpsc::channel();
    if handle
        .run_on_main_thread(move || {
            let _ = tx.send(());
        })
        .is_err()
    {
        return false;
    }
    rx.recv_timeout(ROUND_TRIP).is_ok()
}

/// The tray's Exit item, handled on the main thread like a real menu event.
fn post_tray_exit(handle: &AppHandle<Wry>) -> bool {
    let inner = handle.clone();
    handle
        .run_on_main_thread(move || {
            tray::on_menu_event(
                &inner,
                MenuEvent {
                    id: MenuId::new(TrayAction::Exit.as_str()),
                },
            );
        })
        .is_ok()
}

/// Starts a tray-style call and plays the tone into it; `None` when it did not start.
fn record(handle: &AppHandle<Wry>, source: &WavSource) -> Option<Arc<CallSession>> {
    call::request(handle, CallRequest::Record(CallSource::Tray));
    let session = handle
        .try_state::<Arc<CallSession>>()
        .map(|s| Arc::clone(&s))?;
    let started = timing_support::eventually(|| session.try_is_recording() == Some(true))
        && timing_support::eventually(|| source.opens() == 2);
    if started {
        source.pump();
        Some(session)
    } else {
        None
    }
}

#[derive(Debug, Default)]
struct NoReport {
    recorded: bool,
    asked: Vec<String>,
    one_ask_kept: bool,
    still_recording: bool,
    meta_state: Option<CallState>,
    loop_alive: bool,
}

#[test]
fn tray_exit_while_recording_asks_and_no_keeps_recording() {
    // Acceptance 1, I3: the tray's Exit item while a call records asks once (the catalog text
    // `exit.recording.question`); the answer No changes nothing: the call still records, its
    // meta is still `recording`, no second ask, the loop is still running and the main thread
    // answers. Bite: Exit still `app.exit(0)` (the loop ends, no ask), no ask, a stop on No,
    // an ask per click or per poll, the dialog shown on the main thread (the round trip hangs).
    let _serial = serial();
    let _watchdog = watchdog("tray_exit_while_recording_asks_and_no_keeps_recording");
    let (wc, app, outcome) = wry_call_app();
    let (exit, source, root) = (wc.exit.clone(), wc.source.clone(), wc.root());
    exit.answer(false);

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let mut report = NoReport::default();
        let Some(session) = record(&handle, &source) else {
            end_loop(&handle, 0);
            return report;
        };
        report.recorded = true;
        let posted = post_tray_exit(&handle);
        timing_support::eventually(|| posted && !exit.asked().is_empty());
        report.one_ask_kept = timing_support::holds_for(QUIET, || exit.asked().len() == 1);
        report.asked = exit.asked();
        report.still_recording = session.try_is_recording() != Some(false);
        report.meta_state = Some(meta_of(&only_folder(&root)).state);
        report.loop_alive = main_round_trip(&handle);
        end_loop(&handle, 0);
        report
    });

    let report = report.expect("the driver sent no report");
    assert!(report.recorded, "premise: the call started: {report:?}");
    assert_eq!(
        report.asked,
        vec![question(i18n::UiLanguage::En)],
        "one ask with the catalog question: {report:?}"
    );
    assert!(report.one_ask_kept, "a second ask appeared: {report:?}");
    assert!(report.still_recording, "No stopped the call: {report:?}");
    assert_eq!(report.meta_state, Some(CallState::Recording), "{report:?}");
    assert!(report.loop_alive, "the loop stopped after No: {report:?}");
    assert_eq!(code, 0, "run_return code after the test's own end_loop(0)");
}

#[test]
fn tray_exit_yes_stops_with_exit_then_ends_the_loop() {
    // Acceptance 1, I1, I2: Yes stops the call with `Exit` (meta `ready-to-transcribe`,
    // stop_reason `exit`, both tracks finished, one `call_stopped reason=exit`), then the loop
    // ends with code 0; from the confirmed exit on the call shell offers no notice and enqueues
    // no pass (the meta stays `ready-to-transcribe`). Bite: Yes only exits (the call stays
    // `recording`), the stop reason not Exit, no `app.exit` after the stop (the fallback code),
    // the stop deferred to RunEvent::Exit (reason system_end), a notice or an enqueue on the
    // way out.
    let _serial = serial();
    let _watchdog = watchdog("tray_exit_yes_stops_with_exit_then_ends_the_loop");
    let (wc, app, outcome) = wry_call_app();
    let (exit, source) = (wc.exit.clone(), wc.source.clone());
    exit.answer(true);

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let recorded = record(&handle, &source).is_some();
        if recorded {
            fallback_end(&handle);
            post_tray_exit(&handle);
        } else {
            end_loop(&handle, 0);
        }
        recorded
    });

    assert_eq!(report, Some(true), "premise: the call started");
    assert_eq!(
        code, 0,
        "run_return code after Yes ({FALLBACK_CODE} = the loop was still running {BUDGET:?} later)"
    );
    assert_eq!(wc.exit.asked().len(), 1, "one ask");
    let dir = only_folder(&wc.root());
    let meta = meta_of(&dir);
    assert_eq!(meta.state, CallState::ReadyToTranscribe);
    assert_eq!(meta.stop_reason, Some(StopReason::Exit));
    assert_tracks_complete(&dir);
    let stopped = wc.stopped(1);
    assert_eq!(stopped.len(), 1, "one call_stopped: {stopped:?}");
    assert!(
        stopped[0].split_whitespace().any(|t| t == "reason=exit"),
        "{stopped:?}"
    );
    assert!(
        wc.notifier.calls().is_empty(),
        "a notice was offered on the way out: {:?}",
        wc.notifier.calls()
    );
    assert!(
        timing_support::holds_for(QUIET, || meta_of(&dir).state
            == CallState::ReadyToTranscribe),
        "the exiting call was enqueued for a pass"
    );
}

#[test]
fn tray_exit_while_idle_asks_nothing() {
    // Acceptance 1 (idle branch): with no call recording the Exit item asks nothing and ends the
    // loop with code 0 as before. Bite: an ask on every Exit (a dialog for nothing, and with
    // the box unanswered the loop never ends), Exit swallowed by the call part.
    let _serial = serial();
    let _watchdog = watchdog("tray_exit_while_idle_asks_nothing");
    let (wc, app, outcome) = wry_call_app();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        fallback_end(&handle);
        post_tray_exit(&handle)
    });

    assert_eq!(report, Some(true), "premise: Exit was posted");
    assert_eq!(
        code, 0,
        "the idle Exit ends the loop ({FALLBACK_CODE} = it did not)"
    );
    assert!(
        wc.exit.asked().is_empty(),
        "an idle Exit asked: {:?}",
        wc.exit.asked()
    );
}

#[derive(Debug, Default)]
struct DialogReport {
    recorded: bool,
    asked_once: bool,
    main_alive_while_asked: bool,
    second_click_ignored: bool,
    stopped_while_asked: bool,
}

#[test]
fn the_open_exit_question_blocks_neither_the_main_thread_nor_the_call() {
    // I3, D2: while the question is open (the fake's ask blocks like the real box), the main
    // thread still answers, a second Exit click asks nothing more, and a tray Stop still stops
    // the call. A Yes after the call already stopped just exits: the stop reason stays `user`
    // (no second stop, no `exit`). Bite: the box shown on the main thread or on the call worker
    // (the round trip or the Stop hangs), a second ask per click, a Yes that stops again (a
    // second `call_stopped`) or rewrites the reason.
    let _serial = serial();
    let _watchdog = watchdog("the_open_exit_question_blocks_neither_the_main_thread_nor_the_call");
    let (wc, app, outcome) = wry_call_app();
    let (exit, source) = (wc.exit.clone(), wc.source.clone());
    exit.hold();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let mut report = DialogReport::default();
        let Some(session) = record(&handle, &source) else {
            exit.release(false);
            end_loop(&handle, 0);
            return report;
        };
        report.recorded = true;
        post_tray_exit(&handle);
        report.asked_once = timing_support::eventually(|| exit.asked().len() == 1);
        report.main_alive_while_asked = main_round_trip(&handle);
        post_tray_exit(&handle);
        report.second_click_ignored = timing_support::holds_for(QUIET, || exit.asked().len() == 1);
        call::request(&handle, CallRequest::Stop(CallSource::Tray));
        report.stopped_while_asked =
            timing_support::eventually(|| session.try_is_recording() == Some(false));
        fallback_end(&handle);
        exit.release(true);
        report
    });

    let report = report.expect("the driver sent no report");
    assert!(report.recorded, "premise: the call started: {report:?}");
    assert!(report.asked_once, "{report:?}");
    assert!(report.main_alive_while_asked, "{report:?}");
    assert!(report.second_click_ignored, "{report:?}");
    assert!(report.stopped_while_asked, "{report:?}");
    assert_eq!(
        code, 0,
        "a Yes after the stop just exits ({FALLBACK_CODE} = it did not)"
    );
    let meta = meta_of(&only_folder(&wc.root()));
    assert_eq!(meta.stop_reason, Some(StopReason::User), "{meta:?}");
    assert_eq!(wc.stopped(1).len(), 1, "exactly one call_stopped");
    assert_eq!(wc.exit.asked().len(), 1, "one ask in all");
}

#[test]
fn run_event_exit_finalizes_a_recording_call_without_a_notice_or_a_pass() {
    // Acceptance 2, I1, I2 (R14): the loop ends while a call records (no question: a
    // main-thread `exit(0)`, the path WM_ENDSESSION, the installer close and any other end
    // take): `on_run_event`'s Exit arm stops the call with `SystemEnd`: meta
    // `ready-to-transcribe`, stop_reason `system_end`, both tracks finished, one
    // `call_stopped reason=system_end`, no ask, no notice offered and nothing enqueued (the
    // meta stays `ready-to-transcribe`: the next start queues it). The time of the Exit arm is
    // printed (`finalize took`), not judged (see the module docs). Bite: no stop in the Exit
    // arm (meta `recording`, tracks cut), a stop with another reason, the stop after
    // `SessionEnd::end`, a notice or an enqueue while exiting (the pass turns the meta
    // `failed`).
    let _serial = serial();
    let _watchdog =
        watchdog("run_event_exit_finalizes_a_recording_call_without_a_notice_or_a_pass");
    let (wc, app, outcome) = wry_call_app();
    let source = wc.source.clone();

    let (code, report) = run_with_driver(app, outcome, move |handle| {
        let recorded = record(&handle, &source).is_some();
        end_loop(&handle, 0);
        recorded
    });

    assert_eq!(report, Some(true), "premise: the call started");
    assert_eq!(code, 0);
    let dir = only_folder(&wc.root());
    let meta = meta_of(&dir);
    assert_eq!(meta.state, CallState::ReadyToTranscribe, "{meta:?}");
    assert_eq!(meta.stop_reason, Some(StopReason::SystemEnd), "{meta:?}");
    assert_tracks_complete(&dir);
    let stopped = wc.stopped(1);
    assert_eq!(stopped.len(), 1, "one call_stopped: {stopped:?}");
    assert!(
        stopped[0]
            .split_whitespace()
            .any(|t| t == "reason=system_end"),
        "{stopped:?}"
    );
    assert!(wc.exit.asked().is_empty(), "the exit path asked a question");
    assert!(
        wc.notifier.calls().is_empty(),
        "a notice was offered while exiting: {:?}",
        wc.notifier.calls()
    );
    assert!(
        timing_support::holds_for(QUIET, || meta_of(&dir).state
            == CallState::ReadyToTranscribe),
        "the exiting call was enqueued for a pass"
    );
}

// ---- child processes ---------------------------------------------------------------------------

/// The child entry. In a normal run (`MODE_VAR` unset, also under `--ignored`) it returns at once.
#[test]
#[ignore = "child entry of the call lifecycle tests: run by them, with VOICEN_LIFECYCLE_CHILD set"]
fn lifecycle_child() {
    let (Some(mode), Some(dir)) = (std::env::var_os(MODE_VAR), std::env::var_os(DIR_VAR)) else {
        return;
    };
    let dir = PathBuf::from(dir);
    match mode.to_str() {
        Some("recording") => child_recording(&dir),
        Some("endsession") => child_endsession(&dir),
        other => panic!("unknown child mode {other:?}"),
    }
}

/// A mock-runtime app with a WAV-backed call recording; writes `root.txt` (the calls root) once
/// both tracks play, then waits to be killed.
fn child_recording(dir: &Path) {
    let (rig, source) = CallRig::with_wav();
    call::request(rig.handle(), CallRequest::Record(CallSource::Tray));
    assert!(
        pumping_eventually(|| rig.recording()),
        "the call never started"
    );
    assert!(
        pumping_eventually(|| source.opens() == 2),
        "both tracks were not opened"
    );
    source.pump();
    let root = rig.dir.path().join("calls");
    fs::write(dir.join("root.tmp"), root.to_string_lossy().as_bytes()).expect("root.tmp");
    fs::rename(dir.join("root.tmp"), dir.join("root.txt")).expect("root.txt");
    // Until the parent kills this process; bounded by the hang bound.
    let _ = pumping_eventually(|| false);
}

/// A real-runtime app (`App::run`, as `run()` runs it: the process ends inside the exit) with the
/// session managed and a WAV-backed call recording; `ready.txt` once both tracks play. The loop
/// ends only through the parent's WM_ENDSESSION. Prints `finalize=<time>` from the Exit arm.
fn child_endsession(dir: &Path) {
    let logs = dir.join("logs");
    let log = voicen_lib::diag::start(logs.clone(), Box::new(|_| {}));
    let end = voicen_lib::diag::begin_session(logs, &log);
    let (app, outcome) = wry_app_in(dir, log);
    app.manage(end);
    let source = WavSource::new();
    start_call_part(
        app.handle(),
        dir.join("calls"),
        source.clone(),
        Arc::new(FakeExit::default()),
        Arc::new(FakeNotifier::new()),
    );
    let ready = dir.join("ready.txt");
    let mut driver = Some(move |handle: AppHandle<Wry>| {
        if record(&handle, &source).is_some() {
            fs::write(&ready, b"ready").expect("ready.txt");
        }
        thread::sleep(Duration::from_secs(60));
        end_loop(&handle, FALLBACK_CODE);
    });
    app.run(move |handle, event| {
        let ready_event = matches!(event, RunEvent::Ready);
        if matches!(event, RunEvent::Exit) {
            let (_, took) = timing_support::measure(|| {
                voicen_lib::on_run_event(handle, event, &outcome, false);
            });
            println!("finalize={took:?}");
            return;
        }
        voicen_lib::on_run_event(handle, event, &outcome, false);
        if ready_event {
            if let Some(driver) = driver.take() {
                let handle = handle.clone();
                thread::spawn(move || driver(handle));
            }
        }
    });
}

/// What a child left: its exit code (`None` when killed at the budget) and output.
#[derive(Debug)]
#[allow(dead_code, reason = "shown through Debug in the failure messages")]
struct Ran {
    code: Option<i32>,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

fn spawn_reader(mut from: impl Read + Send + 'static) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = from.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn spawn_child(mode: &str, dir: &Path) -> (Child, [JoinHandle<String>; 2]) {
    let exe = std::env::current_exe().expect("this test exe's path");
    let mut child = Command::new(exe)
        .args([
            "lifecycle_child",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(MODE_VAR, mode)
        .env(DIR_VAR, OsString::from(dir.as_os_str()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .expect("spawn the lifecycle child");
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("the child started without piped output");
    };
    (child, [spawn_reader(out), spawn_reader(err)])
}

/// Waits for `child` up to `budget`; kills and waits for it at the budget.
fn finish(mut child: Child, readers: [JoinHandle<String>; 2], budget: Duration) -> Ran {
    let started = timing_support::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if !timing_support::passed(started, budget) => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) | Err(_) => break None,
        }
    };
    let timed_out = status.is_none();
    if timed_out {
        let _ = child.kill();
        let _ = child.wait();
    }
    let [out, err] = readers;
    Ran {
        code: status.and_then(|s| s.code()),
        timed_out,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    }
}

/// The first top-level window of process `pid` of class `class`, as a raw handle value.
fn window_of(pid: u32, class: &str) -> Option<isize> {
    struct Search<'a> {
        pid: u32,
        class: &'a str,
        found: Option<isize>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, search: LPARAM) -> BOOL {
        // SAFETY: `search` is the `Search` that `window_of` passes, alive and not otherwise
        // touched for the whole `EnumWindows` call.
        let search = unsafe { &mut *(search.0 as *mut Search<'_>) };
        let mut owner = 0u32;
        // SAFETY: `owner` is a writable local; the call only reads the window's owner.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == search.pid {
            let mut buf = [0u16; 64];
            // SAFETY: `buf` is writable; GetClassNameW sends no message.
            let len = unsafe { GetClassNameW(hwnd, &mut buf) };
            let len = usize::try_from(len).unwrap_or(0).min(buf.len());
            if String::from_utf16_lossy(&buf[..len]) == search.class {
                search.found = Some(hwnd.0 as isize);
                return BOOL::from(false);
            }
        }
        BOOL::from(true)
    }
    let mut search = Search {
        pid,
        class,
        found: None,
    };
    // SAFETY: the callback writes only to `search`, and only during this call. A callback that
    // stops the enumeration makes EnumWindows return an error, which is not a failure here.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search<'_> as isize)) };
    search.found
}

/// WM_QUERYENDSESSION, then WM_ENDSESSION(TRUE), as Windows sends at log-off and Restart Manager
/// at an installer close. The second may return only when the target ends inside its handler.
fn send_end_session(raw: isize) {
    let hwnd = HWND(raw as *mut c_void);
    // SAFETY: a window of another process; plain integer arguments.
    unsafe {
        let _ = SendMessageW(hwnd, WM_QUERYENDSESSION, Some(WPARAM(0)), Some(LPARAM(0)));
        let _ = SendMessageW(hwnd, WM_ENDSESSION, Some(WPARAM(1)), Some(LPARAM(0)));
    }
}

#[test]
fn wm_endsession_finalizes_a_recording_call_before_the_exited_line() {
    // Acceptance 2, I1 (R14; the installer's Restart Manager close takes this same path): a child
    // app (`App::run`, session managed) records a WAV-backed call; the parent sends
    // WM_QUERYENDSESSION + WM_ENDSESSION(TRUE) to the child's tao window. The child exits with
    // code 0 and leaves: meta `ready-to-transcribe` with stop_reason `system_end`, both tracks
    // finished, one `call_stopped reason=system_end` BEFORE the one `exited` line, no pass
    // line (`final_finished`). The child prints the measured finalize time; it is shown here
    // (the R14 record cites it). Bite: the call left `recording` (no stop at the exit, the
    // tracks cut), the stop after the `exited` line, a pass started on the way out.
    let _serial = serial();
    let tmp = TempDir::new();
    let (mut child, readers) = spawn_child("endsession", tmp.path());
    let ready = tmp.path().join("ready.txt");
    let up =
        timing_support::eventually(|| ready.exists() || matches!(child.try_wait(), Ok(Some(_))));
    if !up || !ready.exists() {
        let _ = child.kill();
        let ran = finish(child, readers, CHILD_BUDGET);
        panic!("the child never started recording: {ran:#?}");
    }
    let pid = child.id();
    let mut target = None;
    timing_support::eventually(|| {
        target = window_of(pid, TAO_TARGET);
        target.is_some()
    });
    let Some(raw) = target else {
        let _ = child.kill();
        let ran = finish(child, readers, CHILD_BUDGET);
        panic!("premise: no `{TAO_TARGET}` window of the child: {ran:#?}");
    };
    assert!(
        timing_support::within_hang(move || send_end_session(raw)).is_some(),
        "HANG: the end-session messages were not answered"
    );
    let ran = finish(child, readers, CHILD_BUDGET);
    eprintln!("call_lifecycle: the child's {}", ran.stdout.trim());

    assert!(
        !ran.timed_out,
        "the child did not end after WM_ENDSESSION: {ran:#?}"
    );
    assert_eq!(ran.code, Some(0), "{ran:#?}");
    let dir = only_folder(&tmp.path().join("calls"));
    let meta = meta_of(&dir);
    assert_eq!(meta.state, CallState::ReadyToTranscribe, "{meta:?}");
    assert_eq!(meta.stop_reason, Some(StopReason::SystemEnd), "{meta:?}");
    assert_tracks_complete(&dir);
    let lines = log_lines(&tmp.path().join("logs"));
    let stopped = headed(&lines, "call_stopped");
    assert_eq!(stopped.len(), 1, "one call_stopped: {lines:#?}");
    assert!(
        stopped[0]
            .split_whitespace()
            .any(|t| t == "reason=system_end"),
        "{stopped:?}"
    );
    assert_eq!(
        headed(&lines, "exited").len(),
        1,
        "one exited line: {lines:#?}"
    );
    assert!(
        head_index(&lines, "call_stopped") < head_index(&lines, "exited"),
        "the call was stopped after the exited line: {lines:#?}"
    );
    assert!(
        headed(&lines, "final_finished").is_empty(),
        "a pass ran on the way out: {lines:#?}"
    );
}

// ---- sleep and lock: the hotkey thread's window --------------------------------------------------

/// The app, its started dictation (the hotkey thread) and its started call part, over fakes.
struct Hot {
    rig: AppRig,
    notifier: Arc<FakeNotifier>,
    source: Arc<WavSource>,
    _dictation: DictationHandle,
}

fn start_hot() -> Hot {
    let rig = AppRig::new(voicen_core::settings::EngineKind::Api);
    let notifier = Arc::new(FakeNotifier::new());
    let source = WavSource::new();
    let engine = TextEngine::new("call lifecycle test transcript");
    // As `run()`: the dictation first (its hotkey thread carries the power events), then the
    // call part.
    let dictation = start_dictation_with(
        rig.app.handle(),
        DictationPorts {
            audio: Arc::new(FakeAudioSource::new()),
            clipboard: Arc::new(FakeClipboard::new()),
            paster: Arc::new(FakePaster::new()),
            engine_factory: Some(engine_factory(&engine)),
            builtin: None,
            indicator: Arc::new(FakeIndicator::new()),
            credentials: Arc::clone(&rig.creds),
            hotkeys: Arc::clone(&rig.hotkeys),
            detector: Err(voicen_core::vad::VadError::Unavailable),
            temp_audio: Arc::new(voicen_core::platform::FakeTempAudioStore::new()),
        },
        notifier.clone(),
    )
    .expect("start_dictation_with");
    let calls_root = rig.dir.path().join("calls");
    fs::create_dir_all(&calls_root).expect("the calls root");
    call::start_calls(
        rig.app.handle(),
        CallPorts {
            source: source.clone(),
            disk: FakeDisk::with_available(100_000_000_000),
            mono: Arc::new(SystemMonotonic),
            open_track: plain_opener(),
            notifier: notifier.clone(),
            calls_root,
            clipboard: Arc::new(FakeClipboard::new()),
            folder: Arc::new(FakeFolder::default()),
            save: Arc::new(FakeSave::default()),
            credentials: Arc::clone(&rig.creds),
            detector: None,
            exit_confirm: Arc::new(FakeExit::default()),
        },
    )
    .expect("the call part starts");
    Hot {
        rig,
        notifier,
        source,
        _dictation: dictation,
    }
}

impl Hot {
    fn root(&self) -> PathBuf {
        self.rig.dir.path().join("calls")
    }

    /// Whether a call records; polls `try_is_recording` while pumping (never blocks on the
    /// session lock, T-128).
    fn recording(&self) -> bool {
        let Some(session) = self.rig.app.try_state::<Arc<CallSession>>() else {
            return false;
        };
        let session = Arc::clone(&session);
        let mut known = None;
        let answered = eventually(|| {
            known = session.try_is_recording();
            known.is_some()
        });
        assert!(answered, "HANG: the session lock stayed held for {HANG:?}");
        known.unwrap_or(false)
    }

    /// Posts a tray-style Record and waits until both tracks play.
    #[track_caller]
    fn start_call(&self) {
        call::request(self.rig.app.handle(), CallRequest::Record(CallSource::Tray));
        assert!(eventually(|| self.recording()), "the call never started");
        assert!(
            eventually(|| self.source.opens() >= 2),
            "both tracks were not opened"
        );
        self.source.pump();
    }

    fn offered(&self) -> Vec<MessageId> {
        self.notifier.calls().iter().map(|c| c.id).collect()
    }

    fn lines(&self, head: &str) -> Vec<String> {
        headed(&self.rig.log_lines(), head)
    }
}

/// The one top-level window of this process titled "Voicen hotkey".
#[track_caller]
fn hotkey_window() -> HWND {
    let pid = std::process::id();
    let mut found = Vec::new();
    let mut after: Option<HWND> = None;
    loop {
        // SAFETY: static strings; `after` is a window FindWindowExW just returned.
        let next = unsafe { FindWindowExW(None, after, PCWSTR::null(), w!("Voicen hotkey")) };
        let Ok(hwnd) = next else { break };
        if hwnd.is_invalid() {
            break;
        }
        let mut owner = 0u32;
        // SAFETY: a window handle and a writable u32.
        let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == pid {
            found.push(hwnd);
        }
        after = Some(hwnd);
    }
    assert_eq!(
        found.len(),
        1,
        "no single \"Voicen hotkey\" window: {found:?}"
    );
    found[0]
}

/// Sends `msg` to `hwnd` and returns once its procedure returned.
fn deliver(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) {
    // SAFETY: a window of this process; plain integer arguments.
    let _ = unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) };
}

fn suspend(hwnd: HWND) {
    deliver(hwnd, WM_POWERBROADCAST, PBT_APMSUSPEND as usize, 0);
}

fn resume(hwnd: HWND) {
    deliver(hwnd, WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize, 0);
}

/// This process's session id (the `lParam` of WM_WTSSESSION_CHANGE).
fn this_session() -> isize {
    let mut id = 0u32;
    // SAFETY: this process's id and a writable u32.
    unsafe { ProcessIdToSessionId(std::process::id(), &mut id) }
        .expect("ProcessIdToSessionId for this process");
    id as isize
}

#[test]
fn suspend_stops_the_call_with_sleep_and_resume_offers_the_sleep_notice() {
    // Acceptance 3, I4, D1: PBT_APMSUSPEND to the hotkey window stops the recording call with
    // `Sleep` (meta `ready-to-transcribe`, stop_reason `sleep`, tracks finished, `call_stopped
    // reason=sleep`); until the resume nothing is offered and nothing is enqueued (the meta stays
    // `ready-to-transcribe`); PBT_APMRESUMEAUTOMATIC then offers `notice.stopped.sleep` first,
    // and only after it the folder is enqueued (the meta leaves `ready-to-transcribe`). Bite: no
    // suspend hand-off to the call (it keeps recording), the stop reason not Sleep, the notice
    // or the enqueue at the stop (before the sleep), the notice never released or released
    // after the pass notice, the resume releasing nothing, a pass that never starts.
    let _serial = serial();
    let hot = start_hot();
    hot.start_call();
    let hwnd = hotkey_window();

    suspend(hwnd);
    assert!(
        eventually(|| !hot.recording()),
        "PBT_APMSUSPEND did not stop the call"
    );
    let dir = only_folder(&hot.root());
    let meta = meta_of(&dir);
    assert_eq!(meta.state, CallState::ReadyToTranscribe, "{meta:?}");
    assert_eq!(meta.stop_reason, Some(StopReason::Sleep), "{meta:?}");
    assert_tracks_complete(&dir);
    assert!(
        eventually(|| hot.lines("call_stopped").len() == 1),
        "no call_stopped line"
    );
    assert!(
        hot.lines("call_stopped")[0]
            .split_whitespace()
            .any(|t| t == "reason=sleep"),
        "{:?}",
        hot.lines("call_stopped")
    );
    assert!(
        holds_for(SETTLE, || {
            hot.offered().is_empty() && meta_of(&dir).state == CallState::ReadyToTranscribe
        }),
        "before the resume a notice was offered or the call was enqueued: {:?}",
        hot.offered()
    );

    resume(hwnd);
    assert!(
        eventually(|| hot.offered().first() == Some(&NOTICE_STOPPED_SLEEP)),
        "the resume offered no notice.stopped.sleep first: {:?}",
        hot.offered()
    );
    assert!(
        eventually(|| meta_of(&dir).state != CallState::ReadyToTranscribe),
        "the resume did not enqueue the stopped call"
    );
    assert_eq!(
        hot.offered()
            .iter()
            .filter(|id| **id == NOTICE_STOPPED_SLEEP)
            .count(),
        1,
        "the sleep notice was offered more than once: {:?}",
        hot.offered()
    );
}

#[test]
fn a_lock_and_an_unlock_leave_the_call_recording() {
    // Acceptance 3, I4: a WTS lock and unlock of this session (WM_WTSSESSION_CHANGE) change
    // nothing for a recording call: it still records, its meta is still `recording`, nothing is
    // stopped, offered or enqueued. Bite: the unlock arm (or any session change) mapped to
    // `Sleep` or `Resumed`, a stop on a session change.
    let _serial = serial();
    let hot = start_hot();
    hot.start_call();
    let hwnd = hotkey_window();

    deliver(
        hwnd,
        WM_WTSSESSION_CHANGE,
        WTS_SESSION_LOCK as usize,
        this_session(),
    );
    deliver(
        hwnd,
        WM_WTSSESSION_CHANGE,
        WTS_SESSION_UNLOCK as usize,
        this_session(),
    );

    let dir = only_folder(&hot.root());
    assert!(
        holds_for(SETTLE, || {
            hot.recording()
                && meta_of(&dir).state == CallState::Recording
                && hot.offered().is_empty()
        }),
        "a lock or unlock touched the call: recording={} meta={:?} offered={:?}",
        hot.recording(),
        meta_of(&dir).state,
        hot.offered()
    );
    assert!(
        hot.lines("call_stopped").is_empty(),
        "{:?}",
        hot.lines("call_stopped")
    );
}

#[test]
fn the_next_call_request_releases_a_held_sleep_notice_without_a_resume() {
    // D1: when no resume arrives, the next call request (here a Record) also releases what the
    // shell holds: the stopped call's `notice.stopped.sleep` is offered, and its folder is
    // enqueued. Bite: held items released only by a resume event (a lost resume loses the
    // notice for good).
    let _serial = serial();
    let hot = start_hot();
    hot.start_call();
    let hwnd = hotkey_window();
    suspend(hwnd);
    assert!(
        eventually(|| !hot.recording()),
        "the suspend did not stop the call"
    );
    let first = only_folder(&hot.root());
    assert!(
        holds_for(SETTLE, || hot.offered().is_empty()),
        "a notice was offered before any resume: {:?}",
        hot.offered()
    );

    call::request(hot.rig.app.handle(), CallRequest::Record(CallSource::Tray));
    assert!(
        eventually(|| hot.offered().first() == Some(&NOTICE_STOPPED_SLEEP)),
        "the next request did not release the held notice: {:?}",
        hot.offered()
    );
    assert!(
        eventually(|| meta_of(&first).state != CallState::ReadyToTranscribe),
        "the next request did not release the held hand-off"
    );
}

#[test]
fn an_unlock_after_a_suspend_releases_nothing_that_the_shell_holds() {
    // I4: only PBT_APMRESUMEAUTOMATIC releases the held sleep notice and hand-off; a WTS unlock
    // of this session between the suspend and the resume must not (the unlock arm re-registers
    // the hotkeys only). Bite: the unlock arm also running the call port's `on_resume` (a lock
    // test on a recording call cannot see it: releasing an empty hold changes nothing).
    let _serial = serial();
    let hot = start_hot();
    hot.start_call();
    let hwnd = hotkey_window();
    suspend(hwnd);
    assert!(
        eventually(|| !hot.recording()),
        "the suspend did not stop the call"
    );
    let dir = only_folder(&hot.root());

    deliver(
        hwnd,
        WM_WTSSESSION_CHANGE,
        WTS_SESSION_UNLOCK as usize,
        this_session(),
    );
    assert!(
        holds_for(SETTLE, || {
            hot.offered().is_empty() && meta_of(&dir).state == CallState::ReadyToTranscribe
        }),
        "an unlock released the held notice or hand-off: {:?}",
        hot.offered()
    );

    resume(hwnd);
    assert!(
        eventually(|| hot.offered().first() == Some(&NOTICE_STOPPED_SLEEP)),
        "the resume offered no notice.stopped.sleep: {:?}",
        hot.offered()
    );
}

// ---- start-up: recover and enqueue ---------------------------------------------------------------

/// Runs a child that records a WAV-backed call and kills it with TerminateProcess; returns the
/// call folder it left (moved into `into`, a calls root of the parent's rig).
fn killed_recording_into(into: &Path) -> PathBuf {
    let tmp = TempDir::new();
    let (mut child, readers) = spawn_child("recording", tmp.path());
    let root_file = tmp.path().join("root.txt");
    let up = timing_support::eventually(|| {
        root_file.exists() || matches!(child.try_wait(), Ok(Some(_)))
    });
    // TerminateProcess: no Drop, no EOS page, the meta stays `recording`.
    let _ = child.kill();
    let ran = finish(child, readers, CHILD_BUDGET);
    assert!(
        up && root_file.exists(),
        "premise: the child never recorded: {ran:#?}"
    );
    let child_root = PathBuf::from(fs::read_to_string(&root_file).expect("root.txt"));
    let folder = only_folder(&child_root);
    assert_eq!(
        meta_of(&folder).state,
        CallState::Recording,
        "premise: the kill left the meta `recording`"
    );
    fs::create_dir_all(into).expect("the parent's calls root");
    let target = into.join(folder.file_name().expect("the folder name"));
    fs::rename(&folder, &target).expect("move the killed call into the parent's root");
    let _ = fs::remove_dir_all(&child_root);
    target
}

#[test]
fn a_killed_recording_is_recovered_at_the_next_start() {
    // Acceptance 4 (failure branch), I5: a child records a WAV-backed call and is killed with
    // TerminateProcess; a fresh app over the same call folder starts its call part: the worker's
    // first job is the one recover, so the meta becomes `recovered` with the scanned duration,
    // `call_recovered calls=1` is logged, the folder is not started or enqueued (it stays
    // `recovered`, no call records) and nothing is offered. Bite: no recover at start (the
    // meta stays `recording`, no line), recover run before the settings are known or on the
    // wrong store (a second store: the folder then counts as active), the recovered folder
    // enqueued for a pass (the meta leaves `recovered`).
    let _serial = serial();
    let rig = CallRig::bare();
    let dir = killed_recording_into(&rig.dir.path().join("calls"));
    rig.start(
        WavSource::new(),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
    );

    assert!(
        pumping_eventually(|| rig.count(&["call_recovered", "calls=1"]) == 1),
        "no `call_recovered calls=1` line: {:?}",
        rig.lines(&["call_recovered"])
    );
    let meta = meta_of(&dir);
    assert_eq!(meta.state, CallState::Recovered, "{meta:?}");
    assert!(
        meta.duration_ms.is_some(),
        "recovered keeps a duration: {meta:?}"
    );
    assert!(
        call_shell_support::holds_for_a_while(|| {
            !rig.recording()
                && meta_of(&dir).state == CallState::Recovered
                && rig.notifier.calls().is_empty()
        }),
        "the recovered call was started, enqueued or announced: {:?}",
        rig.notifier.calls()
    );
}

#[test]
fn a_ready_to_transcribe_folder_is_enqueued_once_at_start() {
    // I5 (FR-007: a call left by a shutdown is transcribed at the next start): a call folder at
    // `ready-to-transcribe` before the start is handed to the pass queue by the start-up
    // recovery: its meta leaves `ready-to-transcribe` (the rig has no engine key, so the pass
    // ends `failed`), `call_recovered calls=0` is logged (nothing was `recording`), and the
    // pass ran once (at most one `final_finished` line). Bite: no enqueue of `Recovery.queue`
    // (the meta stays), an enqueue per start-up path (two passes), recovered folders enqueued
    // too.
    let _serial = serial();
    let rig = CallRig::bare();
    let dir = killed_recording_into(&rig.dir.path().join("calls"));
    let mut meta = meta_of(&dir);
    meta.state = CallState::ReadyToTranscribe;
    meta.stop_reason = Some(StopReason::User);
    meta.duration_ms = Some(2_000);
    meta.ended_at = Some(meta.started_at.clone());
    write_meta(&dir, &meta).expect("plant a ready-to-transcribe meta");

    rig.start(
        WavSource::new(),
        FakeDisk::with_available(100_000_000_000),
        plain_opener(),
    );

    assert!(
        pumping_eventually(|| meta_of(&dir).state != CallState::ReadyToTranscribe),
        "the ready-to-transcribe folder was not enqueued at start"
    );
    let queue = rig
        .handle()
        .try_state::<Arc<voicen_core::call::final_pass::PassQueue>>()
        .map(|q| Arc::clone(&q))
        .expect("start_calls manages the one PassQueue");
    assert!(
        timing_support::within_hang(move || queue.wait_idle()).is_some(),
        "HANG: the pass queue did not go idle"
    );
    assert_eq!(rig.count(&["call_recovered", "calls=0"]), 1);
    assert!(
        rig.count(&["final_finished"]) <= 1,
        "the folder was passed more than once: {:?}",
        rig.lines(&["final_finished"])
    );
}
