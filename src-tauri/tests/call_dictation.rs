//! T-151 (US7 #1, #3-#5; spec 007 FR-009; req FR-22, FR-36, NFR-02): a dictation during a call
//! recording through the composed app. One app as `run()` builds it (`build_app`, mock runtime),
//! the dictation started by `start_dictation_with` over the real `ShellIndicator` (so
//! `tray::applied` sees the dictation half), the real hotkey thread, `WinClipboard` and
//! `WinPaster` into a `win32_support` test window and a real-time WAV source; the call part
//! started by `voicen_lib::call::start_calls` over `WavSource` tracks, on the same app and so on
//! the same tray part. Windows CI only (decision #5); not compiled on the Linux gate.
//!
//! Invariant (docs/tasks/T-151.md analysis): the call session and the dictation session share no
//! capture, no Esc claim and no state. A dictation start, paste, Esc or end never opens, stops,
//! gaps or drops a call track; Esc is claimed only while a dictation records; the tray shows
//! `shown(dictation_state, call_recording)`; with Record off and no dictation recording, no
//! capture handle is open on either port.
//!
//! Green on its first run: the product already composes (decisions #349 Q1), so the red
//! evidence of this file is the recorded bite check in `docs/tasks/T-151.mutants/`.
//!
//! Test rules: `win32_support::serial()` first in every test; no wall-clock ceiling (every wait
//! is a state, bounded only by the hang bound of `timing_support`, F-019, F-021); single presses
//! for the call hotkey and for Esc, a hold only where the hold is the subject (F-014); the
//! Esc claim is checked after the Recording indicator plus one hotkey round trip, never by an
//! overlapping probe (F-020); the tracks are checked after the stop, on the files (F-031,
//! F-033). The track files of a call with a dictation are compared with those of the same call
//! recorded alone over the same stamp-interleaved pumps (`WavSource::pump_for` steps both tracks
//! together, one block at a time, T-266), so the expected duration is what the recorder makes of
//! the pumped span, not a number written here. Data: a `TempDir` per rig, a fake key, a fake
//! transcript; nothing real is logged.
#![cfg(windows)]

mod call_shell_support;
mod timing_support;
mod win32_support;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use tauri::Manager;

use call_shell_support::{plain_opener, CallRig, CountedHandle, FakeExit, FakeFolder, FakeSave};
use call_shell_support::{WavSource, PUMP_MS};
use timing_support::HANG;
use voicen_core::call::codec::{scan, ReadEnd};
use voicen_core::call::session::CallSession;
use voicen_core::call::{CallMeta, CallSource};
use voicen_core::clock::SystemMonotonic;
use voicen_core::platform::{
    AudioSource, CaptureHandle, DeviceId, FakeClipboard, FakeNotifier, FakeTempAudioStore,
    FrameSink, InputDevice,
};
use voicen_core::recording::{CaptureError, TrayState};
use voicen_core::settings::EngineKind;
use voicen_core::test_support::fixtures;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::realtime::RealtimeSource;
use voicen_core::tray as table;
use voicen_lib::call::{self, start_calls, CallPorts, CallRequest};
use voicen_lib::dictation::{
    start_dictation_with, DictationHandle, DictationPorts, ShellIndicator,
};
use voicen_lib::tray;
use voicen_lib::win::clipboard::WinClipboard;
use voicen_lib::win::paste::WinPaster;
use win32_support::{
    assert_hotkey_free, combo_is_free, dictation_lines, engine_factory, esc_claim, esc_reached,
    esc_released, eventually, has_pair, serial, AppRig, Keys, Shape, TestWindow, TextEngine, HOLD,
    HOTKEY_KEYS, SETTLE, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SPACE,
};

const TRANSCRIPT: &str = "call dictation test transcript";
const CALL_HOTKEY: &str = "Ctrl+Alt+R";
const VK_R: u16 = 0x52;
const CALL_KEYS: [u16; 3] = [VK_CONTROL, VK_MENU, VK_R];

