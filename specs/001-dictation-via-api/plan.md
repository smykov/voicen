# Implementation Plan: Dictation via an OpenAI-compatible API

**Branch**: `001-dictation-via-api` | **Date**: 2026-10-02 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/001-dictation-via-api/spec.md`

## Summary

The tray app turns a global hotkey into a recording. The recording passes a speech-detection gate, then goes to an OpenAI-compatible `/audio/transcriptions` endpoint, through a pass-through post-processing stage, and is delivered to the clipboard and by Ctrl+V into the window that was focused at recording start. Failures paste nothing, report through a toast, the overlay and the tray state, and keep the audio for a retry. Results are delivered in recording order.

Technical approach (see [research.md](research.md)):

- **Core (`crates/voicen-core`).** All decisions are platform-independent code in the core: the recording state machine (hold/toggle, 0.3 s minimum, 10 min maximum, Esc), the speech gate, the engine trait and the one OpenAI-compatible client, failure classification, timeouts, the ordered delivery queue, the pending recording, the delivery decision, message keys and log events. The core is synchronous. Every Windows service sits behind a trait in `voicen_core::platform`, and the Linux tests use fakes.
- **Shell (`src-tauri`).** It implements those traits with the `windows` crate and `cpal`: RegisterHotKey on a hidden top-level window that also receives power and session messages, WASAPI capture, a clipboard write with history-exclusion formats, SendInput paste with foreground and integrity checks, the tray, WinRT toasts with a Retry action, Credential Manager reads, and a single instance.
- **UI (`src/`).** It adds only the display-only overlay route, which is driven by one IPC event.

## Technical Context

**Language/Version**: Rust 1.99 (edition 2021) for `voicen-core` and `src-tauri`; TypeScript + Svelte 5 (SvelteKit static) for the overlay

**Primary Dependencies**:
- Core: `reqwest` (blocking, multipart, rustls), `serde`/`serde_json`, `thiserror`, `rubato` (resampling), `whisper-rs` (Silero VAD, behind a cargo feature shared with 002).
- Shell: `tauri` 2, `tauri-plugin-single-instance`, `cpal`, `windows` (Win32 + WinRT toasts).
- Dev: `wiremock` + `tokio` (mock OpenAI-compatible server).

**Storage**: No persistent store of its own. Settings are read as a projection of 004's `SettingsService::snapshot()`. The API key is read through 004's `CredentialStore` (`KeySlot::TranscriptionApi`); its one Windows implementation is 004 T016. The pending recording is one temporary WAV under `<data dir>/tmp/audio/`; in-flight audio stays in memory.

**Testing**:
- `cargo test -p voicen-core` in the `voicen-rust:1.99` image: fakes for every platform trait, plus wiremock for HTTP.
- `cargo test --workspace` on the Windows CI runner: Win32 integration tests against a test window, and an injected WAV audio source.
- vitest + Playwright with mocked IPC for the overlay.
- The owner's manual checklist for the real microphone, hotkey, toasts, sleep/lock and NFR-08 targets.

**Target Platform**: Windows 10 22H2 / Windows 11 x64 (Windows 11 checked per release); development host Linux

**Project Type**: desktop app (Tauri 2: Rust core library + Windows shell + web UI)

**Performance Goals**: stop → paste p90 ≤ 3 s via OpenAI `whisper-1` and Groq `whisper-large-v3-turbo` (NFR-01); hotkey → capture start p95 ≤ 200 ms (NFR-02)

**Constraints**:
- Idle RAM ≤ 150 MB with the overlay destroyed when hidden (NFR-03).
- No transcript, audio or key in logs (NFR-04, P-009).
- Audio kept only until delivered (NFR-06).
- The engine never crashes the app (NFR-07); release profile `panic = "abort"`, so engine code MUST return errors, not panic (research R-15).
- MIT-compatible licences (NFR-12).

**Scale/Scope**: a single user, one dictation at a time plus overlapping jobs (realistically ≤ 3 in flight), recordings ≤ 10 min (≈ 19.2 MB WAV)

No NEEDS CLARIFICATION remains: research.md resolves every unknown below.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | How this plan complies | Status |
|---|---|---|
| I. PRINCIPLES.md | P-004/P-005: every FR failure branch has a red test listed in tasks.md. P-009: log events are typed structs with no text, audio or key fields; the client masks the key and never logs response bodies. P-010: timeouts, the data dir and message keys each have one resolver. P-011: one OpenAI-compatible client (reused by 002 local server and the 003 chat client's transport), one delivery path for dictation and retry, and one failure → notification path. P-013: settings are read through the real loader, with a test. | PASS |
| II. Privacy by default | Audio goes only to the configured endpoint. The pending recording is one temp file, deleted on success, on exit and at the next start. No telemetry. The key is read only from Credential Manager. | PASS |
| III. Never lose words, never paste garbage | Speech gate before any request. No paste on any failure. Clipboard-only fallback (start window gone, elevated, modifiers held). Pending audio for retry. Every Must failure branch is in the spec and has a test (tasks.md). | PASS |
| IV. Core + thin shell | The state machine, gate, engine, ordering, retry, delivery decision and timeouts are in `voicen-core` behind `platform` traits. The shell only adapts Win32/WinRT/cpal. The `Engine` trait keeps recording and delivery code unchanged for 002 (NFR-11). | PASS |
| V. Measured, not assumed | NFR-01/NFR-02 come from the log-timing events (FR-033). The verification table below says, per requirement, where it is proven. | PASS |

Post-design re-check (after data-model.md and contracts/): still PASS. There is no complexity-tracking entry. Two items need the owner, but neither breaks a gate (see "Owner decisions needed").

## Where each requirement is verified

| Spec FR (req) | Linux host — core with fakes | Linux host — UI, mocked IPC | Windows CI runner | Owner's manual check |
|---|---|---|---|---|
| FR-001, FR-002 (FR-01) | — | — | Smoke: launch, no foreground window taken; a second launch exits and signals the first | Tray menu entries |
| FR-003–FR-006 (FR-02, FR-03) | Recording state machine: hold, 0.3 s, toggle, 10 min, auto-repeat | — | Hotkey thread with synthetic SendInput key events into a test window | Real hold/toggle |
| FR-007 (NFR-02) | Timing event emitted | — | Injected-source capture start p95 ≤ 200 ms | Mic indicator only while recording; USB mic timing |
| FR-008 (FR-04) | Indicator state machine (recording/processing/message) | Overlay renders each state | Overlay window has WS_EX_NOACTIVATE; foreground unchanged after show | Notepad keeps focus |
| FR-009 (FR-04, FR-27) | Capture-open errors → "microphone unavailable" + reason, no recording state | — | No device: error path | Privacy setting denied |
| FR-010 (FR-22) | Esc discards; Esc-claim failure → warning | — | Esc registered only while recording | Esc reaches Notepad when idle |
| FR-011–FR-013 (FR-05, FR-25, FR-26) | Hotkey registration state machine incl. re-register after resume | — | Conflict: another process holds the combo → error state; re-register on a simulated resume message | Sleep/lock/Explorer restart (SC-006) |
| FR-014 (FR-10) | — | — | Test window receives no hotkey characters and no menu activation | Notepad / Word menu not activated |
| FR-015, FR-016 (FR-12) | Gate with energy detector; fallback when the model fails to load; fixtures with Silero if the image has cmake (research R-6) | — | Silero on fixtures (speech/silence/cough/keyboard) | — |
| FR-017–FR-020 (FR-06, FR-11, FR-24, NFR-07) | wiremock: request shape, 401/403/413/429/5xx/malformed/empty/slow/refused; timeouts injected small | — | Same suite runs in `cargo test --workspace` | Real OpenAI and Groq keys (SC-001) |
| FR-021 (FR-09 point) | Pass-through stage is called between engine and delivery | — | — | — |
| FR-022–FR-025 (FR-10, NFR-08) | Delivery decision: paste / copy-only / manual, modifier wait | — | Clipboard formats set; SendInput Ctrl+V into a test window; elevated / not-in-front → no paste | NFR-08 app list (SC-004) |
| FR-026–FR-028 (FR-11, FR-25, NFR-06) | Pending replace rules; retry with current settings; tray error clear rules | Overlay message 3 s | Toast with Retry action fires the retry; tray icon states | Toast Retry click; Focus Assist |
| FR-029 (FR-23) | Delivery queue ordering incl. failure, no-speech and retry entries | — | — | Two windows |
| FR-030, FR-031 (FR-27) | Fallback/notify-once state machine; device-lost → process captured | — | Injected source reports device lost | USB unplug |
| FR-032 (NFR-06) | Temp audio store: delete on success, exit, stale at start | — | Temp dir empty after exit | — |
| FR-033 (FR-20, NFR-01) | Events carry only allowlisted fields; a redaction test with key/text in the inputs | — | Start smoke reads one timing line | Benchmark (SC-001) |
| FR-034 (FR-15) | Every `MessageKey` has en and ru texts | Catalog loads in the UI (covered by T-005 parity tests and `messages!`) | — | — |

## Project Structure

### Documentation (this feature)

```text
specs/001-dictation-via-api/
├── spec.md
├── plan.md              # this file
├── research.md          # Phase 0
├── data-model.md        # Phase 1
├── quickstart.md        # Phase 1
├── contracts/
│   ├── core-traits.md          # Engine, SpeechDetector, platform traits, observer
│   ├── openai-transcription.md # HTTP contract and failure classification
│   ├── ipc.md                  # overlay event, shell ↔ UI
│   └── messages.md             # message keys with en/ru texts
├── checklists/
│   ├── requirements.md
│   └── requirements-quality.md
└── tasks.md             # /speckit-tasks
```

### Source Code (repository root)

```text
crates/voicen-core/
├── src/
│   ├── lib.rs
│   ├── platform.rs          # traits: AudioSource, Clipboard, Paster, Notifier, Indicator,
│   │                        #   TempAudioStore, SettingsSource (+ DictationSettings, a projection of 004's Settings); no clock port (T-042)
│   ├── timeouts.rs          # Timeouts — single source (FR-24)
│   ├── messages.rs          # MessageKey, FailureReason → key
│   ├── events.rs            # DictationEvent (log allowlist), PipelineObserver
│   ├── audio/
│   │   ├── mod.rs           # AudioBuffer (16 kHz mono i16)
│   │   ├── resample.rs      # device rate/channels → 16 kHz mono
│   │   └── wav.rs           # WAV encoding
│   ├── recording/
│   │   ├── mod.rs           # RecordingController state machine (hold/toggle/min/max/Esc; T-042 hold)
│   │   └── indicator.rs     # IndicatorState: tray + overlay, owned by RecordingController
│   ├── hotkey.rs            # HotkeyRegistration state (active/failed, re-register)
│   ├── microphone.rs        # device choice, fallback, notify-once
│   ├── vad/
│   │   ├── mod.rs           # SpeechDetector trait, SpeechGate (model → energy fallback)
│   │   ├── energy.rs
│   │   └── silero.rs        # cfg(feature = "silero")
│   ├── engine/
│   │   ├── mod.rs           # Engine trait, TranscribeRequest, EngineError
│   │   └── openai.rs        # OpenAI-compatible client (shared with 002 local server)
│   ├── failure.rs           # FailureReason classification
│   ├── postprocess.rs       # PostProcessor trait + PassThrough (003 plugs in)
│   ├── delivery.rs          # DeliveryDecision (paste / copy-only / manual)
│   ├── queue.rs             # DeliveryQueue (recording order)
│   ├── pending.rs           # PendingRecording (at most one)
│   └── pipeline.rs          # Dictation orchestrator: ties the above together
└── tests/
    ├── fixtures/            # speech/silence/cough/keyboard WAV (MIT-compatible)
    ├── openai_client.rs     # wiremock
    ├── pipeline.rs          # end-to-end core with fakes
    └── vad_fixtures.rs      # cfg(feature = "silero")

