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
//!   open_calls(app: &AppHandle<R>)                         // the tray "Calls" route (no-op until T-142)
//!   calls_opened(app: &AppHandle<R>) -> u32                // test-read: how many times `open_calls` ran
//! voicen_lib::tray::TrayPart::set_call_recording(&self, bool)
//! voicen_core::call::events::{refused_notice, CallNotice::message}   // tests/call_notice_map.rs (core)
//! ```
#![cfg(windows)]
#![allow(dead_code)]

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::{App, AppHandle, Manager};
use voicen_core::autostart::{Autostart, FakeAutostart};
use voicen_core::call::codec::SyncWrite;
use voicen_core::call::session::{CallSession, TrackFile, TrackOpener};
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
use voicen_core::platform::{CaptureHandle, FakeNotifier};
use voicen_core::secrets::{CredentialStore, FakeCredentialStore};
use voicen_core::settings::service::SettingsService;
use voicen_core::test_support::local_models::FakeDisk;
use voicen_core::test_support::TempDir;
use voicen_core::timeouts::Timeouts;
use voicen_lib::call::CallPorts;
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

/// A `TrackSource` that plays the tone on both tracks: every `open` makes a fresh
/// `WavTrackSource`, so a second call in the same test opens like the first. Nothing runs
/// until the test calls [`WavSource::pump`].
#[derive(Default)]
pub struct WavSource {
    opened: Mutex<Vec<Arc<WavTrackSource>>>,
}

impl WavSource {
    pub fn new() -> Arc<WavSource> {
        Arc::new(WavSource::default())
    }

    /// Pushes the due blocks of every track opened so far, up to `START_MS + PUMP_MS`.
    pub fn pump(&self) {
        for source in lock(&self.opened).iter() {
            source.pump(CaptureStamp::from_ms(START_MS + PUMP_MS));
        }
    }

    /// How many tracks were opened.
    pub fn opens(&self) -> usize {
        lock(&self.opened).len()
    }
}

impl TrackSource for WavSource {
    fn open(
        &self,
        track: Track,
        device: TrackDevice,
        sink: Arc<dyn TrackSink>,
    ) -> Result<(Box<dyn CaptureHandle>, EndpointId), OpenError> {
        let source = WavTrackSource::from_bytes(&tone_wav())
            .map_err(|_| OpenError::Other)?
            .with_block_ms(20)
            .with_start(CaptureStamp::from_ms(START_MS));
        let source = Arc::new(source);
        let opened = source.open(track, device, sink)?;
        lock(&self.opened).push(source);
        Ok(opened)
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
    pub notifier: Arc<FakeNotifier>,
    /// Ends the process with a named message when the test outlives `2 * HANG` (a build, a join
    /// or a drop that never returns). Declared last so it is dropped after the app.
    _watch: timing_support::HangWatch,
}

impl CallRig {
    /// An app with no call part (what `build_app` alone gives).
    pub fn bare() -> CallRig {
        let watch = timing_support::watch_for_hang("the call rig was built");
        let dir = TempDir::new();
        let logs = dir.path().join("logs");
        let log: Arc<Log> = voicen_lib::diag::start(logs.clone(), Box::new(|_| {}));
        let store = Arc::new(ModelStore::new(dir.path().join("models"), MODELS));
        let (service, _) = load_settings(
            dir.path().to_path_buf(),
            no_keys(),
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
            notifier: Arc::new(FakeNotifier::new()),
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
            },
        )
        .expect("the call part starts");
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