/// Pumped before the dictation, during its hold and after it (ms of track audio): the same
/// three pumps in the call recorded alone and in the call with a dictation.
const BEFORE_MS: u64 = 1_000;
const DURING_MS: u64 = 1_500;

// ---- the dictation's audio port, counted ----------------------------------------------------

/// The real-time WAV source of the dictation with an open-handle count (`RealtimeSource`
/// counts nothing): a handle is open from `start` until its `stop` returns or it is dropped.
struct CountingAudio {
    inner: RealtimeSource,
    open: Arc<AtomicUsize>,
}

impl CountingAudio {
    fn new() -> Arc<CountingAudio> {
        Arc::new(CountingAudio {
            inner: RealtimeSource::from_buffer(&fixtures::speech_3s()),
            open: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn open_handles(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

impl AudioSource for CountingAudio {
    fn devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        self.inner.devices()
    }

    fn start(
        &self,
        device: &DeviceId,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let handle = self.inner.start(device, sink)?;
        Ok(CountedHandle::open(handle, &self.open))
    }
}

// ---- the composed app -----------------------------------------------------------------------

/// The app, its dictation (real hotkey thread, tray and paste) and its call part (WAV tracks).
struct Rig {
    // Dropped first: the hotkey thread goes before the app.
    _dictation: DictationHandle,
    rig: AppRig,
    tracks: Arc<WavSource>,
    audio: Arc<CountingAudio>,
    engine: Arc<TextEngine>,
    window: TestWindow,
}

impl Rig {
    #[track_caller]
    fn new() -> Rig {
        assert_hotkey_free();
        let window = TestWindow::open(Shape::TopLevel);
        window.front();
        let rig = AppRig::new(EngineKind::Api);
        let engine = TextEngine::new(TRANSCRIPT);
        let audio = CountingAudio::new();
        let tracks = WavSource::new();
        let notifier = Arc::new(FakeNotifier::new());
        // As `run()`: the dictation first (it registers the call slot), then the call part.
        let dictation = start_dictation_with(
            rig.app.handle(),
            DictationPorts {
                audio: audio.clone(),
                clipboard: Arc::new(WinClipboard::new()),
                paster: Arc::new(WinPaster::new()),
                engine_factory: Some(engine_factory(&engine)),
                builtin: None,
                indicator: Arc::new(ShellIndicator::new(rig.app.handle())),
                credentials: Arc::clone(&rig.creds),
                hotkeys: Arc::clone(&rig.hotkeys),
                detector: Err(voicen_core::vad::VadError::Unavailable),
                temp_audio: Arc::new(FakeTempAudioStore::new()),
            },
            notifier.clone(),
        )
        .expect("start_dictation_with");
        let calls_root = rig.dir.path().join("calls");
        fs::create_dir_all(&calls_root).expect("the calls root");
        start_calls(
            rig.app.handle(),
            CallPorts {
                source: tracks.clone(),
                disk: FakeDisk::with_available(100_000_000_000),
                mono: Arc::new(SystemMonotonic),
                open_track: plain_opener(),
                notifier,
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
        assert!(
            eventually(|| !combo_is_free(CALL_HOTKEY)),
            "the app does not hold {CALL_HOTKEY}"
        );
        Rig {
            _dictation: dictation,
            rig,
            tracks,
            audio,
            engine,
            window,
        }
    }

    /// The state the tray shows now (`tray::applied`: the last apply that went through).
    fn tray(&self) -> Option<TrayState> {
        tray::applied(self.rig.app.handle()).map(|a| a.state)
    }

    #[track_caller]
    fn assert_tray_becomes(&self, want: TrayState, why: &str) {
        assert!(
            eventually(|| self.tray() == Some(want)),
            "{why}: the tray shows {:?}, not {want:?}, within {HANG:?}",
            self.tray()
        );
    }

    /// Whether a call records; polls `try_is_recording` while pumping (never blocks on the
    /// session lock: the call worker may need this thread's messages, T-128).
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

    /// Starts the call with one press of the call hotkey (F-014: a press, not a hold).
    #[track_caller]
    fn start_call(&self) {
        let mut keys = Keys::press(&CALL_KEYS);
        keys.release_all();
        assert!(
            eventually(|| self.recording()),
            "the call hotkey did not start a call within {HANG:?}"
        );
    }

    /// Stops the call like the user would (the same `CallSession::stop(User)` the tray's Stop
    /// reaches), on a helper thread while this thread pumps.
    #[track_caller]
    fn stop_call(&self) {
        let session = self
            .rig
            .app
            .try_state::<Arc<CallSession>>()
            .map(|s| Arc::clone(&s))
            .expect("the call part's session");
        let (done, finished) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = session.stop(voicen_core::call::StopReason::User);
            let _ = done.send(());
        });
        assert!(
            eventually(|| !matches!(
                finished.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            )),
            "HANG: `CallSession::stop` did not return within {HANG:?}"
        );
    }

    /// Presses the dictation hotkey and returns once the tray shows Recording (the hold is
    /// the subject of the tests that call it).
    #[track_caller]
    fn press_dictation(&self) -> Keys {
        let keys = Keys::press(&HOTKEY_KEYS);
        self.assert_tray_becomes(TrayState::Recording, "the dictation press");
        keys
    }

    #[track_caller]
    fn assert_pasted(&self) {
        assert!(
            eventually(|| self.window.text() == TRANSCRIPT),
            "the window text {HANG:?} after the release: {:?} (engine calls {})",
            self.window.text(),
            self.engine.calls()
        );
    }

    /// The one call folder under the calls root.
    #[track_caller]
    fn call_folder(&self) -> PathBuf {
        only_folder(&self.rig.dir.path().join("calls"))
    }

    /// Why the Esc checks print what the rig knows.
    fn context(&self) -> String {
        format!(
            "tray {:?}; engine calls {}; window text {:?}; track handles {}; audio handles {}",
            self.tray(),
            self.engine.calls(),
            self.window.text(),
            self.tracks.open_handles(),
            self.audio.open_handles()
        )
    }
}

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

/// What a finished call left: the readable duration of each track (48 kHz units), that both
/// ended in their EOS page, and the meta's drops and device changes.
#[derive(Debug, PartialEq, Eq)]
struct Tracks {
    mic: u64,
    system: u64,
    drops: usize,
    device_changes: usize,
}

#[track_caller]
fn read_tracks(folder: &Path) -> Tracks {
    let mut durations = Vec::new();
    for file in ["mic.opus", "system.opus"] {
        let scanned = scan(File::open(folder.join(file)).expect(file)).expect("a readable track");
        assert_eq!(
            scanned.end,
            ReadEnd::Complete,
            "{file} is finished (EOS written)"
        );
        durations.push(scanned.duration_48k);
    }
    let meta: CallMeta =
        serde_json::from_slice(&fs::read(folder.join("meta.json")).expect("meta.json"))
            .expect("meta.json parses");
    Tracks {
        mic: durations[0],
        system: durations[1],
        drops: meta.drops.len(),
        device_changes: meta.device_changes.len(),
    }
}

/// The same call recorded with no dictation and no hotkey thread, over the same three pumps:
/// what the recorder makes of the pumped span. The rig is gone when this returns.
#[track_caller]
fn recorded_alone() -> Tracks {
    let (rig, source) = CallRig::with_wav();
    call::request(rig.handle(), CallRequest::Record(CallSource::Switch));
    assert!(
        call_shell_support::eventually(|| rig.recording()),
        "the baseline call did not start"
    );
    source.pump_for(BEFORE_MS);
    source.pump_for(DURING_MS);
    source.pump_for(PUMP_MS);
    rig.stop_call();
    let folder = only_folder(&rig.dir.path().join("calls"));
    read_tracks(&folder)
}

/// One second of a track in 48 kHz units.
const MIN_48K: u64 = 48_000;

#[track_caller]
fn assert_a_real_span(alone: &Tracks) {
    // Premise: the baseline recorded audio, so equality with it is not two empty tracks.
    assert!(
        alone.mic >= MIN_48K && alone.system >= MIN_48K,
        "premise: the baseline call recorded under a second: {alone:?}"
    );
    assert_eq!(
        (alone.drops, alone.device_changes),
        (0, 0),
        "premise: the baseline call has drops or device changes: {alone:?}"
    );
}

// ---- Acceptance 1 ---------------------------------------------------------------------------

#[test]
fn a_dictation_during_a_call_pastes_and_leaves_the_call_tracks_unbroken() {
    // Acceptance 1 (T072, T073; FR-009, US7 #1): a call records, a dictation hold follows. The
    // transcript lands in the test window, the call is still recording after it, and after the
    // stop both tracks hold exactly the audio the same call records alone (`codec::scan`
    // duration), with no drop and no device change in the meta. Bite: a dictation start, paste
    // or end that stops, gaps or drops a call track (the durations differ or the call is no
    // longer recording), a dictation that never pastes (the window text), a call part whose
    // dictation shares its capture.
    let _serial = serial();
    let alone = recorded_alone();
    assert_a_real_span(&alone);

    let rig = Rig::new();
    rig.start_call();
    rig.tracks.pump_for(BEFORE_MS);
    let mut keys = rig.press_dictation();
    rig.tracks.pump_for(DURING_MS);
    thread::sleep(HOLD);
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);
    rig.assert_pasted();
    assert_eq!(rig.engine.calls(), 1, "engine calls");
    assert!(
        rig.recording(),
        "the call stopped with the dictation: {}",
        rig.context()
    );
    rig.tracks.pump_for(PUMP_MS);
    rig.stop_call();

    assert_eq!(
        read_tracks(&rig.call_folder()),
        alone,
        "the call's tracks after a dictation differ from the call recorded alone"
    );
}

// ---- Acceptance 2 ---------------------------------------------------------------------------

#[test]
fn the_tray_shows_dictation_recording_then_call_recording_again() {
    // Acceptance 2 (T072; FR-006, windows-shell tray `shown`): with a call recording the tray
    // shows CallRecording; during the dictation hold Recording (the dictation wins); once the
    // dictation ends CallRecording again, and Idle after the call stops. Read through
    // `tray::applied`, the last apply the tray made, over the real `ShellIndicator` and the
    // call's own indicator on the same tray part. Bite: `shown` with the call flag first (the
    // hold shows CallRecording), a half of the pair not reaching the tray (the dictation or
    // the call never shows), the dictation end clearing the call's flag (Idle during the call).
    let _serial = serial();
    let rig = Rig::new();
    rig.start_call();
    rig.assert_tray_becomes(TrayState::CallRecording, "a call recording alone");

    let mut keys = rig.press_dictation();
    let shown_now = tray::applied(rig.rig.app.handle()).expect("the tray applied a state");
    assert_eq!(
        shown_now.icon,
        table::icon(TrayState::Recording),
        "the icon of the Recording state during the hold"
    );
    thread::sleep(HOLD);
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);
    rig.assert_pasted();
    rig.assert_tray_becomes(
        TrayState::CallRecording,
        "the dictation ended, the call goes on",
    );
    assert!(rig.recording(), "the call stopped: {}", rig.context());

    rig.stop_call();
    rig.assert_tray_becomes(TrayState::Idle, "the call stopped");
}

// ---- Acceptance 3 ---------------------------------------------------------------------------

#[test]
fn esc_without_a_dictation_reaches_the_focused_app_and_the_call_goes_on() {
    // Acceptance 3 (T072; call-session I6, FR-22): a call records and no dictation does. The
    // app holds no Esc, a single Esc press reaches the focused test window, and the call is
    // still recording after it. Bite: a call start that claims Esc (the probe finds it held
    // and the window never sees the key), an Esc that stops the call.
    let _serial = serial();
    let rig = Rig::new();
    rig.start_call();
    rig.tracks.pump_for(BEFORE_MS);
    assert!(
        eventually(esc_released),
        "a recording call holds Esc: {}",
        rig.context()
    );

    let mut esc = Keys::press(&[VK_ESCAPE]);
    assert!(
        eventually(|| esc_reached(&rig.window)),
        "Esc did not reach the focused window within {HANG:?}: {:?}",
        rig.window.messages()
    );
    esc.release_all();
    assert!(rig.recording(), "Esc stopped the call: {}", rig.context());
    rig.tracks.pump_for(PUMP_MS);
    rig.stop_call();
    assert_eq!(
        read_tracks(&rig.call_folder()).drops,
        0,
        "Esc left a drop in the call"
    );
}

#[test]
fn esc_during_a_dictation_discards_only_the_dictation() {
    // Acceptance 3, failure branch (T072; FR-22): Esc during the hold of a dictation made while
    // a call records discards the dictation (no paste, no engine call, one `cancelled` dictation
    // line) and nothing else: the call keeps recording, its tracks equal the call recorded
    // alone, Esc is released again. The claim is checked after the Recording tray state plus a
    // hotkey round trip, never by an overlapping probe (F-020). Bite: Esc not wired to the
    // dictation (it is delivered), Esc cancelling or stopping the call (not recording, or the
    // tracks differ), a claim that stays after the dictation ends.
    let _serial = serial();
    let alone = recorded_alone();
    assert_a_real_span(&alone);

    let rig = Rig::new();
    rig.start_call();
    rig.tracks.pump_for(BEFORE_MS);
    let mut keys = rig.press_dictation();
    if let Err(why) = esc_claim(
        || rig.tray() == Some(TrayState::Recording),
        &rig.rig.hotkeys,
        &|| false,
        &|| rig.context(),
    ) {
        panic!("{why}");
    }
    rig.tracks.pump_for(DURING_MS);
    let mut esc = Keys::press(&[VK_ESCAPE]);
    rig.assert_tray_becomes(TrayState::CallRecording, "Esc cancelled the dictation");
    esc.release_all();
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);

    assert!(
        eventually(|| !dictation_lines(&rig.rig.log_lines()).is_empty()),
        "no dictation line for the cancelled recording"
    );
    thread::sleep(SETTLE);
    let lines = dictation_lines(&rig.rig.log_lines());
    assert_eq!(lines.len(), 1, "dictation lines: {lines:?}");
    assert!(has_pair(&lines[0], "outcome", "cancelled"), "{}", lines[0]);
    assert_eq!(
        rig.engine.calls(),
        0,
        "the cancelled audio reached the engine"
    );
    assert_eq!(rig.window.text(), "", "something was pasted");
    assert!(
        eventually(esc_released),
        "Esc is still claimed after the dictation ended: {}",
        rig.context()
    );
    assert!(
        rig.recording(),
        "Esc during the dictation ended the call: {}",
        rig.context()
    );
    rig.tracks.pump_for(PUMP_MS);
    rig.stop_call();
    assert_eq!(
        read_tracks(&rig.call_folder()),
        alone,
        "the call's tracks after a discarded dictation differ from the call recorded alone"
    );
}

// ---- Acceptance 4 ---------------------------------------------------------------------------

#[test]
fn with_record_off_and_no_dictation_no_capture_stream_is_open() {
    // Acceptance 4 (T072; NFR-22, FR-36): with call mode off and no dictation recording, no
    // capture handle is open on either port; each port's count rises only with its own
    // session (the dictation opens the audio port alone, the call the two tracks alone) and
    // falls back to zero after a dictation ends and after a call stops. Bite: a capture opened
    // at start (a track handle held while idle), a dictation or call stop that leaves its
    // handle open, a dictation that opens a track or a call that opens the audio port.
    let _serial = serial();
    let rig = Rig::new();
    let idle = |why: &str| {
        assert_eq!(
            rig.tracks.open_handles(),
            0,
            "{why}: a track capture is open"
        );
        assert_eq!(
            rig.audio.open_handles(),
            0,
            "{why}: a dictation capture is open"
        );
    };
    idle("at start");
    assert_eq!(rig.tracks.opens(), 0, "a track was opened before any call");

    // A dictation alone: its own port, no track.
    let mut keys = rig.press_dictation();
    assert!(
        eventually(|| rig.audio.open_handles() == 1),
        "the dictation hold opened {} captures: {}",
        rig.audio.open_handles(),
        rig.context()
    );
    assert_eq!(rig.tracks.open_handles(), 0, "a dictation opened a track");
    thread::sleep(HOLD);
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);
    rig.assert_pasted();
    assert!(
        eventually(|| rig.audio.open_handles() == 0),
        "a dictation capture stayed open after the dictation ended: {}",
        rig.context()
    );
    idle("after a dictation");
    assert_eq!(rig.tracks.opens(), 0, "a dictation opened a track");

