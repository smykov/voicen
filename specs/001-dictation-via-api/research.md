# Research: Dictation via an OpenAI-compatible API

The stack is fixed by `docs/requirements.md` §9. This file decides only HOW to build within it. Each entry gives the decision, the rationale and the alternatives considered. Items marked "confirm on Windows CI" are behaviours the Linux host cannot prove; the task that builds them carries the check.

## R-1 Global hotkey and hold-mode release

- **Decision**: Use `RegisterHotKey` with `MOD_NOREPEAT` on a hidden top-level window that runs its own message loop on a dedicated shell thread. `WM_HOTKEY` gives the press. In hold mode the thread then polls `GetAsyncKeyState` for every key of the combination every 10 ms; the first key found up counts as the release (spec FR-003). The press timestamp is taken from `WM_HOTKEY` handling and passed into the core.
- **Rationale**: RegisterHotKey is the working assumption in requirements §10 (fewer antivirus heuristics than a low-level hook, and no hook timeouts). The OS consumes the registered key-down, so the trigger key never reaches the focused app (FR-014). `MOD_NOREPEAT` removes auto-repeat presses (spec edge case).
- **Alternatives**: `WH_KEYBOARD_LL` hook (it sees releases directly, but antivirus heuristics and silent hook removal after timeouts were rejected in requirements §10); `tauri-plugin-global-shortcut` (it wraps RegisterHotKey but gives no release event and no control of the window that receives power messages).

## R-2 Hotkey modifiers leaking into the focused app (menu activation)

