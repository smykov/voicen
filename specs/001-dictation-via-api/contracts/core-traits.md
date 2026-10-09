# Contract: core traits (`voicen_core`)

These are the seams between the platform-independent core and the Windows shell or test fakes. The signatures are normative in shape; names may change only through a plan update. All traits are `Send + Sync` and synchronous (research R-7, R-10). The core never calls Windows APIs directly.

## Engine (NFR-11; shared with 002)

```rust
pub trait Engine: Send + Sync {
    /// Short stable name for logs ("api", "local_server", later "builtin").
    fn kind(&self) -> &'static str;
    /// Transcribe 16 kHz mono audio. Empty or whitespace-only text is returned as Ok(""),
    /// and the pipeline maps it to NoSpeech. Must never panic on server or engine data.
    fn transcribe(&self, audio: &AudioBuffer, req: &TranscribeRequest) -> Result<String, FailureReason>;
}

pub struct TranscribeRequest {
    pub language: Option<String>,    // Settings::speech_language; None = auto → field omitted
    pub timeouts: Timeouts,          // single source, timeouts.rs; the engine stores no Duration
}
```

`OpenAiCompatibleEngine` implements `Engine` (contract: [openai-transcription.md](openai-transcription.md)); it builds its blocking client per call from `req.timeouts`. Its endpoint role is fixed by the constructor (T-018): `OpenAiCompatibleEngine::new(base_url: NormalizedUrl, model, key: Option<Secret>)` is the API engine (`kind() == "api"`, deadline `Timeouts::api_transcription`, `model` always sent); `OpenAiCompatibleEngine::local_server(base_url, model: Option<String>, key: Option<Secret>)` is the local-server engine (`kind() == "local_server"`, deadline `Timeouts::local_server`, `model: None` omits the part). Both share the one `Authorization` rule.

