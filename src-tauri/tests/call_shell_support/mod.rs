//! T-128: the shared rig of `call_launch.rs` and `call_tray.rs`: the shell's app wiring
//! (`build_app`, mock runtime) with the call part started through `voicen_lib::call::start_calls`
//! over fakes, and readers of what the shell did (the log file, the notifier, the tray record).
//! Windows only, like the tests that include it (decision #5).
//!
//! API pinned for the developer (docs/tasks/T-128.md `## Tests`; `voicen_lib::call` does not exist
//! yet, so every file that includes this module fails to compile until it does: that unresolved
//! import is the red):
//!
//! ```text
//! voicen_lib::call::
//!   CallPorts { source: Arc<dyn TrackSource + Send + Sync>, disk: Arc<dyn DiskSpace>,
//!               mono: Arc<dyn MonotonicClock>, open_track: TrackOpener,
//!               notifier: Arc<dyn Notifier>, calls_root: PathBuf }
//!       // `calls_root`: the root in force while `calls.storage_root` is unset (`<data_dir>/calls`
//!       // in `run()`), so a test never writes the runner's real data dir
//!   start_calls(app: &AppHandle<R>, ports: CallPorts) -> io::Result<()>
//!       // builds the one `Arc<CallStore>` and `Arc<CallSession>` and manages both (the app's
//!       // `try_state::<Arc<CallSession>>()` is how a test reads the session), starts the
//!       // `call-requests` worker and the `call-tick` thread
//!   CallRequest::{Record(CallSource), Stop(CallSource), RefuseSwitch}
//!   request(app: &AppHandle<R>, request: CallRequest)     // posts and returns; no-op without a call part
//!   apply_launch(app: &AppHandle<R>, args: &LaunchArgs)    // the first launch's `--call` switch
//!   open_calls(app: &AppHandle<R>, target: Option<CallRef>)   // T-142: the tray "Calls" route (None) and
//!                                                          // a targeted open; posts to the `calls` opener thread
//!   calls_opened(app: &AppHandle<R>) -> u32                // test-read: how many times `open_calls` ran
//!                                                          // (T-142: it still counts every request)
//!   CallRef { root: String, id: String }                   // T-142: the `{root, id}` of `calls://open`
//!   trait SaveDialog: Send + Sync                          // T-142: the native save dialog port
//!       { fn pick(&self, suggested_name: &str, format: Format) -> io::Result<Option<PathBuf>> }
//!                                                          // Ok(None) = cancelled; Format = core storage::Format
//!   CallPorts gains clipboard: Arc<dyn Clipboard>, folder: Arc<dyn FolderOpener>,
//!                   save: Arc<dyn SaveDialog>              // T-142
//! voicen_lib::tray::TrayPart::set_call_recording(&self, bool)
//! voicen_core::call::events::{refused_notice, CallNotice::message}   // tests/call_notice_map.rs (core)
//!
//! T-145 adds to `CallPorts`: `exit_confirm: Arc<dyn ExitConfirm>` (`voicen_lib::call::ExitConfirm`:
//! `trait ExitConfirm: Send + Sync { fn ask(&self, text: &str) -> bool }`, true = Yes; the
//! native Yes/No box of the tray's Exit while a call records). The rig's [`FakeExit`] is it.
//!
//! T-139 adds to `CallPorts`: `credentials: Arc<dyn CredentialStore>` (the store the settings
//! service writes; the rig's [`CallRig::creds`]) and `detector: Option<Arc<dyn SpeechSpans>>`
//! (`None`: core's energy fallback); `start_calls` then also manages the one
//! `Arc<voicen_core::call::final_pass::PassQueue>` the stop hand-off feeds.
//! ```
#![cfg(windows)]
#![allow(dead_code)]

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::{App, AppHandle, Manager};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::call::codec::SyncWrite;
use voicen_core::call::session::{CallSession, TrackFile, TrackOpener};
use voicen_core::call::storage::Format;
use voicen_core::call::wav_source::WavTrackSource;
use voicen_core::call::{
    CaptureStamp, EndpointId, OpenError, StopReason, Track, TrackDevice, TrackSink, TrackSource,
};
use voicen_core::clock::SystemMonotonic;
use voicen_core::diag::Log;
use voicen_core::local_models::catalog::MODELS;
use voicen_core::local_models::download::DiskSpace;
use voicen_core::local_models::service::LocalModels;
use voicen_core::local_models::store::ModelStore;
use voicen_core::platform::{CaptureHandle, Clipboard, FakeClipboard, FakeNotifier};
use voicen_core::recording::CaptureError;
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::service::SettingsService;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_core::vad::SpeechSpans;
use voicen_lib::call::{CallPorts, ExitConfirm, SaveDialog};
use voicen_lib::logs_folder::FolderOpener;
use voicen_lib::settings_ipc::load_settings;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