- **Decision**: In the `WM_HOTKEY` handler, send one key-down/up of an unassigned virtual key (`0xE8`) with `SendInput` before the modifiers are released.
- **Rationale**: When Alt goes down and up with no other key in between that the target saw, the target activates its menu bar. The registered trigger key is consumed, so the target sees only Alt down … Alt up. An intervening unassigned key breaks the "lone Alt" pattern (the same technique as AutoHotkey's menu-mask key). Ctrl and Shift releases have no default effect. Confirm on Windows CI with a test window that counts `WM_SYSKEYUP`/`WM_MENUSELECT`/`WM_CHAR`.
- **Alternatives**: Forbidding Alt in hotkeys (this contradicts the default Ctrl+Alt+Space of req FR-21); swallowing modifier releases with a hook (rejected, see R-1).

## R-3 Audio capture

- **Decision**: Use `cpal` (WASAPI shared mode) in the shell. A stream is built and started on recording start and dropped on stop, so the Windows microphone indicator shows only while recording (FR-007). Samples (f32/i16, any rate and channel count) are pushed to the core, which mixes down to mono and resamples to 16 kHz with `rubato` (MIT) when the recording stops. Device identity is the WASAPI endpoint ID: cpal 0.18.2 exposes it (`DeviceTrait::id()` = `IMMDevice::GetId`; T-012 stores the string without cpal's host prefix and opens by `device_by_id`, a `Specific` device that does not follow the default). Every call of a stream's error callback ends the recording as `DeviceLost` (FR-031), whatever the error kind: in cpal 0.18.2 `run_input` the stream thread of a `Specific` device ends after it (T-012). An access-denied error from building the stream maps to "microphone unavailable: access denied" (FR-009) when cpal's error kind says so. Built in T-006 (checked 2026-10-05 against cpal 0.18.2 sources, the newest on the crates.io index): the stream is opened on a mic-open thread and `AudioSource::start` waits at most `OPEN_BUDGET` = 3 s (OQ-12), because `default_input_config` activates the default device with an unbounded wait and caches the client, so `build_input_stream`'s timeout no longer applies; a stream that opens after the budget is dropped at once. cpal's WASAPI error map never yields `PermissionDenied` (`E_ACCESSDENIED` becomes `BackendError`, the code survives only in OS text), so access denied is not distinguishable from cpal's error kind: a Windows privacy denial most likely shows as "the device could not be opened" until the owner's check (log line `outcome=capture_failed mic=...`) shows otherwise; the Windows behaviour itself is to verify on a real machine. Why: `docs/decisions/windows-shell.md`.
- **Rationale**: cpal is the §9 choice. Resampling once in the core keeps the shell thin and testable on Linux.
- **Alternatives**: Keeping the stream open while idle (lower latency, but it breaks NFR-02's indicator rule); requesting 16 kHz from WASAPI directly (shared mode usually refuses rates other than the mix format).
- **Latency**: building and starting a cpal stream on a wired/USB device is typically well under 200 ms. The Windows CI timing test with the injected source and the owner's USB-mic check prove it (SC-002).

## R-4 Injectable audio source

- **Decision**: The core trait `AudioSource` (contracts/core-traits.md) has two implementations: `CpalSource` in the shell, and a replaying source used by the Windows CI tests and by core tests. Superseded in T-051/T-006: there is no `WavFileSource`, no `VOICEN_TEST_AUDIO` and no `test-audio` feature; the replaying source is `voicen_core::test_support::realtime::RealtimeSource` (feature `test-fakes`), and the Windows tests inject it through `DictationPorts.audio` of `start_dictation`.
- **Rationale**: Requirements §9 calls for an injectable WAV audio source on the Windows runner.

## R-5 Speech gate

- **Decision**: The core trait `SpeechDetector` has `fn contains_speech(&self, audio: &AudioBuffer) -> Result<bool, VadError>`. `SpeechGate` tries the Silero detector and, if the model cannot be loaded, uses the `EnergyDetector` for that and later recordings and logs a warning once per session (FR-016). Silero runs through `whisper-rs`'s whisper.cpp VAD API with the bundled ggml Silero model (< 1 MB, requirements §8). "Speech" means at least 0.3 s of total speech segments (excludes a 1 s cough or key clicks; tuned on the fixtures). Energy fallback (`vad/energy.rs`, constants pinned by T-041): non-overlapping 30 ms frames of 480 samples, a trailing partial frame dropped; each frame's RMS level in dBFS (an all-zero frame is −inf); the noise floor is the 10th percentile (nearest rank) of the frame levels, clamped to [−70, −40] dBFS (−70 for digital zero, −40 for an utterance without pauses — a steady noise above about −28 dBFS then counts as speech, accepted for a fallback); a frame is loud at ≥ floor + 12 dB; only runs of ≥ 3 consecutive loud frames (90 ms) count, so 10–20 ms key clicks never do; speech means ≥ 10 counted frames (300 ms). A once-loaded primary that errors at run time is latched off for the gate's lifetime. The constants are pinned by synthetic fixtures (spec CHK016; `test_support::fixtures`, T-041); real-audio FR-12 is proven by Silero on real clips (T-043).
- **Rationale**: The §9 stack bundles VAD with whisper.cpp, and one native dependency serves 001 and 002. The energy detector is pure Rust and therefore fully testable on Linux.
- **Alternatives**: The `voice_activity_detector`/ONNX Silero crate (it adds onnxruntime, roughly 10–20 MB against NFR-09, and a second native runtime); WebRTC VAD (weaker on noise, a different licence check).

## R-6 Where whisper-rs builds

- **Decision**: `silero.rs` is compiled behind the core cargo feature `silero`, which is shared with feature 002's `whisper` engine (one `whisper-rs` dependency). The local gate (`make check-core`) runs without the feature unless the owner approves adding `cmake` and `clang` to `docker/rust.Dockerfile`. The Windows CI job always builds with it and runs `tests/vad_fixtures.rs`.
- **Rationale**: whisper-rs compiles whisper.cpp from source (it needs cmake and a C/C++ compiler). Adding them to the image is an install that needs the owner's consent (constitution, Constraints).
- **Alternatives**: Always on (it blocks the local gate until the image changes); a separate crate (two seams for one native library).

## R-7 OpenAI-compatible client

- **Decision**: Use `reqwest` 0.13 `default-features = false` with `blocking`, `multipart`, `rustls` and `system-proxy` (no `json`: the body is parsed with `serde_json`; there is no `rustls-tls` feature in 0.13; T-040, decision #44). The blocking `Client` is built per call from the request's `Timeouts` (connect timeout on the client, the whole-request timeout per request). The request is `POST {base_url without trailing '/'}/audio/transcriptions`, multipart with `file` (`audio.wav`, `audio/wav`), `model`, optional `language`, and `response_format=json`. The header is `Authorization: Bearer <key>` only when a key is set; 002's local server may have none. The response is JSON `{"text": "..."}`. Missing or non-string `text` means "unexpected response". The response body is capped at 1 MiB, and a larger body is reported as "unexpected response" (NFR-07). See contracts/openai-transcription.md.
- **Rationale**: A blocking client keeps the core free of an async runtime and lets each job run on its own worker thread (R-10). rustls avoids OpenSSL (licence, and the Windows build). One client serves FR-06 and FR-17 (P-011).
- **Alternatives**: async reqwest on Tauri's tokio runtime (it puts async throughout the core API and its dyn-trait objects); `ureq` (weaker multipart support, and 003 would need a second client).

## R-8 Timeouts

- **Decision**: `voicen_core::timeouts::Timeouts` is the single source (P-010); the values are settings (decision #99) and `Timeouts::from_settings` derives them per job from the job's settings snapshot; defaults: `connect = 5 s`, `api_transcription = 30 s`, `local_server = 60 s` (002), `post_processing = 15 s` (003). The client is built with `.connect_timeout(connect)` and each request with `.timeout(api_transcription)`. reqwest's request timeout covers the whole request including connect, which matches Clarification 1. Tests construct `Timeouts` with milliseconds.
- **Classification**: Connect-phase errors (`is_connect()`): a timeout or refusal gives `CannotReach{host}`; DNS resolution failure (including a lookup with no answer at the deadline, decisions #106, #113; docs/decisions/engine-http.md), or "network unreachable"/"host unreachable" OS errors, give `NetworkUnavailable`. A request timeout (`is_timeout()` and not connect) gives `Timeout`. 401/403 give `InvalidApiKey`, and so does a stored key whose `Bearer <key>` fails HTTP header value validation (checked before any request; contracts/openai-transcription.md). Any other non-2xx gives `ServerError{status}`. A body that is not JSON, has no `text` or is too large gives `UnexpectedResponse`. A 2xx response with empty or whitespace-only `text` is not a failure: it becomes `NoSpeech` (spec edge case). The mapping is a pure function over a small `TransportError` enum, so it is unit-tested without sockets. Wiremock tests cover the real reqwest adapter for refused (a closed localhost port), slow (delay > timeout), statuses and bodies.
- **Alternatives**: Separate read timeouts (reqwest `read_timeout` is per-read; it does not express FR-24).

## R-9 Secrets in the client (P-009)

- **Decision**: The key is held in 004's `Secret` type (decisions #21) with no `Display`/`Debug` output beyond `***`. It is read from `CredentialStore` per request (so a retry picks up a fixed key, Clarification 4) and dropped after use. reqwest's own logging is not enabled, and the app's log subscriber filters `reqwest`/`hyper` to `warn` without request details. Error values carry only `FailureReason` (code, host, status), never the body, the URL query or headers. A test feeds a key and a transcript into every failure path and asserts that neither string appears in any `DictationEvent` or error `Display`.

## R-10 Pipeline concurrency and ordering

- **Decision**: A `Pipeline` owns a `DeliveryQueue` keyed by a monotonically increasing sequence number assigned at recording stop, or at the Retry click for a retry (Clarification 3). Each job runs speech gate → engine → post-process on its own worker thread (std threads; ≤ a few concurrent jobs). Outcomes are posted back. The queue releases an outcome only when every lower sequence number has been released. A no-speech outcome is released as its notice (no clipboard change). A failure is released as its notification plus the pending update. Delivery itself (clipboard + paste) runs on one delivery thread, so pastes never interleave. Head-of-line waiting is bounded by the 30 s timeout (+15 s for 003).
- **Rationale**: This puts FR-029 in one place. Assigning the sequence at stop closes the CHK024 overtake hole.
- **Alternatives**: Delivering out of order with a reorder buffer in the shell (logic would leave the core); a single-job FIFO (it breaks FR-029's "start a new recording while processing" — that would still work, but stage parallelism would be lost and p90 would suffer).

## R-11 Clipboard with history exclusion

- **Decision**: Use Win32 `OpenClipboard` (retry up to 10 × 20 ms if another process holds it), `EmptyClipboard`, then `SetClipboardData(CF_UNICODETEXT)`. Also set the registered formats `ExcludeClipboardContentFromMonitorProcessing` (empty), `CanIncludeInClipboardHistory` (DWORD 0) and `CanUploadToCloudClipboard` (DWORD 0). If the clipboard still cannot be opened, the outcome is the failure "clipboard unavailable": nothing is pasted, and the recording's audio becomes the pending recording like any other failure (spec edge case), so Retry re-runs it.
- **Rationale**: These are the documented formats that keep content out of Win+V history and cloud sync (req FR-10). `arboard` cannot set them.
- **Alternatives**: `arboard` + a separate format write (two opens, a race).

## R-12 Paste into the start window

- **Decision**: At recording start the shell records `GetForegroundWindow()` → the root owner (`GetAncestor(GA_ROOTOWNER)`), its process ID and its integrity level. At delivery:
  1. Wait until `GetAsyncKeyState` reports Shift/Ctrl/Alt/Win up, polling 10 ms, for at most 1 s. If they are still held, deliver copy-only with "copied — paste manually" (Clarification 2).
  2. Check that the current foreground root window equals the start window. If not (including closed), deliver copy-only.
  3. Check that the target's integrity level is ≤ ours. Otherwise the target is elevated, UIPI would drop the input silently, so deliver copy-only. If the target process cannot be queried, treat it as elevated.
  4. `SendInput` Ctrl down, V down/up, Ctrl up as one batch.
- **Rationale**: The integrity comparison is the precise UIPI condition behind req FR-10's "runs elevated". Sending all four events in one batch prevents interleaving with user input.
- **Alternatives**: `TokenElevation` only (it misses UIAccess and integrity-level cases); `WM_PASTE` (many apps ignore it — the NFR-08 list includes Chromium and Electron apps).

## R-13 Overlay window that never takes focus

- **Decision**: The shell creates the overlay `WebviewWindow` on demand: label `overlay`, route `/overlay`, `decorations(false)`, `transparent(true)`, `always_on_top(true)`, `skip_taskbar(true)`, `focused(false)`, `resizable(false)`, `shadow(false)`. It is created hidden. Before it is shown, the HWND gets `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT` and is shown with `ShowWindow(SW_SHOWNOACTIVATE)`. `set_ignore_cursor_events(true)` makes it click-through. Position: bottom-centre of the work area of the monitor that holds the start window, 48 px above the taskbar (spec CHK010 left open; this default can change without affecting the requirements). The window is destroyed when the indicator state becomes hidden (NFR-03). The overlay is driven by one event (contracts/ipc.md). The webview's creation latency (~100–300 ms) does not delay capture, because capture starts first.
- **Built by** (T-057, `src-tauri/src/overlay.rs`; why: `docs/decisions/overlay.md`): the one "overlay" thread builds the window already visible, with `focused(false)` and `focusable(false)` (so tao creates it with `WS_EX_NOACTIVATE` and shows it with `SW_SHOWNOACTIVATE`), `always_on_top`, `skip_taskbar`, `decorations(false)`, `transparent`, `shadow(false)`, `resizable(false)` and a position read from raw Win32 (monitor of the window in front). Nothing is called on it while it is visible (tao re-shows with `SW_SHOW` on any setter). So the hidden-build, `WS_EX_TOOLWINDOW` / `WS_EX_TRANSPARENT` and `set_ignore_cursor_events` steps in the Decision above are not built (OQ-10). A new window is built only after the old one's `Destroyed`. The payload and the page (T-053) are in contracts/ipc.md.
- **Confirm on Windows CI**: after the overlay is shown, `GetForegroundWindow()` still equals the test window, and the test window receives no `WM_KILLFOCUS`.
- **Alternatives**: A native layered window drawn with GDI (no webview cost, but a second UI technology and no Playwright check); keeping the overlay alive while hidden (it breaks NFR-03).

## R-14 Messages and languages

- **Decision**: The core exposes `MessageKey` (an enum) plus parameters, and never formats text. Texts live in one catalog per language. Location (agreed with 004): `i18n/en.json`, `i18n/ru.json`, consumed by the UI through an import and by the shell (toasts, tray) through `include_str!` + serde. The language is resolved in one place (004 owns that resolver; until it lands, the shell uses `en`). The texts this feature adds are listed in contracts/messages.md. Completeness is checked by T-005's parity tests and the `messages!` macro (every id exists in both catalogs), not by a feature-owned test.
- **Rationale**: P-010 (one source of message text). Toasts are produced in Rust, so a UI-only catalog would not do.
- **Open**: The location must agree with 004's i18n design (an owner/coordination item in plan.md).

## R-15 Crash isolation (NFR-07)

- **Decision**: The release profile has `panic = "abort"`, so a panic cannot be "caught". Engine and parsing code therefore return `Result` everywhere. The response parsing never indexes or unwraps on server data. The body size is capped. Clippy lints `unwrap_used`, `expect_used`, `indexing_slicing` are denied in `engine/`, `failure.rs` and `vad/` (module-level `#![deny]`). Fault-injection tests (wiremock: truncated body, wrong JSON types, 10 MB body, connection reset mid-body, invalid UTF-8) assert an `Err(FailureReason)` and no panic. whisper.cpp native faults are outside Rust's control and are covered by 006's crash files.
- **Alternatives**: `panic = "unwind"` + `catch_unwind` around engines (a larger binary; it hides bugs instead of preventing them; changing the profile is a cross-feature decision).

## R-16 Toasts with Retry

- **Decision**: WinRT `ToastNotificationManager::CreateToastNotifierWithId(AUMID)` through the `windows` crate. The AUMID is the app identifier `dev.voicen.app`, which the installer's Start-menu shortcut carries (006 owns that; requirements §8). The toast XML has an `<action content="Retry" arguments="retry:<pending-id>"/>` on failure toasts. The `Activated` event handler, alive while the app runs, routes `retry:<id>` to `Pipeline::retry`. A stale id (already retried or replaced) does nothing (spec edge case). A failure to show the toast (no AUMID in dev builds, Focus Assist) is ignored, because the overlay and tray carry the message (FR-028).
- **Alternatives**: `tauri-plugin-notification` (no action buttons on desktop); `tauri-winrt-notification` (acceptable if its pinned version supports buttons with an activation callback — check during the task; the `windows` crate is already a dependency).

## R-17 Tray, sleep/lock, Explorer restart, single instance

- **Tray**: Tauri 2 `TrayIconBuilder` with icons for idle, recording, error and hotkey-error, and a menu (spec FR-001; the Retry item is visible only while a pending recording exists; the History item is hidden until feature 005 provides the window, which resolves spec CHK020). Opening the menu (right click) clears the transient error state (FR-028), but not the hotkey error.
- **Explorer restart**: The shell's hidden window registers `RegisterWindowMessageW("TaskbarCreated")`. On receipt it re-adds the tray icon (by rebuilding it, if Tauri's tray does not re-add it itself — confirm on Windows CI by broadcasting the message).
- **Sleep/lock**: The same hidden top-level window (message-only windows get no broadcasts) handles `WM_POWERBROADCAST` (`PBT_APMSUSPEND`: stop any recording and process what was captured, spec edge case; `PBT_APMRESUMEAUTOMATIC`: unregister + re-register the hotkey) and `WM_WTSSESSION_CHANGE` (`WTSRegisterSessionNotification`; on `WTS_SESSION_UNLOCK`, re-register). A failed re-registration enters the hotkey-error state (FR-011/FR-013).
- **Single instance**: `tauri-plugin-single-instance`; its callback opens or focuses the settings window (004 provides the window; until then, a stub route).
- **Dependencies to approve (owner)**: `reqwest` (MIT/Apache), `rubato` (MIT), `thiserror` (MIT/Apache), `serde_json` (MIT/Apache), `whisper-rs` (Unlicense — public-domain-equivalent, compatible with MIT; shared with 002), `cpal` (Apache-2.0), `windows` (MIT/Apache), `tauri-plugin-single-instance` (MIT/Apache), and for development `wiremock` + `tokio` (MIT). All are compatible with NFR-12. 006 adds them to the licence list.

## R-18 Pending recording and temporary audio

- **Decision**: In-flight audio stays in memory (≤ 19.2 MB per job). Only the pending recording goes to disk, as `<data dir>/tmp/audio/pending-<id>.wav`, written by the `TempAudioStore` trait (a real file system in the shell; an in-memory fake in tests). It is deleted after a successful retry, replaced by a newer failed one, and deleted on exit. At start the shell deletes everything under `<data dir>/tmp/audio/` before the hotkey is registered (FR-032). `<data dir>` is resolved by the shell's one data-dir function (architecture: cross-cutting values).
- **Rationale**: Idle RAM (NFR-03) must not carry a 19 MB pending recording. Disk use is bounded to one file.
- **Alternatives**: Pending in memory (simpler, but it costs idle RAM); all audio on disk (more writes of private audio for no gain).

## R-19 Settings before feature 004

- **Decision**: The core trait `SettingsSource` returns a `DictationSettings` snapshot at each recording start and at each retry (P-013: settings are read through the real loader). Until 004 lands, the shell implementation reads `<data dir>/settings.json` (serde, unknown fields ignored) with the defaults of req FR-21. 004 replaces the implementation and keeps the trait.
