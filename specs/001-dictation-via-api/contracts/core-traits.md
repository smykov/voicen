# Contract: core traits (`voicen_core`)

These are the seams between the platform-independent core and the Windows shell or test fakes. The signatures are normative in shape; names may change only through a plan update. All traits are `Send + Sync` and synchronous (research R-7, R-10). The core never calls Windows APIs directly.

## Engine (NFR-11; shared with 002)

```rust
pub trait Engine: Send + Sync {
    /// Short stable name for logs ("api", later "builtin", "local_server").
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

`OpenAiCompatibleEngine::new(base_url: NormalizedUrl, model, key: Option<Secret>)` implements `Engine` (contract: [openai-transcription.md](openai-transcription.md)); it builds its blocking client per call from `req.timeouts`. 002 reuses it for the local server with `Timeouts::local_server`.

The factory is one total function in core (T-040, decision #44):

```rust
pub fn engine_for(settings: &Settings, creds: &dyn CredentialStore)
    -> Result<Box<dyn Engine>, FailureReason>;
```

It takes 004's `Settings` snapshot (`engine`, `api.base_url`, `api.model`; no `DictationSettings` projection) and is called per job, so a retry uses the current settings and key (Clarification 4). It never panics and sends no request:

- `Api`: `check_base_url(api.base_url)` (else `EngineNotConfigured`, no key read) → `creds.read(KeySlot::TranscriptionApi)` once (`Err` → `KeyStoreUnavailable`; `Ok(None)` → no `Authorization` header) → `OpenAiCompatibleEngine`.
- `BuiltinLocal`, `LocalServer`, `None`: `EngineNotConfigured`, no key read. T-018 adds the `LocalServer` arm here (002's "register in src-tauri factory" becomes this arm); T-017 wraps `engine_for` in the shell for `BuiltinLocal` (no whisper in core, decision #12).

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
pub trait AudioSource: Send + Sync {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, CaptureError>;   // DeviceInfo { id, name, is_default }
    /// Opens the device and starts capture; frames are delivered to `sink` until the returned
    /// handle is dropped. A device loss is reported through `sink.on_end(DeviceLost)`.
    fn start(&self, device: &DeviceId, sink: Box<dyn FrameSink>) -> Result<CaptureHandle, CaptureError>;
}
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

pub trait Notifier: Send + Sync {
    /// Toast. `retry` is Some(pending_id) for retryable failures. A failure to show is not an error
    /// for the pipeline (the overlay and tray carry the message).
    fn notify(&self, key: MessageKey, params: &MessageParams, retry: Option<PendingId>);
}

pub trait Indicator: Send + Sync {
    fn set_tray(&self, state: TrayState, retry_available: bool);
    fn set_overlay(&self, state: &OverlayState);           // shell forwards it as the IPC event
}

// Clipboard, Paster, TempAudioStore: `voicen_core::platform` (T-001), with public fakes
// (FakeClipboard, FakePaster, FakeTempAudioStore; feature `test-fakes`). ClipboardError and
// PasteError are unit structs (no OS text, P-009).
pub struct StartWindow { pub handle: WindowRef /* opaque HWND */, pub process_id: u32, pub elevated: bool }
pub struct PendingId(/* u64, monotonic per Pipeline, never reused */);

// CredentialStore, KeySlot and Secret are defined by 004 (`voicen_core::secrets`, decisions #21); 001 only reads:
//   store.read(KeySlot::TranscriptionApi) -> Result<Option<Secret>, CredentialError>   // None = no key stored (NFR-04)
// The Windows implementation is 004 T016; there is no second one.

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

There is no `SettingsSource` and no `DictationSettings` projection (decision #47 (1)): a job carries the `Arc<Settings>` snapshot taken at press in its `PressContext`. `AudioSource` is built with T-006, `Notifier` and `Indicator` with T-006/T-007.

## Pipeline API (called by the shell)

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
    pub fn new(deps: PipelineDeps) -> Pipeline;            // Timeouts::default(), engine_for
    #[cfg(any(test, feature = "test-fakes"))]
    pub fn with_timeouts(deps: PipelineDeps, timeouts: Timeouts) -> Pipeline;
    pub fn with_engine_factory(self, factory: Box<EngineFactory>) -> Pipeline;   // T-017; test fakes
    pub fn run_job(&self, rec: FinishedRecording<PressContext>) -> JobReport;    // blocking; plain std thread, never in a tokio runtime
    pub fn pending(&self) -> Option<(PendingId, FailureReason)>;                 // tray retry_available, toast retry id
}
```