use crate::timing_support;

/// The second process's argv[0] and cwd as the single-instance plugin passes them (fake paths).
pub const EXE: &str = r"C:\Users\fake-user\AppData\Local\Voicen\voicen.exe";
pub const CWD: &str = r"C:\Users\fake-user";

/// Where the first block of a WAV track is stamped, and how far a test pumps it (ms).
pub const START_MS: u64 = 5_000;
pub const PUMP_MS: u64 = 2_000;

/// The length of one WAV track block (ms), and the step of `WavSource::pump_for` (T-266).
const BLOCK_MS: u64 = 20;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Dispatches every message queued for this thread (the mock's main thread).
pub fn pump() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid, writable MSG for the whole loop; the calls only read and
    // dispatch this thread's own queue.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Polls `cond`, pumping this thread's messages, until it holds or the hang bound passed.
pub fn eventually(cond: impl FnMut() -> bool) -> bool {
    timing_support::eventually_pumping(pump, cond)
}

/// Pumps for a short window; false as soon as `cond` fails (a bounded negative).
pub fn holds_for_a_while(cond: impl FnMut() -> bool) -> bool {
    timing_support::holds_for_pumping(pump, std::time::Duration::from_millis(500), cond)
}

pub fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

// ---- track sources ------------------------------------------------------------------------

/// A 2 s 16-bit mono 16 kHz WAV of a 440 Hz tone (silence would encode to almost nothing).
fn tone_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..32_000u32)
        .map(|i| {
            let phase = f64::from(i) * 440.0 * std::f64::consts::TAU / 16_000.0;
            (phase.sin() * 12_000.0) as i16
        })
        .collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&16_000u32.to_le_bytes());
    v.extend_from_slice(&32_000u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

/// A 16-bit mono 16 kHz WAV of `samples` (T-139: a speech clip for the Silero leg).
fn wav_of(samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&16_000u32.to_le_bytes());
    v.extend_from_slice(&32_000u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

/// A `TrackSource` that plays the tone on both tracks: every `open` makes a fresh
/// `WavTrackSource`, so a second call in the same test opens like the first. Nothing runs
/// until the test calls [`WavSource::pump`].
pub struct WavSource {
    opened: Mutex<Vec<Arc<WavTrackSource>>>,
    wav: Vec<u8>,
    /// T-151: opens not yet stopped or dropped (core's `WavHandle` counts nothing).
    open_handles: Arc<AtomicUsize>,
}

impl Default for WavSource {
    fn default() -> WavSource {
        WavSource {
            opened: Mutex::new(Vec::new()),
            wav: tone_wav(),
            open_handles: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl WavSource {
    pub fn new() -> Arc<WavSource> {
        Arc::new(WavSource::default())
    }

    /// T-139: both tracks play `samples` (16 kHz mono) instead of the tone.
    pub fn with_pcm(samples: &[i16]) -> Arc<WavSource> {
        Arc::new(WavSource {
            opened: Mutex::new(Vec::new()),
            wav: wav_of(samples),
            open_handles: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// T-139: pushes the due blocks of every track opened so far, up to `START_MS + ms`.
    ///
    /// T-266: the tracks are fed in stamp order: every opened track is stepped together, one
    /// block (`BLOCK_MS`) at a time, so the latest stamp fed to one track never leads another
    /// track's by more than one block (below the writer's 200 ms hold-off; a whole span per
    /// track in turn let `Writer::advance` zero-fill the lagging live track, F-036 draft).
    pub fn pump_for(&self, ms: u64) {
        let opened = lock(&self.opened);
        let mut step: u64 = 0;
        loop {
            let to = step.min(ms);
            for source in opened.iter() {
                source.pump(CaptureStamp::from_ms(START_MS + to));
            }
            if to == ms {
                break;
            }
            step += BLOCK_MS;
        }
    }

    /// Pushes the due blocks of every track opened so far, up to `START_MS + PUMP_MS`, in stamp
    /// order (`pump_for(PUMP_MS)`).
    pub fn pump(&self) {
        self.pump_for(PUMP_MS);
    }

    /// How many tracks were opened.
    pub fn opens(&self) -> usize {
        lock(&self.opened).len()
    }

    /// T-151: track handles opened and not yet stopped or dropped (a track capture is open).
    pub fn open_handles(&self) -> usize {
        self.open_handles.load(Ordering::SeqCst)
    }
}

/// T-151: a handle that counts itself in `WavSource::open_handles` from the open until its
/// `stop` returns or it is dropped, whichever comes first (once).
pub struct CountedHandle {
    inner: Option<Box<dyn CaptureHandle>>,
    open: Arc<AtomicUsize>,
}

impl CountedHandle {
    /// Counts `inner` as open in `open` (one more now, one less when it is stopped or dropped).
    pub fn open(inner: Box<dyn CaptureHandle>, open: &Arc<AtomicUsize>) -> Box<dyn CaptureHandle> {
        open.fetch_add(1, Ordering::SeqCst);
        Box::new(CountedHandle {
            inner: Some(inner),
            open: Arc::clone(open),
        })
    }

    fn close(&mut self) {
        if self.inner.take().is_some() {
            self.open.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl CaptureHandle for CountedHandle {
    fn stop(mut self: Box<Self>) -> Result<(), CaptureError> {
        let Some(inner) = self.inner.take() else {
            return Ok(());
        };
        let result = inner.stop();
        self.open.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

impl Drop for CountedHandle {
    fn drop(&mut self) {
        self.close();
    }
}

impl TrackSource for WavSource {
    fn open(
        &self,
        track: Track,
        device: TrackDevice,
        sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        let source = WavTrackSource::from_bytes(&self.wav)
            .map_err(|_| OpenError::Other)?
            .with_block_ms(BLOCK_MS)
            .with_start(CaptureStamp::from_ms(START_MS));
        let source = Arc::new(source);
        let (inner, endpoint) = source.open(track, device, sink)?;
        lock(&self.opened).push(source);
        Ok((CountedHandle::open(inner, &self.open_handles), endpoint))
    }

    fn now(&self) -> CaptureStamp {
        CaptureStamp::from_ms(START_MS)
    }
}

/// A `TrackSource` that refuses every open with `error` (the first failing part of a start is
/// the system track).
pub struct Refusing(pub OpenError);

impl TrackSource for Refusing {
    fn open(
        &self,
        _track: Track,
        _device: TrackDevice,
        _sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        Err(match self.0 {
            OpenError::NoDevice => OpenError::NoDevice,
            OpenError::AccessDenied => OpenError::AccessDenied,
            OpenError::Busy => OpenError::Busy,
            OpenError::Other => OpenError::Other,
        })
    }

    fn now(&self) -> CaptureStamp {
        CaptureStamp::from_ms(START_MS)
    }
}

// ---- track files --------------------------------------------------------------------------

/// Opens a plain file.
pub fn plain_opener() -> TrackOpener {
    Arc::new(|path: &Path| {
        let file: TrackFile = Box::new(File::create(path)?);
        Ok(file)
    })
}

/// Opens files whose next write fails once after `armed` is set, then every write succeeds
/// again (a one-shot fault: F-024). The header writes before `armed` is set succeed.
pub fn failing_once_opener(armed: Arc<AtomicBool>) -> TrackOpener {
    Arc::new(move |path: &Path| {
        let file: TrackFile = Box::new(FailOnce {
            file: File::create(path)?,
            armed: Arc::clone(&armed),
        });
        Ok(file)
    })
}

struct FailOnce {
    file: File,
    armed: Arc<AtomicBool>,
}

impl io::Write for FailOnce {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.armed.swap(false, Ordering::SeqCst) {
            return Err(io::Error::other("injected write failure"));
        }
        io::Write::write(&mut self.file, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut self.file)
    }
}

impl SyncWrite for FailOnce {
    fn sync_data(&mut self) -> io::Result<()> {
        File::sync_data(&self.file)
    }
}

// ---- the app ------------------------------------------------------------------------------

fn no_keys() -> Arc<dyn CredentialStore> {
    Arc::new(FakeCredentialStore::new())
}

fn no_autostart() -> Arc<dyn Autostart> {
    Arc::new(FakeAutostart::new())
}

fn idle_models() -> Arc<LocalModels> {
    let (models, cleanup) = LocalModels::open(
        TempDir::new().path().join("models"),
        FakeDisk::with_available(u64::MAX),
        Timeouts::default(),
        MODELS,
    );
    cleanup.expect("cleanup over a missing models dir");
    Arc::new(models)
}

/// One app as `run()` builds it (`build_app`, no call part yet) over a fresh data dir, its log
/// in `<dir>/logs`.
pub struct CallRig {
    pub dir: TempDir,
    pub logs: PathBuf,
    pub app: App<MockRuntime>,
    pub service: Arc<SettingsService>,
    /// T-139: the credential store the service writes and the call part's passes read.
    pub creds: Arc<dyn CredentialStore>,
    pub notifier: Arc<FakeNotifier>,
    /// T-145: the exit question's port; answers No until told otherwise.
    pub exit: Arc<FakeExit>,
    /// Ends the process with a named message when the test outlives `2 * HANG` (a build, a join
    /// or a drop that never returns). Declared last so it is dropped after the app.
    _watch: timing_support::HangWatch,
}

impl CallRig {
    /// An app with no call part (what `build_app` alone gives).
    pub fn bare() -> CallRig {
        CallRig::bare_with(no_keys())
    }

    /// T-139: [`bare`](Self::bare) over `creds` as the settings service's store.
    pub fn bare_with(creds: Arc<dyn CredentialStore>) -> CallRig {
        let watch = timing_support::watch_for_hang("the call rig was built");
        let dir = TempDir::new();
        let logs = dir.path().join("logs");
        let log: Arc<Log> = voicen_lib::diag::start(logs.clone(), Box::new(|_| {}));
        let store = Arc::new(ModelStore::new(dir.path().join("models"), MODELS));
        let (service, _) = load_settings(
            dir.path().to_path_buf(),
            Arc::clone(&creds),
            no_autostart(),
            store,
            None,
            &log,
        );
        let app = voicen_lib::build_app(
            mock_builder(),
            mock_context(noop_assets()),
            service.clone(),
            idle_models(),
            log,
        )
        .expect("mock app builds");
        CallRig {
            dir,
            logs,
            app,
            service,
            creds,
            notifier: Arc::new(FakeNotifier::new()),
            exit: Arc::new(FakeExit::default()),
            _watch: watch,
        }
    }

    /// `bare` plus `start_calls` over `source`, `disk` and `open_track`, the recording
    /// `FakeNotifier` as the shared notifier, the real monotonic clock.
    pub fn started(
        source: Arc<dyn TrackSource + Send + Sync>,
        disk: Arc<dyn DiskSpace>,
        open_track: TrackOpener,
    ) -> CallRig {
        let rig = CallRig::bare();
        rig.start(source, disk, open_track);
        rig
    }

    /// The call part started on a `bare` rig.
    pub fn start(
        &self,
        source: Arc<dyn TrackSource + Send + Sync>,
        disk: Arc<dyn DiskSpace>,
        open_track: TrackOpener,
    ) {
        self.start_ports(
            source,
            disk,
            open_track,
            Arc::new(FakeClipboard::new()),
            Arc::new(FakeFolder::default()),
            Arc::new(FakeSave::default()),
            None,
        );
    }

    /// T-139: [`start`](Self::start) with the pass worker's primary detector (the T-142
    /// ports are the defaults of `start`).
    pub fn start_with_detector(
        &self,
        source: Arc<dyn TrackSource + Send + Sync>,
        disk: Arc<dyn DiskSpace>,
        open_track: TrackOpener,
        detector: Option<Arc<dyn SpeechSpans>>,
    ) {
        self.start_ports(
            source,
            disk,
            open_track,
            Arc::new(FakeClipboard::new()),
            Arc::new(FakeFolder::default()),
            Arc::new(FakeSave::default()),
            detector,
        );
    }

    /// [`CallRig::start`] with the three T-142 ports given by the test.
    pub fn start_with(
        &self,
        source: Arc<dyn TrackSource + Send + Sync>,
        disk: Arc<dyn DiskSpace>,
        open_track: TrackOpener,
        clipboard: Arc<dyn Clipboard>,
        folder: Arc<dyn FolderOpener>,
        save: Arc<dyn SaveDialog>,
    ) {
        self.start_ports(source, disk, open_track, clipboard, folder, save, None);
    }

    /// The call part started with every port given.
    #[allow(clippy::too_many_arguments)]
    fn start_ports(
        &self,
        source: Arc<dyn TrackSource + Send + Sync>,
        disk: Arc<dyn DiskSpace>,
        open_track: TrackOpener,
        clipboard: Arc<dyn Clipboard>,
        folder: Arc<dyn FolderOpener>,
        save: Arc<dyn SaveDialog>,
        detector: Option<Arc<dyn SpeechSpans>>,
    ) {
        self.try_start_ports(source, disk, open_track, clipboard, folder, save, detector)
            .expect("the call part starts");
    }

    /// T-220: [`start`](Self::start) that returns what `start_calls` returned instead of
    /// expecting `Ok` (a second start on a rig that already has the call part is `Err`).
    pub fn try_start(
        &self,
        source: Arc<dyn TrackSource + Send + Sync>,
        disk: Arc<dyn DiskSpace>,
        open_track: TrackOpener,
    ) -> io::Result<()> {
        self.try_start_ports(
            source,
            disk,
            open_track,
            Arc::new(FakeClipboard::new()),
            Arc::new(FakeFolder::default()),
            Arc::new(FakeSave::default()),
            None,
        )
    }

    /// The call part started with every port given, the `io::Result` returned.
    #[allow(clippy::too_many_arguments)]
    fn try_start_ports(
        &self,
        source: Arc<dyn TrackSource + Send + Sync>,
        disk: Arc<dyn DiskSpace>,
        open_track: TrackOpener,
        clipboard: Arc<dyn Clipboard>,
        folder: Arc<dyn FolderOpener>,
        save: Arc<dyn SaveDialog>,
        detector: Option<Arc<dyn SpeechSpans>>,
    ) -> io::Result<()> {
        let calls_root = self.dir.path().join("calls");
        fs::create_dir_all(&calls_root).expect("the calls root");
        voicen_lib::call::start_calls(
            self.handle(),
            CallPorts {
                source,
                disk,
                mono: Arc::new(SystemMonotonic),
                open_track,
                notifier: self.notifier.clone(),
                calls_root,
                clipboard,
                folder,
                save,
                credentials: Arc::clone(&self.creds),
                detector,
                exit_confirm: self.exit.clone(),
            },
        )
    }

    /// A rig whose tracks play the tone, with plenty of free disk.
    pub fn with_wav() -> (CallRig, Arc<WavSource>) {
        let source = WavSource::new();
        let rig = CallRig::started(
            source.clone(),
            FakeDisk::with_available(100_000_000_000),
            plain_opener(),
        );
        (rig, source)
    }

    pub fn handle(&self) -> &AppHandle<MockRuntime> {
        self.app.handle()
    }

    /// The managed call session; `None` before `start_calls`.
    pub fn session(&self) -> Option<Arc<CallSession>> {
        self.handle()
            .try_state::<Arc<CallSession>>()
            .map(|s| Arc::clone(&s))
    }

    /// Whether a call records. Never blocks on the session lock: the call worker holds it while
    /// it opens captures and publishes the tray flag, and that publish may need this thread to
    /// pump its messages, so a test thread parked on the lock would wait for a worker that waits
    /// for it (T-128 hang, run 43577968221). It polls `try_is_recording`, pumping between polls,
    /// and panics with a named message at the hang bound.
    #[track_caller]
    pub fn recording(&self) -> bool {
        let Some(session) = self.session() else {
            return false;
        };
        let mut known = None;
        let answered = eventually(|| {
            known = session.try_is_recording();
            known.is_some()
        });
        assert!(
            answered,
            "HANG: the session lock stayed held for the whole hang bound while the test read \
             `is_recording` (test `{}`); a call worker is blocked under the lock",
            thread::current().name().unwrap_or("<unnamed>")
        );
        known.unwrap_or(false)
    }

    /// Stops a recording call like the user would (`StopReason::User`), on a helper thread, while
    /// this thread keeps pumping; panics with a named message at the hang bound.
    #[track_caller]
    pub fn stop_call(&self) {
        let Some(session) = self.session() else {
            return;
        };
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            let _ = session.stop(StopReason::User);
            let _ = done.send(());
        });
        // A helper that panicked drops its sender: that also ends the wait.
        let returned = eventually(|| !matches!(finished.try_recv(), Err(TryRecvError::Empty)));
        assert!(
            returned,
            "HANG: `CallSession::stop` did not return within the hang bound (test `{}`)",
            thread::current().name().unwrap_or("<unnamed>")
        );
    }

    /// The lines of `<logs>/voicen.log` that have every one of `words` as a whole word.
    pub fn lines(&self, words: &[&str]) -> Vec<String> {
        fs::read_to_string(self.logs.join("voicen.log"))
            .map(|text| {
                text.lines()
                    .filter(|l| words.iter().all(|w| l.split_whitespace().any(|x| x == *w)))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn count(&self, words: &[&str]) -> usize {
        self.lines(words).len()
    }
}

// ---- the T-142 ports ------------------------------------------------------------------------

/// A [`FolderOpener`] that records every folder it was asked to open and opens nothing.
#[derive(Default)]
pub struct FakeFolder {
    opened: Mutex<Vec<PathBuf>>,
    fail_next: Mutex<Option<io::ErrorKind>>,
}

impl FakeFolder {
    /// The next `open` (only that one) is recorded and then fails with `kind`.
    pub fn fail_next(&self, kind: io::ErrorKind) {
        *lock(&self.fail_next) = Some(kind);
    }

    pub fn opened(&self) -> Vec<PathBuf> {
        lock(&self.opened).clone()
    }
}

impl FolderOpener for FakeFolder {
    fn open(&self, dir: &Path) -> io::Result<()> {
        lock(&self.opened).push(dir.to_path_buf());
        match lock(&self.fail_next).take() {
            Some(kind) => Err(io::Error::from(kind)),
            None => Ok(()),
        }
    }
}

/// A [`SaveDialog`] that never shows a dialog: it answers `answer` (a picked target, or `None`
/// = cancelled) and records every ask.
#[derive(Default)]
pub struct FakeSave {
    answer: Mutex<Option<PathBuf>>,
    asked: Mutex<Vec<(String, Format)>>,
    fail_next: Mutex<Option<io::ErrorKind>>,
}

impl FakeSave {
    /// The next ask (only that one) is recorded and then fails with `kind`.
    pub fn fail_next(&self, kind: io::ErrorKind) {
        *lock(&self.fail_next) = Some(kind);
    }

    /// The next and every later ask is answered with `target` (`None`: cancelled).
    pub fn answer(&self, target: Option<PathBuf>) {
        *lock(&self.answer) = target;
    }

    /// Every ask: the suggested file name and the format.
    pub fn asked(&self) -> Vec<(String, Format)> {
        lock(&self.asked).clone()
    }
}

impl SaveDialog for FakeSave {
    fn pick(&self, suggested_name: &str, format: Format) -> io::Result<Option<PathBuf>> {
        lock(&self.asked).push((suggested_name.to_string(), format));
        if let Some(kind) = lock(&self.fail_next).take() {
            return Err(io::Error::from(kind));
        }
        Ok(lock(&self.answer).clone())
    }
}

// ---- the T-145 exit question --------------------------------------------------------------

#[derive(Default)]
struct ExitState {
    asked: Vec<String>,
    answer: bool,
    hold: bool,
    released: bool,
}

/// An [`ExitConfirm`] that shows no box: it records every ask (the question text) and answers
/// `false` (No) unless told otherwise. [`hold`](Self::hold) makes an ask block, like the real
/// box does, until [`release`](Self::release) answers it (bounded by the hang bound).
#[derive(Default)]
pub struct FakeExit {
    state: Mutex<ExitState>,
    changed: Condvar,
}

impl FakeExit {
    /// The next and every later ask answers `yes` at once.
    pub fn answer(&self, yes: bool) {
        lock(&self.state).answer = yes;
    }

    /// Every later ask blocks until [`release`](Self::release).
    pub fn hold(&self) {
        lock(&self.state).hold = true;
    }

    /// Answers the held ask (and any later one) with `yes`.
    pub fn release(&self, yes: bool) {
        let mut state = lock(&self.state);
        state.answer = yes;
        state.released = true;
        drop(state);
        self.changed.notify_all();
    }

    /// The text of every ask, in order.
    pub fn asked(&self) -> Vec<String> {
        lock(&self.state).asked.clone()
    }
}

impl ExitConfirm for FakeExit {
    fn ask(&self, text: &str) -> bool {
        let mut state = lock(&self.state);
        state.asked.push(text.to_string());
        if state.hold {
            let (guard, _) = self
                .changed
                .wait_timeout_while(state, crate::timing_support::HANG, |s| !s.released)
                .unwrap_or_else(PoisonError::into_inner);
            state = guard;
        }
        state.answer
    }
}
