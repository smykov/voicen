# Data Model: Dictation via an OpenAI-compatible API

All entities live in `voicen-core` and are in-memory unless stated otherwise. Names are the planned Rust type names. Trait signatures are in [contracts/core-traits.md](contracts/core-traits.md).

## DictationSettings (read model; owned by 004)

Built as the `Arc<Settings>` snapshot of 004's `SettingsService::snapshot()`, taken at press and carried in the recording's `PressContext` (decision #47 (1), T-001); a retry takes a new snapshot at the click (T-007). There is no `DictationSettings` type and no `SettingsSource`: the table below names the `Settings` fields a job reads (Clarification 4).

| Field | Type | Rule |
|---|---|---|
| engine | `EngineKind` (`None`, `Api`, later `BuiltIn`, `LocalServer` from 002) | `None` → the hotkey does not record; "choose a transcription engine" (req FR-21) |
| api_base_url | `Url` | path joined as `<base>/audio/transcriptions`; a trailing `/` is ignored |
| api_model | `String` | sent as `model` |
| language | `Language` (`Auto` or ISO-639-1 code) | `Auto` → no `language` field |
| microphone | `Option<DeviceId>` | `None` → Windows default device |
| hotkey | `HotkeyBinding` | see below |
| auto_paste | `bool` | default `true` |

The API key is not part of the snapshot. It is read from `CredentialStore` per request (R-9).

## HotkeyBinding / HotkeyRegistration

| Field | Type | Rule |
|---|---|---|
| modifiers | set of `Ctrl`, `Alt`, `Shift`, `Win` | at least one modifier, or a function key |
| key | virtual-key code | default `Space` with `Ctrl+Alt` (req FR-21) |
| mode | `Hold` \| `Toggle` | default `Hold` |

`HotkeyRegistration` state: `Active(binding)` \| `Failed{binding, reason}`.

- start: register → `Active` or `Failed` (tray `HotkeyError`, notification, open settings on the hotkey field — FR-011)
- change: register the new binding → success: `Active(new)`, unregister the old one; failure: stay `Active(old)`, report "hotkey unavailable" (FR-012)
- resume/unlock: re-register → failure → `Failed` (FR-013)
- `Failed` → `Active` only after a successful registration; the hotkey error clears only then (FR-028)
- Each result reaches core as `DictationSession::hotkey_registration(registered, at)` → `RecordingController::hotkey_registration` (tray `HotkeyError`; T-051)

## Recording (RecordingController state machine)

| Field | Type |
|---|---|
| id | `RecordingId` (monotonic) |
| start_window | `StartWindow` |
| started_at | `Instant` (from the caller's event: the hotkey thread stamps the press; no clock port, T-042) |
| device | `DeviceId` used (selected or fallback) |
| samples | `AudioBuffer` (accumulating; finalised to 16 kHz mono i16) |
| end | `RecordingEnd` = `Released` \| `Toggled` \| `MaxLength` \| `DeviceLost` \| `Suspended` \| `Cancelled` \| `TooShort` |

States and transitions (one recording at a time):

```text
Idle --press [engine ≠ None, device opens]--> Recording
Idle --press [engine = None]--> Idle + notice "choose a transcription engine" on the overlay for 3 s, then open settings on the Engine tab
Idle --press [device fails]--> Idle + failure "microphone unavailable: <reason>"   (no recording state)
Recording --press (hold: auto-repeat)--> Recording            (ignored)
Recording --release (hold) [held < 0.3 s]--> Idle (TooShort, silent discard)
Recording --release (hold) / press (toggle)--> Idle → job (Released / Toggled)
Recording --10 min--> Idle → job (MaxLength) + notice "maximum length reached"
Recording --device lost / suspend--> Idle → job (DeviceLost / Suspended)
Recording --Esc--> Idle (Cancelled; nothing sent; a later hold release does nothing)
```

Validation:

- min hold 0.3 s (hold mode only)
- max 10 min (both modes)
- Esc is registered on entering `Recording` and unregistered on leaving it. If Esc registration fails, the recording continues and a warning event is emitted.

## DictationSession (T-051)

The one runtime owner of the transitions above (`voicen_core::dictation::DictationSession`; contract in [contracts/core-traits.md](contracts/core-traits.md) "Dictation session"). It holds, behind one lock:

| Part | Rule |
|---|---|
| `RecordingController<PressContext>` | every decision; the only source of `IndicatorState` and of "is a recording on" (`live_id`) |
| live capture | the open `CaptureHandle` of the live recording, its press instant and its frame sink (mono samples, the first frame's format and instant); taken out at release, stopped before the release returns |
| FIFO queue | `FinishedRecording`s pushed in the critical section of their `finish`, so queue order = finish order = recording order |
| `retry_available` | `Pipeline::pending().is_some()`, read by the worker right after each `run_job` |
| last published | the `(tray, retry_available)` and overlay last given to the `Indicator`; a value is sent only when it differs |

One worker thread owns the `Pipeline` (the only caller of `run_job` and `job_finished`); one timer thread calls `tick` at `next_deadline`. The snapshot and the start window are taken once, at the press that starts the recording. A press whose capture cannot open publishes no recording state. Between a stop's `release` and its `finish` nothing is published, so the overlay goes `Recording` → `Processing` without `Hidden` in between. Dropping the session lets the job in flight finish, drops queued recordings and joins both threads.

## StartWindow

| Field | Type | Rule |
|---|---|---|
| handle | opaque `WindowRef` (HWND in the shell) | root owner of the foreground window at start |
| process_id | `u32` | |
| elevated | `bool` | target integrity level > ours, or unknown |

## DictationJob

| Field | Type |
|---|---|
| seq | `u64` — delivery order, assigned at recording stop or at the Retry click |
| recording | `Recording` (audio + start window) |
| origin | `Fresh` \| `Retry{pending_id}` |
| stage | `Gating` → `Transcribing` → `PostProcessing` → `Ready` |
| outcome | `JobOutcome` |

`JobOutcome`:

- `Text(String)` — final text after post-processing, not blank
- `NoSpeech` — the gate found no speech, the engine returned empty text, or the post-processed text is blank
- `Failed(FailureReason)`

Built (T-001): `Pipeline::run_job(FinishedRecording<PressContext>) -> JobReport { end: JobEnd, pending: Option<PendingId> }`; the job view is private, `seq` is taken at `run_job` entry until T-011 moves it to the stop, and `origin: Retry` comes with T-007.

## DeliveryQueue

- An ordered map `seq → slot`. A slot is `Waiting` or `Done(JobOutcome)`.
- Invariant: an outcome with seq *n* is released only after every seq < *n* is released (FR-029).
- `NoSpeech` and `Failed` release in order like `Text`. A failure never blocks later ones beyond its own completion, which the timeouts bound.
- A retry gets the next seq at the Retry click (Clarification 3).

## DeliveryDecision (for a released `Text`)

Inputs: `auto_paste`, modifiers released within 1 s, foreground == start window, `start_window.elevated`, the `send_ctrl_v` result.

| Condition (first match) | Result | Message |
|---|---|---|
| clipboard write fails | `Failed(ClipboardUnavailable)` → pending, no paster call | `failure.clipboard_unavailable` |
| auto_paste = off | `CopiedOnly`, no paster call | `notice.copied` |
| no start window, or start window elevated | `CopyManual`, no wait | `notice.copied_paste_manually` |
| modifiers still held after 1 s | `CopyManual` | `notice.copied_paste_manually` |
| start window not in front / closed | `CopyManual` | `notice.copied_paste_manually` |
| `send_ctrl_v` returns an error (decision #47 (3)) | `CopyManual` (the text is in the clipboard) | `notice.copied_paste_manually` |
| otherwise | `Pasted` | (none) |

The clipboard is always written first, with the history-exclusion formats (FR-022), and only for a non-blank text. The static conditions (no window, elevated) are checked before the 1 s wait, and the front check right before Ctrl+V; every `CopyManual` row has the same result, so the order changes only what waits. Built as `voicen_core::delivery::deliver` (T-001), `MODIFIER_WAIT` = 1 s.

## PendingRecording

| Field | Type | Rule |
|---|---|---|
| id | `PendingId` | used in the toast Retry argument |
| audio | `TempAudioStore` handle (`<data>/tmp/audio/pending-<id>.wav`) | written on failure |
| start_window | `StartWindow` | the retry delivers here |
| reason | `FailureReason` | last reason |

Rules (NFR-06, FR-026, FR-027):

- At most one exists. A newer `Failed` replaces it, and the old file is deleted. `Text`/`NoSpeech` outcomes of other jobs do not touch it.
- If storing the newer recording's audio fails, there is no pending recording and the older one is still deleted (decision #47 (4)): the tray never offers a dictation older than the last failure reported.
- Built in T-001: one slot in `Pipeline` (id and reason; the start window comes with T-007's retry), `Pipeline::pending()`; the audio goes through `TempAudioStore::put_pending`/`delete_pending`.
- Retry: if no pending recording exists or the id is stale, nothing happens. Otherwise a `DictationJob{origin: Retry}` is created with the current settings.
  - On `Text`: deliver, then delete the pending recording.
  - On `Failed`: report again, and update the reason (same audio).
  - On `NoSpeech` (the engine now returns empty text): delete the pending recording and show "no speech detected".
- Deleted on exit; at start, all of `<data>/tmp/audio/` is deleted (FR-032).

## FailureReason

| Code | Message key | Source |
|---|---|---|
| `InvalidApiKey` | `failure.invalid_api_key` | HTTP 401/403; or the stored key cannot be sent: `Bearer <key>` fails HTTP header value validation (a control byte other than tab, or DEL), checked before the request is built, so nothing is sent. A non-ASCII key passes that rule and is sent as UTF-8; the server's 401/403 decides |
| `NetworkUnavailable` | `failure.network_unavailable` | DNS failure, network/host unreachable |
| `CannotReach{host}` | `failure.cannot_reach` | connection refused, connect timeout, TLS handshake failure, HTTP client setup; `host` = the base URL's `host[:port]` (port only when not the scheme default) |
| `Timeout` | `failure.timeout` | the whole request (connect to last body byte) exceeded its `Timeouts` duration (API 30 s, local server 60 s), including a stall mid-body |
| `ServerError{status}` | `failure.server_error` | any other non-2xx (413, 429, 5xx, …) |
| `UnexpectedResponse` | `failure.unexpected_response` | 2xx body unparsable / not an object / missing or non-string `text` / > 1 MiB / invalid UTF-8 / reset or closed mid-body |
| `KeyStoreUnavailable` | `failure.key_store_unavailable` | `CredentialStore::read` returned an error; `engine_for` returns it before any engine exists, so no request is sent (decision #44) |
| `EngineNotConfigured` | `failure.engine_not_configured` | `engine_for` cannot build an engine from the settings: `BuiltinLocal` (built by the shell, T-017), `None`, or a stored base URL that fails `check_base_url`; no key is read (decision #44) |
| `ClipboardUnavailable` | `failure.clipboard_unavailable` | clipboard open failed after retries |
| `MicrophoneUnavailable{cause}` | `failure.microphone_unavailable` | no device, access denied, open error — not retryable, no pending |
| `HotkeyUnavailable` | `failure.hotkey_unavailable` | registration failed — not retryable |

Retryable (creates a pending recording): every code from `InvalidApiKey` through `ClipboardUnavailable`.

In code: `voicen_core::failure::FailureReason` with `code()` (the Code column without fields), `message_id()` and `message_params()` (`host`, `status`; `reason` for `MicrophoneUnavailable`, the cause's `mic_reason.*` id, which the Rust `i18n::text` renders in the message's language). Every transport failure is mapped by the one `failure::classify` (order in [contracts/openai-transcription.md](contracts/openai-transcription.md)); a transport reason is built only from the status, the classification flags and `host[:port]`, never from a `reqwest::Error`, a URL, a body or a key. `MicrophoneUnavailable{cause}` is built only from the closed `recording::MicCause` set (`NoDevice`, `AccessDenied`, `Busy`, `Other`), never from the OS error text of a `CaptureError`. T-040 has the first eight variants; `ClipboardUnavailable` and `retryable()` (an exhaustive match) since T-001, `MicrophoneUnavailable` since T-042, `HotkeyUnavailable` with T-006.

## IndicatorState

- `TrayState`: `Idle` \| `Recording` \| `Error` \| `HotkeyError`.
  - Priority: `HotkeyError` > `Recording` > `Error` > `Idle`.
  - `Error` is cleared by the next successful delivery (any `JobEnd::Delivered`: `Pasted`, `CopiedOnly` or `CopyManual`; FR-25 "until the next successful dictation") or by opening the tray menu.
  - `HotkeyError` is set by a failed registration and cleared only by a successful one (`RecordingController::hotkey_registration`, T-051).
- `OverlayState`: `Hidden` \| `Recording` \| `Processing` \| `Message{id, params, until}`.
  - `Recording` while a recording is on.
  - `Processing` while ≥ 1 job is not yet released and no recording is on (Clarification 5).
  - `Message` for 3 s after any failure or notice (a job's, a capture failure, or a notice outside a job such as `notice.choose_engine`). A new recording pre-empts the message display, and the message is not re-shown. A message raised during a live recording (for example the previous recording's capture failing at its stop, FR-029) is not shown while the recording is on; after its release it is shown for the rest of its 3 s, and not at all if they have passed.
  - `Hidden` otherwise; the overlay window is destroyed.

## MicrophoneChoice

Inputs: the selected `DeviceId`, and the device list at recording start.

| State | Meaning |
|---|---|
| `Selected` | the selected device is present (or none selected → default) |
| `Fallback(default_id)` | the selected device is missing; the default is used |

- A notification "using <device name>" is shown when the state changes into `Fallback(x)`, or from `Fallback(x)` to `Fallback(y)`. It is not shown again while the state is unchanged (FR-030).
- `Fallback` → `Selected` when the selected device returns (no notice).
- If there is no device at all → `MicrophoneUnavailable{NoDevice}`.

## DictationEvent (log allowlist; FR-033)

| Event | Fields (only these) |
|---|---|
| `RecordingStarted` | seq-less recording id, `hotkey_to_first_frame_ms`, device kind (`selected`/`fallback`) |
| `RecordingEnded` | recording id, `duration_ms`, `end` |
| `SpeechGate` | recording id, `detector` (`silero`/`energy`), `speech: bool` |
| `JobFinished` | seq, recording id (T-008: the log joins the job to its recording by it), `engine` (kind name), `stop_to_text_ms`, outcome code (`text`/`no_speech`/`failed`), failure code, HTTP status |
| `Delivered` | seq, `text_to_paste_ms`, result (`pasted`/`copied_only`/`copy_manual`) |
| `Warning` | code (`vad_fallback`, `esc_unavailable`, `toast_failed`) |
| `CaptureFailed` | recording id, `cause` (`no_device`/`access_denied`/`busy`/`other`; the closed `MicCause`, never OS text) — the capture could not open at press, or failed at stop |

No event type has a field that can hold transcript text, audio, a URL query, a header or a key. This is enforced by the types, plus a redaction test.

As built (`voicen_core::events`, T-001): `DictationEvent` is `Copy` (so no `String`, `Vec`, `Secret` or `FailureReason` field can exist); fields are integers, bools, `&'static str` codes and small enums (`RecordingId`, `DeviceKind`, `RecordingEnd`, `OutcomeCode`, `DeliveryResult`, `WarningCode`). `JobFinished { seq, recording: RecordingId /* T-008 */, engine: Option<&'static str> /* None: no engine built */, stop_to_text_ms, outcome, failure: Option<&'static str> /* FailureReason::code() */, http_status: Option<u16> /* ServerError only; 401/403 are not carried */ }`, emitted after the clipboard write (a clipboard failure is logged as `failed`/`ClipboardUnavailable` with no `Delivered`). `Delivered { seq, text_to_paste_ms, result }`. Per job the order is `[Warning]`, `SpeechGate`, `JobFinished`, `[Delivered]`, all from `Pipeline::run_job` on one `PipelineObserver`. The dictation session (T-051) emits on the same observer (it takes it from `PipelineDeps`): at the release, `RecordingStarted` (only if a frame arrived; first frame instant − press instant) and `RecordingEnded` (`duration_ms` = the hold, `end`), both before the recording's job is queued; `CaptureFailed { recording, cause }` for a capture that failed at press (then no `RecordingStarted`/`RecordingEnded`: no recording state) or at stop. `CaptureFailed` keeps the event `Copy`.