`run_job` is the only path a finished recording takes. The shell reads `rec.id()` first and then calls `controller.job_finished(id, report.end, at)`. Inside, two private halves (T-011 puts the delivery queue between them):

- `process`: `gate.decide(audio)` → (speech only) `factory(&settings, &*credentials)` → `transcribe(audio, &TranscribeRequest{ language: settings.speech_language, timeouts })` with the pipeline's one `Timeouts` → `post_processor.process`. No speech, a blank engine text or a blank post-processed text is `NoSpeech`; without speech there is no factory call, no credential read and no request.
- `release`: a text goes through `delivery::deliver` (clipboard first, then data-model "DeliveryDecision", `MODIFIER_WAIT` = 1 s); a clipboard error is `Failed(ClipboardUnavailable)`. A retryable failure stores the audio under a new `PendingId`, swaps the one pending slot to it (or to nothing, if the store failed) and deletes the audio it replaced (decision #47 (4)); id, store write and swap happen under the slot's one lock, so concurrent failing jobs leave the audio of the last one to take the lock in the slot, and a job reports only an id it put there itself (the replaced audio is deleted after the lock is released); `Text` and `NoSpeech` leave the slot as it is.
- Events, only on the one observer and in this order: `Warning{vad_fallback}` (when `GateDecision.fallback_warning`), `SpeechGate`, `JobFinished` (after the clipboard write), `Delivered` (only for a delivered text).

`JobEnd` mapping: `Pasted` → `Delivered{notice: None}`, `CopiedOnly` → `Delivered{notice.copied}`, `CopyManual` → `Delivered{notice.copied_paste_manually}`, `NoSpeech` → `Notice(notice.no_speech)`, `Failed(r)` → `Failed(r)`.

Target (T-006, T-007, T-009; not built): whether a core facade wraps the controller and the pipeline with the calls below.

```rust
impl Pipeline {
    pub fn on_hotkey_pressed(&self, at: Instant, start_window: Option<StartWindow>);
    pub fn on_hotkey_released(&self, at: Instant);         // hold mode only
    pub fn on_escape(&self);
    pub fn on_suspend(&self);                              // stop + process (spec edge case)
    pub fn on_tray_menu_opened(&self);                     // clears TrayState::Error
    pub fn retry(&self, id: Option<PendingId>);            // None = tray "Retry last failed dictation"
    pub fn on_hotkey_registration(&self, result: Result<(), HotkeyError>, phase: RegistrationPhase);
    pub fn shutdown(&self);                                // drop in-flight results, delete temp audio
}
```

The shell implements the hotkey thread (research R-1, R-2, R-17) and turns OS messages into these calls. Every decision — what to record, discard, send, deliver or show — is taken inside `Pipeline`.

There is no clock port in this feature. Every `Instant` is the instant of the caller's event: the hotkey thread stamps the press and the release when the OS message arrives, so worker or lock delay never counts in the 0.3 s hold. `run_job` reads `std::time::Instant::now()` only for the two event durations (`stop_to_text_ms` from the recording's `stopped_at`, `text_to_paste_ms`), never for a decision (decision #47 (2)). `voicen_core::clock::Clock` (wall time, used by `SettingsService`) is unchanged (T-042).

## RecordingController and IndicatorState (`voicen_core::recording`, T-042)

Hold mode. Sans-IO: no threads, no capture, no clock; `&mut self`, the shell serialises the calls. The controller is the only place that decides a recording starts or ends, the only constructor of a `FinishedRecording`, and the only owner of `IndicatorState`.

```rust
pub const MIN_HOLD: Duration;            // 300 ms; a hold of exactly 300 ms is kept
pub const MESSAGE_DURATION: Duration;    // 3 s

impl<C> RecordingController<C> {         // C: opaque press context, returned with the recording (T-006: StartWindow)
    pub fn press(&mut self, at: Instant, ctx: C) -> Press;        // Start(RecordingId) | Ignored (auto-repeat while recording)
    pub fn release(&mut self, at: Instant) -> Release<C>;         // Stop(StopTicket<C>) | Discarded{id, held} | Ignored; idle on return; .end(): Released | TooShort | None
    pub fn capture_failed(&mut self, id: RecordingId, err: CaptureError, at: Instant)
        -> Option<FailureReason>;                                 // live id: idle + MicrophoneUnavailable{cause}; stale id: None
    pub fn finish(&mut self, ticket: StopTicket<C>, audio: Result<AudioBuffer, CaptureError>, at: Instant)
        -> Result<FinishedRecording<C>, FailureReason>;           // Ok queues the job; Err: MicrophoneUnavailable{cause}
    pub fn job_finished(&mut self, id: RecordingId, end: JobEnd, at: Instant);
    pub fn tray_menu_opened(&mut self, at: Instant);              // clears tray Error
    pub fn tick(&mut self, at: Instant);                          // expires the message at its `until`
    pub fn next_deadline(&self) -> Option<Instant>;               // when the shell's timer calls tick
    pub fn indicator(&self) -> &IndicatorState;                   // { tray: TrayState, overlay: OverlayState }
}
pub enum JobEnd { Delivered { notice: Option<MessageId> }, Notice(MessageId), Failed(FailureReason) }
pub enum MicCause { NoDevice, AccessDenied, Busy, Other }       // CaptureError without the OS text
```

- `StopTicket` is not `Clone` and has a private constructor; `finish` consumes it. So one press yields at most one `FinishedRecording` (`end = Released`, `started_at` = press instant, `stopped_at` = release instant), and none for a hold under `MIN_HOLD` (measured with `saturating_duration_since`), a capture error, or a stale id. A press is accepted again as soon as `release` returns (FR-029).
- `capture_failed` (live id) and `finish(Err)` show the `failure.microphone_unavailable` message for 3 s and set tray `Error`; the caller still gets the reason for its toast and event. `MicrophoneUnavailable` is not retryable.
- A message lasts 3 s from the event that raised it. A message raised during a live recording (for example `finish(Err)` of the previous recording, FR-029) is not shown while the recording is on; after its release it is shown for the rest of its 3 s, and not at all if they have passed.
- A job counts from `finish(Ok)` until `job_finished` for its id; an unknown or repeated id changes nothing. `Delivered` clears tray `Error` and shows its notice if any; `Notice` shows a message; `Failed` shows the reason's message and sets tray `Error`.
- Tray priority `HotkeyError > Recording > Error > Idle`; overlay priority `Recording > Message > Processing (≥ 1 queued job) > Hidden`. A press drops the message; it is not shown again. No input sets `HotkeyError` yet (T-006 adds hotkey registration).
- The controller emits no events: T-006/T-001 build `RecordingStarted`/`RecordingEnded` from the returned id, instants and `end` (`Release::end()`: `TooShort` for a discard, the ticket's `Released` for a stop, the same value as `FinishedRecording::end`). T-009 (toggle, 10-minute maximum, Esc) and T-006 (device lost, suspend) add inputs and `RecordingEnd` variants to the same controller. The engine = none check stays in `settings::gate::dictation_gate`, which the shell calls before `press`.
