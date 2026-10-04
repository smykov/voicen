# Data Model: Dictation via an OpenAI-compatible API

All entities live in `voicen-core` and are in-memory unless stated otherwise. Names are the planned Rust type names. Trait signatures are in [contracts/core-traits.md](contracts/core-traits.md).

## DictationSettings (read model; owned by 004)

A snapshot read from `SettingsSource` at each recording start and each retry (Clarification 4).

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
Idle --press [engine = None]--> Idle + notice "choose a transcription engine" + open settings
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

- `Text(String)` — final text after post-processing
- `NoSpeech` — the gate found no speech, or the engine returned empty text
- `Failed(FailureReason)`

## DeliveryQueue

- An ordered map `seq → slot`. A slot is `Waiting` or `Done(JobOutcome)`.
- Invariant: an outcome with seq *n* is released only after every seq < *n* is released (FR-029).
- `NoSpeech` and `Failed` release in order like `Text`. A failure never blocks later ones beyond its own completion, which the timeouts bound.
- A retry gets the next seq at the Retry click (Clarification 3).

## DeliveryDecision (for a released `Text`)

Inputs: `auto_paste`, modifiers released within 1 s, foreground == start window, `start_window.elevated`.

| Condition (first match) | Result | Message |
|---|---|---|
| clipboard write fails | `Failed(ClipboardUnavailable)` → pending | "clipboard unavailable" |
| auto_paste = off | `CopiedOnly` | "copied to clipboard" |
| modifiers still held after 1 s | `CopiedOnly` | "copied — paste manually" |
| start window not in front / closed | `CopiedOnly` | "copied — paste manually" |
| start window elevated | `CopiedOnly` | "copied — paste manually" |
| otherwise | `Pasted` | (none) |

The clipboard is always written first, with the history-exclusion formats (FR-022).

## PendingRecording

| Field | Type | Rule |
|---|---|---|
| id | `PendingId` | used in the toast Retry argument |
| audio | `TempAudioStore` handle (`<data>/tmp/audio/pending-<id>.wav`) | written on failure |
| start_window | `StartWindow` | the retry delivers here |
| reason | `FailureReason` | last reason |

Rules (NFR-06, FR-026, FR-027):

- At most one exists. A newer `Failed` replaces it, and the old file is deleted. `Text`/`NoSpeech` outcomes of other jobs do not touch it.
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
| `Timeout` | `failure.timeout` | the whole request (connect to last body byte) exceeded its `Timeouts` duration (API 30 s), including a stall mid-body |
| `ServerError{status}` | `failure.server_error` | any other non-2xx (413, 429, 5xx, …) |
| `UnexpectedResponse` | `failure.unexpected_response` | 2xx body unparsable / not an object / missing or non-string `text` / > 1 MiB / invalid UTF-8 / reset or closed mid-body |
| `KeyStoreUnavailable` | `failure.key_store_unavailable` | `CredentialStore::read` returned an error; `engine_for` returns it before any engine exists, so no request is sent (decision #44) |
| `EngineNotConfigured` | `failure.engine_not_configured` | `engine_for` cannot build an engine from the settings: `BuiltinLocal` (built by the shell, T-017), `LocalServer` (until T-018), `None`, or a stored base URL that fails `check_base_url`; no key is read (decision #44) |
| `ClipboardUnavailable` | `failure.clipboard_unavailable` | clipboard open failed after retries |
| `MicrophoneUnavailable{cause}` | `failure.microphone_unavailable` | no device, access denied, open error — not retryable, no pending |
| `HotkeyUnavailable` | `failure.hotkey_unavailable` | registration failed — not retryable |

Retryable (creates a pending recording): every code from `InvalidApiKey` through `ClipboardUnavailable`.

In code: `voicen_core::failure::FailureReason` with `code()` (the Code column without fields), `message_id()` and `message_params()` (`host`, `status`). Every transport failure is mapped by the one `failure::classify` (order in [contracts/openai-transcription.md](contracts/openai-transcription.md)); a reason is built only from the status, the classification flags and `host[:port]`, never from a `reqwest::Error`, a URL, a body or a key. T-040 has the first eight variants; `ClipboardUnavailable` comes with T-001, `MicrophoneUnavailable`/`HotkeyUnavailable` with T-042/T-006.

## IndicatorState

- `TrayState`: `Idle` \| `Recording` \| `Error` \| `HotkeyError`.
  - Priority: `HotkeyError` > `Recording` > `Error` > `Idle`.
  - `Error` is cleared by the next successful delivery (`Pasted` or `CopiedOnly`) or by opening the tray menu.
  - `HotkeyError` is cleared only by a successful registration.
- `OverlayState`: `Hidden` \| `Recording` \| `Processing` \| `Message{key, params, until}`.
  - `Recording` while a recording is on.
  - `Processing` while ≥ 1 job is not yet released and no recording is on (Clarification 5).
  - `Message` for 3 s after any failure or notice. A new recording pre-empts the message display, and the message is not re-shown.
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
| `JobFinished` | seq, `engine` (kind name), `stop_to_text_ms`, outcome code (`text`/`no_speech`/`failed`), failure code, HTTP status |
| `Delivered` | seq, `text_to_paste_ms`, result (`pasted`/`copied_only`/`copy_manual`) |
| `Warning` | code (`vad_fallback`, `esc_unavailable`, `toast_failed`) |

No event type has a field that can hold transcript text, audio, a URL query, a header or a key. This is enforced by the types, plus a redaction test.