src-tauri/src/
├── lib.rs                   # wiring, single instance, tray, setup
├── win/
│   ├── hotkey.rs            # hidden window: WM_HOTKEY, release polling, Esc, power/session msgs
│   ├── capture.rs           # cpal AudioSource
│   ├── clipboard.rs         # CF_UNICODETEXT + history-exclusion formats
│   ├── paste.rs             # foreground/integrity check, modifier wait, SendInput
│   ├── toast.rs             # WinRT toast + Retry activation
│   ├── tray.rs              # icons per state, menu, error-clear on open
│   ├── overlay.rs           # create/destroy no-activate overlay window
└── tests/                   # Windows-only integration tests (test window, injected WAV)

src/
├── routes/overlay/+page.svelte
└── lib/overlay/state.ts (+ state.test.ts)

e2e/overlay.spec.ts
i18n/en.json, i18n/ru.json   # message catalog — location coordinated with 004 (research R-14)
```

**Structure Decision**: This follows the existing three-area layout (`crates/voicen-core`, `src-tauri`, `src`), with a `win/` module inside the shell for Win32 adapters. Each core module owns one concern from the data model. `pipeline.rs` is the only place that sequences them, which gives one seam (P-011). Feature 002 adds engines under `engine/`, 003 a `PostProcessor` implementation, 004 a `SettingsSource` implementation and the settings window, and 006 the file `PipelineObserver` and the log file.

## Owner decisions needed

1. **New dependencies and toolchain installs.** These are listed in research.md (R-3, R-6, R-17). Adding Rust crates and, for Silero VAD tests on Linux, `cmake`/`clang` to `docker/rust.Dockerfile` are installs that need the owner's consent (constitution, Constraints).
2. **Message catalog location** (research R-14). It is shared by the Rust shell (toasts, tray) and the UI, and it must agree with feature 004's i18n design.

## Complexity Tracking

No constitution violations.
