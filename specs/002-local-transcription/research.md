# Research: Local Transcription

The stack is fixed by `docs/requirements.md` §9 (Tauri 2, Rust 1.99, whisper-rs, reqwest, Svelte 5, Playwright with mocked IPC). This file records only HOW the feature is built within it.

Note: the docs tool (context7) was unavailable during planning, so exact whisper-rs function names and the pinned hashes were not looked up. R-1 and R-6 name what must be confirmed in the implementing task; nothing in the design depends on a specific API spelling.

## R-1 Where whisper.cpp lives

- **Decision**: A new workspace crate `crates/voicen-whisper` implements the core's `SpeechModelLoader` / `SpeechModel` traits (contracts/core-traits.md) with `whisper-rs`, CPU only (no CUDA/Vulkan/Metal features). The shell depends on it and injects the loader. `voicen-core` has no dependency on whisper-rs.
- **Rationale**: The core's Linux gate stays fast and free of the native build; fakes cover all core logic (constitution IV). whisper.cpp is still built and run on the Windows CI runner by `cargo test --workspace` (requirements §9: "whisper.cpp transcribing a bundled WAV with a cached `tiny` model"). The crate is not Windows-specific; the toolchain image already has cmake/clang, so it can also be built on Linux later if wanted.
- **Alternatives**: whisper-rs inside `voicen-core` behind a cargo feature (feature combinations double the test matrix; rejected); inside `src-tauri` (a portable component hidden in the Windows-only shell; rejected); spawning `whisper-cli.exe` as a child process (contradicts "in-process" in §9 and architecture; considered only for R-8).
- **To confirm in the task**: current whisper-rs version and API (context creation from a file path, per-call state, full-transcription params: language `Some(code)` or auto-detect, thread count, no timestamps, single segment off; segment text iteration). Licence MIT (whisper-rs Unlicense/MIT — check per NFR-12).

## R-2 Download mechanics

- **Decision**: `reqwest` streaming GET of the catalog URL. Bytes are written to `<models dir>/<file>.part` and fed into an incremental `sha2::Sha256` in the same loop. On end of stream: check that the byte count equals the catalog size and the digest equals the pinned SHA-256; then flush and close, and atomically rename `.part` → final name. Otherwise delete `.part`. No resume (spec FR-004: Retry restarts from zero).
- **Progress**: emitted at most every 250 ms and at least once per second while data arrives (spec FR-002), plus a final event; total from the catalog size (Content-Length is only cross-checked).
- **Cancel**: a cancellation token checked in the read loop; on cancel → delete `.part`, state `NotDownloaded`, no retry offer (spec FR-005).
- **One at a time**: the downloader holds a single active slot; a second request returns `DownloadError::Busy` (spec FR-006); the UI disables other buttons.
- **Rationale**: hashing while streaming avoids reading 550 MB twice; the rename makes "not visible under the final name until verified" (spec FR-003) structural.
- **Alternatives**: hashing after download (second full read; rejected); HTTP range resume (not required, adds a state with partial trust; rejected).

## R-3 Timeouts

- **Decision**: All values come from 001's shared timeouts module (req FR-24; 001 spec FR-018; architecture "timeouts resolved in voicen-core"). This feature adds two named values there: `LOCAL_SERVER_TRANSCRIPTION = 60 s` (req FR-24) and `DOWNLOAD_NO_DATA = 30 s` (spec Clarification 5). Download uses `CONNECT = 5 s` and no total timeout (large files on slow links); the no-data timeout is a per-chunk `tokio::time::timeout` around each read. Built-in engine: no timeout (requirements define none; Findings for the owner).
- **Testability**: the timeouts struct is injected into the downloader and the client so tests use 100 ms values against a mock server that stalls.

## R-4 Model residency and the idle timer

