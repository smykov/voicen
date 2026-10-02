# Implementation Plan: Local Transcription

**Branch**: `002-local-transcription` | **Date**: 2026-10-02 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/002-local-transcription/spec.md`

## Summary

Two engines that plug into the dictation pipeline of `001-dictation-via-api` without changing recording or delivery code (req NFR-11):

1. **Built-in local engine** (req FR-07, FR-08, FR-28, NFR-01, NFR-03, NFR-05): a fixed catalog of five multilingual whisper ggml models with pinned SHA-256; a downloader that streams into a `.part` file while hashing, reports progress, enforces a 5 s connect and 30 s no-data timeout, supports cancel, and only renames to the final name after the hash matches; a model store that lists models from disk at start and deletes leftovers and models; a **model residency** component that loads the selected model in parallel with a recording, keeps it loaded, and unloads it after 10 minutes idle or on engine/model change or deletion; and a `BuiltinEngine` that implements 001's engine trait over a `SpeechModel` trait. The real `SpeechModel` is whisper.cpp through `whisper-rs` in a separate crate `crates/voicen-whisper`; the core uses a fake.
2. **Local-server engine** (req FR-17, FR-24): no new client. It is 001's OpenAI-compatible transcription client built from a local-server endpoint config (user URL, optional model, optional key from the credential store under its own slot) with the 60 s transcription timeout taken from the shared timeouts module.

The shell (`src-tauri`) adds IPC commands and events for the model list, download, cancel and delete, and wires the residency hooks to recording start and settings changes. The UI adds the local-model list component hosted by 004's engine settings tab.

## Technical Context

**Language/Version**: Rust 1.99 (edition 2021) for `voicen-core`, `voicen-whisper` and `src-tauri`; TypeScript + Svelte 5 for the UI.

**Primary Dependencies**: `whisper-rs` (whisper.cpp, CPU only) in `crates/voicen-whisper`; `reqwest` (streaming body, already used by 001's client) for the download; `sha2` for incremental SHA-256; `tokio` (timers, `spawn_blocking`, cancellation) as used by 001; Tauri 2 IPC and events; `@tauri-apps/api` in the UI.

**Storage**: Files only. Models in `<data dir>/models` where the data dir `%LOCALAPPDATA%\Voicen` is resolved once by the shell (architecture, P-010) and passed to the core. Partial downloads as `<file>.part` in the same directory. Local-server key in Windows Credential Manager through 001/004's credential trait, own slot `local-server`. Local-server URL and model name in Settings (owned by 004).

**Testing**: `cargo test -p voicen-core` in `voicen-rust:1.99` on Linux (fake `SpeechModel`, fake clock, fake disk-space probe, temporary model directory, mock HTTP server for download and local server); `cargo test --workspace` on the Windows CI runner (real whisper.cpp transcribing a bundled WAV with the cached `tiny` model; shell IPC commands); vitest + Playwright with mocked IPC for the UI.

**Target Platform**: Windows 10/11 x64; core logic platform-independent.

**Project Type**: Desktop app (Tauri 2) — core library + Windows shell + web UI.

**Performance Goals**: p90 stop → paste ≤ 10 s with `small`, warm, on the reference machine (req NFR-01, OQ-03). Model load overlaps with recording (spec FR-015).

**Constraints**: No network calls from the built-in engine (req NFR-05); idle RAM ≤ 150 MB with no model loaded (req NFR-03); model unloaded 10 min after last use; installed size ≤ 100 MB without models (req NFR-09) — models are never bundled; MIT-compatible licences only (req NFR-12); never log audio, text or keys (req FR-20, NFR-04).

**Scale/Scope**: 5 catalog entries; 1 download at a time; at most 1 loaded model; model files 75–550 MB.

**Unknowns resolved in research.md**: crate placement of whisper-rs (R-1), download mechanics and timeouts (R-2, R-3), residency and threading (R-4, R-5), pinned hashes and repository revision (R-6), local-server client reuse (R-7), failure isolation of native code (R-8), disk-space probe (R-9), CI model cache (R-10). No NEEDS CLARIFICATION remains.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | How this plan complies | Status |
|---|---|---|
| I. PRINCIPLES.md | P-004/P-005: every guarantee in the spec has a red test task, failure branches included (interrupted download, bad hash, no model, load failure, server down, timeout, delete refused). P-009: the local-server key is read from the credential store by the client and masked there; download/load/unload log lines carry the model id only. P-010: catalog, timeouts and the data dir each have one resolver. P-011: the local server reuses 001's client — no second HTTP transcription path; the built-in engine plugs into the one engine trait. P-013: the local-server URL/model/key are proven to reach the runtime by a test through the real settings loader. | PASS |
| II. Privacy by default | Built-in engine has no HTTP dependency (`voicen-whisper` does not depend on `reqwest`); a core test with a request-recording network fake proves zero requests (spec SC-003). Key only in Credential Manager. | PASS |
| III. Never lose the user's words | Every engine failure → FR-11 path (no paste, notify, audio pending). No model → notify with settings link, nothing pasted. | PASS |
| IV. Core / thin shell | Catalog, store, downloader, residency, built-in engine and local-server config live in `voicen-core` and are tested on Linux with fakes. whisper.cpp sits behind the `SpeechModel` trait in its own crate; Windows-only code (disk-space probe impl, IPC, events) in `src-tauri`. | PASS |
| V. Measured, not assumed | Cold/warm marking and load duration in the dictation log line feed the NFR-01 benchmark; verification location stated per requirement below. | PASS |

**Post-design re-check (after Phase 1)**: PASS. One justified addition: a new workspace crate (see Complexity Tracking).

### Where each requirement is verified

| Requirement | Linux host — core with fakes | Linux host — UI with mocked IPC | Windows CI runner | Owner's manual check |
|---|---|---|---|---|
| FR-07 built-in transcription, no-model branch | BuiltinEngine with fake `SpeechModel`: text, no-model → `NoLocalModel`, load/transcribe error → engine failure, language passed through | "no local model" message key exists in en/ru | `voicen-whisper` transcribes bundled WAV with cached `tiny` (fixed language and auto) | offline dictation into Notepad with `small`, network off |
| FR-08 download, progress, SHA-256, retry | downloader vs mock HTTP server: progress, hash match → final file; truncated/altered/stalled/HTTP 500/connect refused → `.part` deleted, `Failed{reason}`; cancel; one at a time; disk space; start-up cleanup | model list, progress bar, Retry, Cancel, recommended badge (Playwright) | CI downloads `tiny` through the app's own downloader with the pinned hash (cache miss) | download `small` from Hugging Face on a real connection |
| FR-17 local server | engine built from local-server config vs mock OpenAI-compatible server: no `Authorization` without key, bearer with key, model sent only when set; server down → "cannot reach host:port" | — | key read from Credential Manager slot `local-server` | dictation through speaches at `http://localhost:8000/v1` |
| FR-24 local-server 60 s / connect 5 s | timeouts with injected short values; values come from the shared timeouts module (asserted) | — | — | — |
| FR-28 model deletion (deletion part) | store delete: file gone, state not downloaded; selected → engine none persisted; in use → refused; remove error → stays downloaded | Delete button and confirmation (Playwright) | delete of a real file after the model was loaded and unloaded (Windows file locking) | — |
| NFR-01 (local, warm) | log line marks `cold`/`warm` and load ms (fake clock) | — | — | benchmark of 20 phrases with `small`, warm |
| NFR-03 warm model, 10 min unload | residency with fake clock: stays loaded, unloads at 10 min, hold during work, unload on engine/model change and delete, never at start | — | — | RAM drops after 10 min idle (Task Manager) |
| NFR-05 no network | pipeline with built-in engine + request-recording network fake: zero requests; dependency check that `voicen-whisper` has no HTTP crate | — | — | code review of network call sites |

