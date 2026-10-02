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
    pub language: Language,          // Auto → field omitted
    pub timeouts: Timeouts,          // single source, timeouts.rs
}
```

`OpenAiCompatibleEngine::new(base_url, model, key: Option<SecretString>, client_cfg)` implements `Engine` (contract: [openai-transcription.md](openai-transcription.md)). 002 reuses it for the local server with `Timeouts::local_server`.

`EngineFactory: Fn(&DictationSettings, &dyn CredentialStore) -> Result<Box<dyn Engine>, FailureReason>`. It is called per job, so a retry uses the current settings and key (Clarification 4).

## SpeechDetector

```rust
pub trait SpeechDetector: Send + Sync {
    fn name(&self) -> &'static str;                       // "silero" | "energy"
    fn contains_speech(&self, audio: &AudioBuffer) -> Result<bool, VadError>;
}
```

`SpeechGate::new(primary: Result<Box<dyn SpeechDetector>, VadError>, fallback: EnergyDetector)`. If `primary` is `Err`, or returns `Err` at run time, the gate uses the fallback and emits `Warning{vad_fallback}` once.

## PostProcessor (FR-021; 003 implements)

```rust
pub trait PostProcessor: Send + Sync {
    /// Returns the text to deliver. Must not fail the dictation: on its own failure it returns
    /// the input text plus a notice (003 defines the notice).
    fn process(&self, text: String, settings: &DictationSettings) -> PostProcessed;
}
pub struct PostProcessed { pub text: String, pub notice: Option<MessageKey> }
```

`PassThrough` returns `PostProcessed{ text, notice: None }`.

## Platform traits (implemented in `src-tauri/src/win/*`, faked in tests)

```rust
pub trait AudioSource: Send + Sync {
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, CaptureError>;   // DeviceInfo { id, name, is_default }
    /// Opens the device and starts capture; frames are delivered to `sink` until the returned
    /// handle is dropped. A device loss is reported through `sink.on_end(DeviceLost)`.
    fn start(&self, device: &DeviceId, sink: Box<dyn FrameSink>) -> Result<CaptureHandle, CaptureError>;
}
pub enum CaptureError { NoDevice, AccessDenied, DeviceBusy, Other(String /* OS error code text, no audio */) }

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

pub trait CredentialStore: Send + Sync {
    /// Reads the transcription API key (NFR-04). None = no key stored.
    fn transcription_api_key(&self) -> Result<Option<SecretString>, CredentialError>;
}

pub trait TempAudioStore: Send + Sync {
    fn put_pending(&self, id: PendingId, audio: &AudioBuffer) -> std::io::Result<()>;
    fn get_pending(&self, id: PendingId) -> std::io::Result<AudioBuffer>;
    fn delete_pending(&self, id: PendingId) -> std::io::Result<()>;
    fn delete_all(&self) -> std::io::Result<()>;           // at start and on exit (FR-032)
}

pub trait SettingsSource: Send + Sync {
    fn dictation_settings(&self) -> DictationSettings;     // 004 owns the persistent implementation
}

pub trait Clock: Send + Sync { fn now(&self) -> Instant; }

pub trait PipelineObserver: Send + Sync {
    fn event(&self, e: &DictationEvent);                   // 006 writes these to the log file
}
```

## Pipeline API (called by the shell)

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