- **Decision**: `ModelResidency` owns `Option<LoadedModel>` and an idle deadline, driven by an injected `Clock` (001's, or a small one added here if 001 has none) so the 10-minute rule is unit-tested with a fake clock (req NFR-03 "unit test of the unload timer"). API: `prewarm(model)` on recording start, `acquire()`/release guard around transcription, `recording_ended()`, `on_selection_changed()`, `unload_now()`, `tick()`. Activity guards hold the countdown; the deadline is `last activity end + 10 min`.
- **Loading** runs on a blocking thread (`spawn_blocking`); a transcription that arrives while loading awaits the same load future (spec edge case "hotkey pressed while loading"). A failed load leaves the residency empty and reports the error to the waiting transcription (spec FR-012).
- **Cold/warm**: `acquire()` returns whether this dictation triggered or waited for a load (`cold`, with load ms) or found it loaded (`warm`); the engine attaches it to 001's per-dictation log record (spec FR-015).
- **Never at start**: nothing calls `prewarm` until a recording starts with the built-in engine selected (spec FR-014).

## R-5 Threading and audio format

- **Decision**: The loaded model is used by one transcription at a time (spec FR-016): the engine serializes through the residency guard (a mutex around the model). whisper threads = `min(available_parallelism, 8)`. Audio from 001 (16 kHz mono 16-bit, decisions #1) is converted to `f32` in [-1, 1] in the core before calling `SpeechModel::transcribe`. Language: `None` for auto-detect, otherwise the ISO code from settings.
- **Rationale**: one model instance, predictable memory; ordering is already guaranteed by 001's pipeline (req FR-23).

## R-6 Catalog, repository revision and pinned hashes

- **Decision**: One `catalog.rs` const table (P-010) with: id, display name, file name (`ggml-tiny.bin`, `ggml-base.bin`, `ggml-small.bin`, `ggml-medium-q5_0.bin`, `ggml-large-v3-turbo-q5_0.bin`), URL `https://huggingface.co/ggerganov/whisper.cpp/resolve/<commit>/<file>` with one pinned repository commit, exact size in bytes, SHA-256 hex, `recommended = id == small`.
- **How the values are obtained**: in the implementing task, from the Hugging Face repository's LFS metadata at the chosen commit (the LFS pointer's `oid sha256` and `size`), then cross-checked by downloading each file once and hashing it. The values are written into the code by hand with the commit in a comment; a core test asserts the table shape (five entries, unique ids, 64-hex hashes, exactly one recommended, all URLs share the pinned commit).
- **Rationale**: pinning a commit keeps hashes valid if upstream files change (spec Assumptions).
- **Not done here**: the actual hash values — they must not be guessed.

## R-7 Local-server engine = 001's client with another config

- **Decision**: No new HTTP code (P-011, req FR-17). `local_server.rs` builds 001's `OpenAiCompatEndpoint` (name as defined by 001) from Settings: base URL, `model: Option<String>` (omitted from the multipart form when empty, spec Clarification 4), `key: Option<SecretRef>` (no `Authorization` header when absent), timeouts `connect 5 s / transcription 60 s`. Failure reasons are 001's (spec FR-019 of 001): connect refused / connect timeout → "cannot reach <host:port>".
- **Dependencies on 001** (coordination): the client must accept an optional model, an optional key, and a per-endpoint transcription timeout; the "cannot reach" reason must include the port when the URL has one (`localhost:8000`, req FR-17 acceptance). Recorded as a finding.
- **Key slot**: credential target `Voicen/local-server`, separate from the API engine's key (spec FR-018); written by 004's key UI.

## R-8 Failure isolation of native code (NFR-07)

- **Finding**: whisper.cpp/ggml may abort the process on internal asserts (e.g. a malformed model file), and the workspace release profile sets `panic = "abort"`, so neither a native abort nor a Rust panic inside the engine can be caught in-process.
- **Decision (within the fixed stack)**: keep in-process (§9), and reduce the risk: verify size and the ggml magic header before loading; load only files that passed SHA-256 at download; return every whisper-rs `Result` error as an engine failure (spec FR-012); never `unwrap` in the engine path.
- **Alternative for the owner**: a child-process worker would make native aborts survivable but contradicts "in-process" in §9 and costs extra RAM and startup. Listed under Findings for the owner.

## R-9 Free disk space

- **Decision**: a core trait `DiskSpace { fn available_bytes(&self, dir: &Path) -> io::Result<u64> }`; Windows impl in the shell via `GetDiskFreeSpaceExW` (`windows` crate, already in §9); fake in core tests. Required = catalog size + 1 % margin. If the probe errors, the download proceeds (disk-full during download follows the interrupted branch).

## R-10 CI model cache and fixtures

- **Decision**: The Windows CI job caches `ggml-tiny.bin` (`actions/cache`, key = pinned commit + hash). On a cache miss the file is fetched by a small test-only binary/example in `voicen-core` that uses the app's own downloader and catalog — so the pinned `tiny` hash is checked against the real source in CI. A short 16 kHz WAV of known speech (MIT/CC0, a few seconds, committed under `crates/voicen-whisper/tests/fixtures/`) is transcribed; the test asserts a non-empty result containing an expected word, for fixed language and auto-detect.
- **Core fixtures**: a few-KB fake "model" file with its SHA-256 computed in the test, served by the mock HTTP server with configurable truncation, corruption, stall, error status and connection drop.
- **Mock HTTP server**: the same mock-server helper 001 uses for its client tests (one helper, P-011); if 001 has none yet, `wiremock` (MIT/Apache-2.0) as a dev-dependency, plus a raw `tokio` TCP listener for stall/drop cases wiremock cannot express.

## R-11 Logging

- **Decision**: Through 001's logging facility only: events `model_download_started{id}`, `model_download_done{id, ms}`, `model_download_failed{id, reason}`, `model_download_cancelled{id}`, `model_deleted{id}`, `model_loaded{id, ms}`, `model_load_failed{id, reason}`, `model_unloaded{id, cause: idle|engine_changed|model_changed|deleted}`; dictation records gain `local: cold|warm` and `load_ms`. Never audio, text, URL credentials or keys; the local-server URL is logged as host:port only (spec FR-024).

## R-12 UI

- **Decision**: `LocalModelList.svelte` renders the catalog view returned by `local_models_list`, listens to `local-model://progress` and `local-model://state` events, offers Download / Cancel / Retry / Delete (with confirm) per row, marks `small` as recommended, disables Download on other rows while one runs, and shows reasons as message keys (en/ru from 004's i18n). Model *selection* and the local-server fields belong to 004's settings form; this component exposes which models are selectable.