The factory is one total function in core (T-040, decision #44):

```rust
pub fn engine_for(settings: &Settings, creds: &dyn CredentialStore)
    -> Result<Box<dyn Engine>, FailureReason>;
```

It takes 004's `Settings` snapshot (`engine`, `api.base_url`, `api.model`, `local_server.base_url`, `local_server.model`; no `DictationSettings` projection) and is called per job, so a retry uses the current settings and key (Clarification 4). It never panics and sends no request:

- `Api`: `check_base_url(api.base_url)` (else `EngineNotConfigured`, no key read) → `creds.read(KeySlot::TranscriptionApi)` once (`Err` → `KeyStoreUnavailable`; `Ok(None)` → no `Authorization` header) → `OpenAiCompatibleEngine`.
- `LocalServer` (T-018; 002's "register in src-tauri factory" became this arm): `check_base_url(local_server.base_url)` (else `EngineNotConfigured`, no key read) → `creds.read(KeySlot::LocalServer)` once (`Err` → `KeyStoreUnavailable`; `Ok(None)` → no `Authorization` header) → `OpenAiCompatibleEngine::local_server` with `local_server.model` trimmed, or no model (the part omitted) when it is empty or whitespace-only.
- `BuiltinLocal`, `None`: `EngineNotConfigured`, no key read. T-017 wraps `engine_for` in the shell for `BuiltinLocal` (no whisper in core, decision #12).

## SpeechDetector

```rust
pub trait SpeechDetector: Send + Sync {
    fn name(&self) -> &'static str;                       // "silero" | "energy"
    fn contains_speech(&self, audio: &AudioBuffer) -> Result<bool, VadError>;
}
```

```rust
#[non_exhaustive]
pub enum VadError { Unavailable, Failed }   // static Display texts: no path, no audio

impl SpeechGate {
    pub fn new(primary: Result<Box<dyn SpeechDetector>, VadError>, fallback: EnergyDetector) -> SpeechGate;
    pub fn decide(&self, audio: &AudioBuffer) -> GateDecision;   // infallible; Send + Sync, shared by job workers
}
pub struct GateDecision { pub speech: bool, pub detector: &'static str, pub fallback_warning: bool }
```

`decide` returns the primary's `Ok(answer)` with the primary's name. If `primary` is `Err`, or returns `Err` at run time, that recording and every later one is decided by the fallback (`detector: "energy"`); the runtime error latches, and the primary is never called again on that gate. A primary error is never treated as speech. Exactly one decision per gate carries `fallback_warning: true` (an atomic swap, so also under concurrent workers). The gate emits nothing: the pipeline (T-001 `run_job`) turns the decision into the `SpeechGate` event and, on `fallback_warning`, the one `Warning{vad_fallback}`. `EnergyDetector` is infallible (`detect(&AudioBuffer) -> bool`; its `contains_speech` is always `Ok`); its rule is research.md R-5.

## PostProcessor (FR-021; 003 implements)

In `voicen_core::post_process` (the module that already holds 003's settings type; T-001):

```rust
pub trait PostProcessor: Send + Sync {
    /// Returns the text to deliver. Must not fail the dictation: on its own failure it returns
    /// the input text plus a notice (003 defines the notice).
    fn process(&self, text: String, settings: &Settings) -> PostProcessed;   // the job's snapshot
}
pub struct PostProcessed { pub text: String, pub notice: Option<MessageId> }
```

`PassThrough` returns `PostProcessed{ text, notice: None }` (the text unchanged, not trimmed). The pipeline calls it only for a non-blank engine text; a blank result is `NoSpeech`. T-020 widens the arguments (credentials, timeouts) and decides how its notice combines with a delivery notice (`JobEnd::Delivered` has one slot; until then the delivery notice wins, else the post-processor's; this interim rule is untested, since no post-processor produces a notice before T-020, which decides it and pins it with a test).

## Platform traits (implemented in `src-tauri/src/win/*`, faked in tests)

```rust
// Capture (built in T-051, `voicen_core::platform`; the default input device — T-012 adds
// `list_devices`, a device argument and device loss through the sink, FR-031).
pub trait AudioSource: Send + Sync {
    /// Opens the device and starts delivering frames to `sink`.
    fn start(&self, sink: Arc<dyn FrameSink>) -> Result<Box<dyn CaptureHandle>, CaptureError>;
}
pub trait FrameSink: Send + Sync {
    /// Called on the adapter's audio thread; must not block. Interleaved f32 in [-1, 1];
    /// `at` is stamped by the adapter in its callback (the first one gives
    /// `RecordingStarted.hotkey_to_first_frame_ms`).
    fn frames(&self, interleaved: &[f32], rate: u32, channels: u16, at: Instant);
}
pub trait CaptureHandle: Send {
    /// Once `stop` returns, or the handle is dropped, the sink is called no more and the
    /// device is closed (NFR-02).
    fn stop(self: Box<Self>) -> Result<(), CaptureError>;
}
// `AudioSource::start` is called by the dictation session at a press, from inside its lock;
// the session's `Drop` drops a live `CaptureHandle` inside it too (`stop` runs outside it).
// So `start` and dropping a handle must return promptly, must never call back into the
// session (an error or device-loss path included) and must not wait for another call on the
// same source or its handles (a `stop` on another thread) or for a thread that calls the
// session (an audio callback reporting to it).
pub enum CaptureError { NoDevice, AccessDenied, DeviceBusy, Other(String /* OS error code text, no audio */) }   // voicen_core::recording::CaptureError (T-042)

pub trait Clipboard: Send + Sync {
    /// Writes text with the history/cloud exclusion formats (FR-022). Retries internally.
    fn set_text_excluded_from_history(&self, text: &str) -> Result<(), ClipboardError>;
}

pub trait Paster: Send + Sync {
    fn capture_start_window(&self) -> Option<StartWindow>;
    /// Waits ≤ `max_wait` for Shift/Ctrl/Alt/Win to be released; false if still held.
    fn wait_modifiers_released(&self, max_wait: Duration) -> bool;
    fn is_in_front(&self, w: &StartWindow) -> bool;
    fn send_ctrl_v(&self) -> Result<(), PasteError>;
}
// `capture_start_window` is called by the dictation session at a press, from inside its lock;
// the other three run on the session's worker during a delivery, with no lock held. So
// `capture_start_window` must return promptly, must never call back into the session and
// must not wait for another call on the same paster: no one mutex held across the methods,
// or a press would wait up to `MODIFIER_WAIT` (1 s) behind the worker's modifier wait, and
// the tray, the timer and job ends with it.

pub trait Notifier: Send + Sync {
    /// Toast. `retry` is Some(pending_id) for retryable failures. A failure to show is not an error
    /// for the pipeline (the overlay and tray carry the message).
    fn notify(&self, key: MessageKey, params: &MessageParams, retry: Option<PendingId>);
}

pub trait Indicator: Send + Sync {                        // built in T-051, `voicen_core::platform`
    fn set_tray(&self, state: TrayState, retry_available: bool);
    fn set_overlay(&self, state: &OverlayState);           // shell forwards it as the IPC event
}
// Called only by the dictation session, from inside its lock, once per change; an
// implementation must not block on another thread and must never call back into the session
// (T-052 / T-053 hop to the main thread with a non-waiting emit or run_on_main_thread).
// The tray half is `src-tauri` `tray::TrayPart::set_tray` (T-052; T-006's `ShellIndicator` (`src-tauri/src/dictation.rs`; its overlay half is a no-op until T-057)
// forwards to it): it stores the latest (state, retry_available) and posts one fire-and-forget
// main-thread task that renders it with core's `voicen_core::tray` table; it never calls a
// TrayIcon setter itself (those wait for the main thread). The tray's "menu opened" reaches
// `DictationSession::tray_menu_opened` from another thread, never from the main thread.

pub trait CancelKey: Send + Sync {                        // T-009, `voicen_core::platform`; fake FakeCancelKey
    fn set(&self, claimed: bool);                          // claim Esc (true) / give it back (false)
}
// The Esc claim (FR-22): the session calls it from inside its lock, once per change of
// `live_id().is_some()` (released at the stop, not at the finish; a failed capture gets no
// claim). Non-blocking, never calls back; the result comes later as
// `DictationSession::cancel_key_result`, an Esc press as `esc_pressed`. Windows:
// `win::hotkey::CancelKeyHandle`, served by the hotkey thread (bare Esc and Esc under the
// hotkey's modifiers, OQ-21 (1)), built and attached inside `start_dictation` (not a
// `DictationPorts` field).

pub trait ShellRequests: Send + Sync {                    // built in T-051, `voicen_core::platform`
    fn open_settings(&self, tab: SettingsTab);            // the OpenSettings of `blocked_actions`; T-055 adds the field focus
}

// Clipboard, Paster, TempAudioStore: `voicen_core::platform` (T-001), with public fakes
// (FakeClipboard, FakePaster, FakeTempAudioStore; feature `test-fakes`). ClipboardError and
// PasteError are unit structs (no OS text, P-009).
pub struct StartWindow { pub handle: WindowRef /* opaque HWND */, pub process_id: u32, pub elevated: bool }
pub struct PendingId(/* u64, monotonic per Pipeline, never reused */);

// CredentialStore, KeySlot and Secret are defined by 004 (`voicen_core::secrets`, decisions #21); 001 only reads:
//   store.read(KeySlot::TranscriptionApi) -> Result<Option<Secret>, CredentialError>   // None = no key stored (NFR-04)
// The Windows implementation is 004 T016; there is no second one.

// Windows implementations (T-006, `src-tauri`, Windows CI only): AudioSource `win/capture.rs` `CpalSource`
// (bounded open, `OPEN_BUDGET` 3 s), Clipboard `win/clipboard.rs` `WinClipboard`, Paster `win/paste.rs`
// `WinPaster`, Indicator `dictation.rs` `ShellIndicator`, ShellRequests `dictation.rs` `SettingsRequests`;
// `dictation::start_dictation` is the only builder of the session and the hotkey thread (`win/hotkey.rs`).

pub trait TempAudioStore: Send + Sync {
    fn put_pending(&self, id: PendingId, audio: &AudioBuffer) -> std::io::Result<()>;
    fn get_pending(&self, id: PendingId) -> std::io::Result<AudioBuffer>;
    fn delete_pending(&self, id: PendingId) -> std::io::Result<()>;
    fn delete_all(&self) -> std::io::Result<()>;           // at start and on exit (FR-032)
}

pub trait PipelineObserver: Send + Sync {                 // voicen_core::events; fake: RecordingObserver
    fn event(&self, e: &DictationEvent);                   // T-008 writes these to the log file
}
```

There is no `SettingsSource` and no `DictationSettings` projection (decision #47 (1)): a job carries the `Arc<Settings>` snapshot taken at press in its `PressContext`. `AudioSource` (with `FrameSink` and `CaptureHandle`), `Indicator` and `ShellRequests` are built in core (T-051) with public fakes (`FakeAudioSource` + `FrameChunk`, `FakeIndicator` + `IndicatorCall`, `FakeShellRequests` + `ShellRequestCall`; feature `test-fakes`) and `test_support::realtime::RealtimeSource` (frames from an `AudioBuffer` or PCM16 WAV bytes, pushed in 10 ms chunks paced by real time, silence after the data); their Windows implementations come with T-006 (capture), T-052 (tray) and T-053 (overlay). `Notifier` comes with T-007.

## Pipeline API (called by the dictation session)

Built (T-001, `voicen_core::pipeline`, decisions #43, #47): the job sequencer.

```rust
pub struct PressContext { pub start_window: Option<StartWindow>, pub settings: Arc<Settings> }   // the controller's C, taken at press
pub struct PipelineDeps {
    pub gate: SpeechGate,
    pub credentials: Arc<dyn CredentialStore>,             // the SettingsService's instance
    pub clipboard: Arc<dyn Clipboard>, pub paster: Arc<dyn Paster>,
    pub temp_audio: Arc<dyn TempAudioStore>, pub observer: Arc<dyn PipelineObserver>,
    pub post_processor: Arc<dyn PostProcessor>,
}
pub type EngineFactory = dyn Fn(&Settings, &dyn CredentialStore) -> Result<Box<dyn Engine>, FailureReason> + Send + Sync;
pub struct JobReport { pub end: JobEnd, pub pending: Option<PendingId> }

impl Pipeline {                                            // Send + Sync
    pub fn new(deps: PipelineDeps) -> Pipeline;            // per job: Timeouts::from_settings(&job.settings.timeouts), engine_for
    #[cfg(any(test, feature = "test-fakes"))]
    pub fn with_timeouts(deps: PipelineDeps, timeouts: Timeouts) -> Pipeline;
    pub fn with_engine_factory(self, factory: Box<EngineFactory>) -> Pipeline;   // T-017; test fakes
    pub fn run_job(&self, rec: FinishedRecording<PressContext>) -> JobReport;    // blocking; plain std thread, never in a tokio runtime
    pub fn pending(&self) -> Option<(PendingId, FailureReason)>;                 // tray retry_available (read on the job thread only), toast retry id
}
```

`run_job` is the only path a finished recording takes. The dictation session's worker reads `rec.id()` first and then calls `controller.job_finished(id, report.end, at)`. Inside, two private halves (T-011 puts the delivery queue between them):

- `process`: `gate.decide(audio)` → (speech only) `factory(&settings, &*credentials)` → `transcribe(audio, &TranscribeRequest{ language: settings.speech_language, timeouts })` with that job's `Timeouts::from_settings(&settings.timeouts)` (the job's settings snapshot; defaults 5/30/60 s, bounds per decision #99; `with_timeouts` overrides it in tests) → `post_processor.process`. No speech, a blank engine text or a blank post-processed text is `NoSpeech`; without speech there is no factory call, no credential read and no request.
- `release`: a text goes through `delivery::deliver` (clipboard first, then data-model "DeliveryDecision", `MODIFIER_WAIT` = 1 s); a clipboard error is `Failed(ClipboardUnavailable)`. A retryable failure stores the audio under a new `PendingId`, swaps the one pending slot to it (or to nothing, if the store failed) and deletes the audio it replaced (decision #47 (4)); id, store write and swap happen under the slot's one lock as an extra safeguard (the replaced audio is deleted after the lock is released); `Text` and `NoSpeech` leave the slot as it is. `release`, and so `run_job` until T-011 splits it, must run on one thread at a time: `deliver` (clipboard write → modifier wait ≤ 1 s → Ctrl+V) is not serialized across jobs, so concurrent jobs could paste one transcript into another's window (research R-10: one delivery thread; the dictation session runs `run_job` on its one FIFO worker, T-051). `process` may run concurrently (R-10 workers). The slot lock does not make concurrent jobs supported.
- Events, only on the one observer and in this order: `Warning{vad_fallback}` (when `GateDecision.fallback_warning`), `SpeechGate`, `JobFinished` (after the clipboard write), `Delivered` (only for a delivered text).

`JobEnd` mapping: `Pasted` → `Delivered{notice: None}`, `CopiedOnly` → `Delivered{notice.copied}`, `CopyManual` → `Delivered{notice.copied_paste_manually}`, `NoSpeech` → `Notice(notice.no_speech)`, `Failed(r)` → `Failed(r)`.

## Dictation session (`voicen_core::dictation`, T-051, decision #64)

Built: the platform-free owner that turns hotkey events into a delivered dictation; the shell's adapters stamp instants and carry out requests, they decide nothing.

```rust
pub struct SessionDeps {
    pub pipeline: PipelineDeps,                            // the session builds its one Pipeline from it
    pub engine_factory: Option<Box<EngineFactory>>,       // None: engine_for
    pub audio: Arc<dyn AudioSource>,
    pub indicator: Arc<dyn Indicator>,
    pub requests: Arc<dyn ShellRequests>,
    pub settings: Arc<SettingsService>,                    // snapshot() at each press (P-013)
    pub cancel_key: Arc<dyn CancelKey>,                    // the Esc claim (T-009)
}
impl DictationSession {                                    // Send + Sync; inputs take &self and the caller's instant
    pub fn start(deps: SessionDeps) -> io::Result<DictationSession>;   // spawns the worker and the timer; Err: the spawn error's kind
    pub fn hotkey_pressed(&self, at: Instant);
    pub fn hotkey_released(&self, at: Instant);
    pub fn tray_menu_opened(&self, at: Instant);
    pub fn hotkey_registration(&self, registered: bool, at: Instant);
    pub fn esc_pressed(&self, at: Instant);                // T-009: cancels the recording that is on
    pub fn cancel_key_result(&self, claimed: bool, at: Instant);   // T-009: false while live → Warning{EscUnavailable}
    pub fn tick(&self, at: Instant);                       // T-009: the timer's input (message expiry, max length)
}
impl Drop for DictationSession { /* the job in flight finishes, queued recordings are dropped, threads joined */ }
```

- **Press.** While a recording is on (`live_id()`), the controller decides by the mode of the press that started it (`press_while_live`): a hold recording ignores it (auto-repeat), a toggle recording stops through the stop path (below); no gate, no start window, no capture either way. Otherwise `snapshot()` is gated with `dictation_gate`; when blocked, `blocked_actions` run in order: `Notify(id)` → `controller.notice(id, at)` (overlay message, tray unchanged), `OpenSettings(tab)` → `requests.open_settings(tab)` after the lock is released, and exactly one `PressBlocked { reason }` is emitted after the lock is released (one per press, not per action; no `RecordingId` is taken; T-006). Else `PressContext { start_window: paster.capture_start_window(), settings }` goes to `press`, and `audio.start(sink)` opens the capture; a start error → `capture_failed` and `CaptureFailed { recording, cause }`. The indicator is published only once the start result is known, so a failed capture never shows a recording state. Because the lock is held from `press` to the start result, `capture_start_window` and `AudioSource::start` (and, in `Drop`, dropping a live `CaptureHandle`) run under it: their implementations follow the rule stated with those traits in "Platform traits" (return promptly, never call the session, never wait for another call on the same port). The snapshot's `Settings.mode` goes to `press_with_mode` and governs that recording to its end (P-013; T-009 ends decision #63's "toggle behaves as hold").
- **Stop path (T-009).** A hold release, a toggle press and the max-length tick all end a recording through one private `stop_recording`: `releasing` is taken first, then the lock for the controller's decision. **Release.** `release` under the lock (a toggle recording ignores it); `CaptureHandle::stop` outside it, before the input returns (device closed, NFR-02). Then `RecordingStarted` (only if a frame arrived: first frame instant − press instant, `device: Selected`) and `RecordingEnded` (`Release::end()`, `Release::held()`). For a stop the frames become audio on the caller's thread (mixed down to mono as they arrive, `audio::mix_to_mono`; `AudioBuffer::from_frames` at the stop; a format change within one capture or unconvertible frames → `CaptureError::Other` with a fixed text), then `finish` and the push onto the FIFO happen in one critical section, so queue order = finish order; releases are serialised, so that is also recording order. `finish(Err)` → `CaptureFailed`.
- **Worker.** One std thread owns the `Pipeline` and is the only caller of `run_job` and `job_finished` (decision #48): `run_job` with no lock held, then `pending().is_some()` (the slot itself, read on the only thread that changes it) as `retry_available`, then `job_finished(id, report.end, Instant::now())` and publish under the lock.
- **Timer.** One std thread calls `tick(now)` once `now >= next_deadline()` (the message expiry or the recording's `MAX_LENGTH`), and re-reads the deadline after every change. It ticks with the lock released, through the stop path, so a max-length stop is queued in recording order.
- **Esc (T-009).** `esc_pressed`: `cancel` under the lock; the capture is closed outside it before the input returns (NFR-02), the audio dropped unread, `RecordingEnded{Cancelled}` emitted; nothing with no recording on (idle, or a stop in flight). The claim (`CancelKey::set`) follows `live_id().is_some()` from `publish`, not held back by a stop in flight. `cancel_key_result(false)` while a recording is on emits one `Warning{EscUnavailable}`; the recording goes on.
- **Publication.** `(indicator.tray, retry_available)` and `indicator.overlay` reach the `Indicator` port from inside the session lock, right after the controller call that changed them, only when they differ from what was last sent; nothing at start. Between a stop's `release` and its `finish` nothing is sent (from any thread): the `finish` sends what changed meanwhile, so the overlay goes Recording → Processing, never through Hidden. If the release path panics in between (an adapter's `stop`, the observer, the conversion), a drop guard ends that hold and publishes what the controller shows, and the panic reaches the caller.
- The shell implements the hotkey thread (research R-1, R-2, R-17) and turns OS messages into these calls. The `RegisterHotKey` modifiers and virtual key, the release poll groups and rule, and the "target is elevated" rule come from `voicen_core::win32_data` (pure data: `hotkey_codes`, `released`, `target_elevated` over `IntegrityLevel` RIDs; T-051), which T-006 cross-checks against the `windows` crate constants on Windows CI.
- Later inputs go through the same session: Retry and toasts (T-007), registrar results at start and on save (T-055), device choice and device loss (T-012), the delivery queue that replaces the worker (T-011). App exit does not go through the session (T-052, which settles T-051 Q3 for exit): the process ends by tao's `process::exit` after `RunEvent::Exit`, the session is never dropped, and in-flight and queued results are dropped with the process (spec.md edge case "app exit"); deleting the pending audio on exit is T-007's.

There is no clock port in this feature. Every `Instant` is the instant of the caller's event: the hotkey thread stamps the press and the release when the OS message arrives, so worker or lock delay never counts in the 0.3 s hold. `run_job` reads `std::time::Instant::now()` only for the two event durations (`stop_to_text_ms` from the recording's `stopped_at`, `text_to_paste_ms`), never for a decision (decision #47 (2)). The session's worker is the caller of `job_finished`, so it stamps the job-end instant (`Instant::now()` when `run_job` returned); the session's timer passes `Instant::now()` to `tick` once the deadline is reached. `voicen_core::clock::Clock` (wall time, used by `SettingsService`) is unchanged (T-042).

## RecordingController and IndicatorState (`voicen_core::recording`, T-042)

Hold and toggle mode, the 10-minute maximum and Esc (T-009). Sans-IO: no threads, no capture, no clock; `&mut self`, the dictation session serialises the calls under its lock (T-051). The controller is the only place that decides a recording starts or ends, the only constructor of a `FinishedRecording`, and the only owner of `IndicatorState`.

```rust
pub const MIN_HOLD: Duration;            // 300 ms; a hold of exactly 300 ms is kept
pub const MESSAGE_DURATION: Duration;    // 3 s
pub const MAX_LENGTH: Duration;          // 600 s, either mode (decision #1, T-009)

impl<C> RecordingController<C> {         // C: opaque press context, returned with the recording (the session: PressContext)
    pub fn press(&mut self, at: Instant, ctx: C) -> Press;        // hold-mode entry: Start(RecordingId) | Ignored (any press while recording)
    pub fn press_with_mode(&mut self, at: Instant, mode: Mode, ctx: C) -> PressOutcome<C>;   // T-009: Start | Stop(ticket, Toggled) | Ignored
    pub fn press_while_live(&mut self, at: Instant) -> Option<PressOutcome<C>>;            // T-009: None while idle
    pub fn release(&mut self, at: Instant) -> Release<C>;         // Stop(StopTicket<C>) | Discarded{id, held} | Ignored (also every toggle release); .end(): Released | TooShort | None; .held(): the hold for Stop and Discarded
    pub fn cancel(&mut self, at: Instant) -> Option<RecordingId>; // T-009 Esc: live → idle, no ticket, no job, no message; None otherwise
    pub fn capture_failed(&mut self, id: RecordingId, err: CaptureError, at: Instant)
        -> Option<FailureReason>;                                 // live id: idle + MicrophoneUnavailable{cause}; stale id: None
    pub fn finish(&mut self, ticket: StopTicket<C>, audio: Result<AudioBuffer, CaptureError>, at: Instant)
        -> Result<FinishedRecording<C>, FailureReason>;           // Ok queues the job; Err: MicrophoneUnavailable{cause}
    pub fn job_finished(&mut self, id: RecordingId, end: JobEnd, at: Instant);
    pub fn tray_menu_opened(&mut self, at: Instant);              // clears tray Error
    pub fn hotkey_registration(&mut self, registered: bool, at: Instant);   // false: tray HotkeyError; only true clears it (T-051)
    pub fn notice(&mut self, id: MessageId, at: Instant);         // a message for 3 s, tray unchanged (JobEnd::Notice's rule; T-051)
    pub fn live_id(&self) -> Option<RecordingId>;                 // the recording that is on (T-051)
    pub fn tick(&mut self, at: Instant) -> Option<StopTicket<C>>; // expires the message at its `until`; at started_at + MAX_LENGTH: ticket (MaxLength), notice.max_length 3 s, tray unchanged
    pub fn next_deadline(&self) -> Option<Instant>;               // min(message until, started_at + MAX_LENGTH)
    pub fn indicator(&self) -> &IndicatorState;                   // { tray: TrayState, overlay: OverlayState }
}
pub enum JobEnd { Delivered { notice: Option<MessageId> }, Notice(MessageId), Failed(FailureReason) }
pub enum MicCause { NoDevice, AccessDenied, Busy, Other }       // CaptureError without the OS text
```

- `StopTicket` is not `Clone` and has a private constructor; `finish` consumes it. So one press yields at most one `FinishedRecording` (`end = Released`, `started_at` = press instant, `stopped_at` = release instant), and none for a hold under `MIN_HOLD` (measured with `saturating_duration_since`), a capture error, or a stale id. A press is accepted again as soon as `release` returns (FR-029).
- `capture_failed` (live id) and `finish(Err)` show the `failure.microphone_unavailable` message for 3 s and set tray `Error`; the caller still gets the reason for its toast and event. `MicrophoneUnavailable` is not retryable.
- A message lasts 3 s from the event that raised it. A message raised during a live recording (for example `finish(Err)` of the previous recording, FR-029) is not shown while the recording is on; after its release it is shown for the rest of its 3 s, and not at all if they have passed.
- A job counts from `finish(Ok)` until `job_finished` for its id; an unknown or repeated id changes nothing. `Delivered` clears tray `Error` and shows its notice if any; `Notice` shows a message; `Failed` shows the reason's message and sets tray `Error`.
- Tray priority `HotkeyError > Recording > Error > Idle`; overlay priority `Recording > Message > Processing (≥ 1 queued job) > Hidden`. A press drops the message; it is not shown again. `hotkey_registration(false, at)` sets `HotkeyError`; only `hotkey_registration(true, at)` clears it (a delivery, the tray menu or a tick do not), and it changes nothing else.
- The controller emits no events: the dictation session builds `RecordingStarted`/`RecordingEnded`/`CaptureFailed` (and, for a blocked press, `PressBlocked` from the gate's `Blocked`) from the returned id, instants, `end` and `held` (`Release::end()`: `TooShort` for a discard, the ticket's `Released` for a stop, the same value as `FinishedRecording::end`). T-009 added toggle (`Toggled`), the 10-minute maximum (`MaxLength`) and Esc (`Cancelled`; no ticket, the session emits `RecordingEnded{Cancelled}` itself); T-010 / T-012 (suspend, device lost) add inputs and `RecordingEnd` variants to the same controller. The engine = none check stays in `settings::gate::dictation_gate`, which the dictation session calls before `press`.