    // A call alone: the tracks, no dictation capture.
    rig.start_call();
    assert!(
        rig.tracks.open_handles() > 0,
        "premise: a recording call has no track open"
    );
    assert_eq!(rig.audio.open_handles(), 0, "a call opened the audio port");
    rig.tracks.pump_for(PUMP_MS);

    // A dictation during the call: one capture on the audio port, the tracks untouched.
    let tracks_open = rig.tracks.open_handles();
    let mut keys = rig.press_dictation();
    assert!(
        eventually(|| rig.audio.open_handles() == 1),
        "the dictation during the call opened {} captures: {}",
        rig.audio.open_handles(),
        rig.context()
    );
    assert_eq!(
        rig.tracks.open_handles(),
        tracks_open,
        "the dictation changed the call's open tracks"
    );
    thread::sleep(HOLD);
    keys.release(&[VK_SPACE, VK_CONTROL, VK_MENU]);
    assert!(
        eventually(|| rig.audio.open_handles() == 0),
        "a dictation capture stayed open after the dictation ended: {}",
        rig.context()
    );
    assert_eq!(
        rig.tracks.open_handles(),
        tracks_open,
        "the dictation's end closed a call track"
    );

    assert!(
        eventually(|| rig.engine.calls() == 2),
        "the dictation during the call never reached the engine: {}",
        rig.context()
    );
    rig.stop_call();
    idle("after the call stopped");
}