## Project Structure

### Documentation (this feature)

```text
specs/002-local-transcription/
├── plan.md              # This file
├── research.md          # Phase 0
├── data-model.md        # Phase 1
├── quickstart.md        # Phase 1
├── contracts/
│   ├── core-traits.md   # SpeechModel, ModelResidency, ModelStore, Downloader, DiskSpace — Rust interfaces
│   └── ipc.md           # Tauri commands and events for the local-model list
└── tasks.md             # Phase 2 (/speckit-tasks)
```

### Source Code (repository root)

```text
crates/voicen-core/src/
├── timeouts.rs                  # shared FR-24 values (from 001) + local server 60 s, download no-data 30 s
├── local_models/
│   ├── mod.rs
│   ├── catalog.rs               # the five models: id, file name, URL at pinned revision, size, SHA-256, recommended
│   ├── store.rs                 # ModelStore: states from disk, start-up cleanup, finalize, delete
│   ├── download.rs              # Downloader: stream → .part + SHA-256, progress, cancel, timeouts
│   └── residency.rs             # ModelResidency: load/keep/unload, idle timer, hold, cold/warm
├── engines/
│   ├── builtin.rs               # BuiltinEngine: Engine trait (001) over SpeechModel
│   ├── speech_model.rs          # SpeechModel / SpeechModelLoader traits
│   └── local_server.rs          # endpoint config → 001's OpenAI-compatible client
└── (tests in each module + crates/voicen-core/tests/local_*.rs)

crates/voicen-whisper/           # NEW: whisper-rs implementation of SpeechModelLoader (CPU only)
├── Cargo.toml
├── src/lib.rs
└── tests/transcribe_wav.rs      # runs on the Windows CI runner with the cached tiny model

src-tauri/src/
├── local_models.rs              # IPC commands + events, DiskSpace impl (GetDiskFreeSpaceExW)
└── lib.rs                       # wiring: residency hooks on recording start and settings change

src/lib/local-models/
├── LocalModelList.svelte        # hosted by 004's engine tab
├── localModels.ts               # IPC wrapper (invoke/listen)
└── localModels.test.ts

e2e/local-models.spec.ts         # Playwright with mocked IPC
tests/fixtures/ (core)           # tiny fake model bytes for the mock download server; bundled WAV for CI
```

**Structure Decision**: Keep the existing three areas (core, shell, UI). Add one crate `crates/voicen-whisper` so that `voicen-core` stays free of the native whisper.cpp build (the Linux gate builds and tests `voicen-core` only, decisions #5) while whisper.cpp itself is still built and tested by `cargo test --workspace` on the Windows CI runner. The shell depends on both crates and injects `voicen-whisper`'s loader into the core.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| New workspace crate `voicen-whisper` | Keeps the native C++ build (cmake, bindgen) out of the core's Linux gate and out of fast core tests; makes "the built-in engine has no HTTP dependency" a checkable crate-level fact (NFR-05) | Putting whisper-rs in `voicen-core` makes every core test build whisper.cpp; putting it in `src-tauri` would hide a platform-independent component inside the Windows-only shell and prevent ever testing it on Linux |