// ---- T-266: the comparison still catches a broken track ---------------------------------------

#[test]
fn a_call_missing_the_last_100_ms_of_both_tracks_differs_from_the_call_recorded_alone() {
    // T-266 Acceptance 2 (failure branch): the exact `Tracks` comparison of Acceptance 1 is not
    // loosened by the interleaved pump. A call whose three pumps end 100 ms short (a gap at
    // the end of both tracks, what a drop or a stall leaves) reads back different from the call
    // recorded alone: both durations are shorter, so `assert_eq!` on `Tracks` fails. Bite: a
    // tolerance on the durations wider than 100 ms, a comparison that ignores a track.
    let _serial = serial();
    let alone = recorded_alone();
    assert_a_real_span(&alone);

    let (rig, source) = CallRig::with_wav();
    call::request(rig.handle(), CallRequest::Record(CallSource::Switch));
    assert!(
        call_shell_support::eventually(|| rig.recording()),
        "the short call did not start"
    );
    source.pump_for(BEFORE_MS);
    source.pump_for(DURING_MS);
    source.pump_for(PUMP_MS - 100);
    rig.stop_call();
    let short = read_tracks(&only_folder(&rig.dir.path().join("calls")));

    assert_ne!(
        short, alone,
        "a call 100 ms short read back as the baseline"
    );
    assert!(
        short.mic < alone.mic && short.system < alone.system,
        "both tracks are shorter than the baseline: {short:?} vs {alone:?}"
    );
}
