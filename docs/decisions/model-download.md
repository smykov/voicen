# Local model download (core coordinator and shell adapter)

**Code:** `crates/voicen-core/src/local_models/{catalog,store,download,service}.rs` (T-016, T-044); `src-tauri/src/local_models.rs`, `src-tauri/src/paths.rs` (`models_dir`), `src-tauri/src/lib.rs` (wiring) (T-044) · **Tests that pin it:** core `local_download`, `local_download_refused`, `local_store`, `local_models_service`; shell `src-tauri/tests/local_models.rs` (Windows CI only)

Tasks: T-016 (core half), T-044 (coordinator and shell half). Contract: `specs/002-local-transcription/contracts/{core-traits,ipc}.md`. Decisions: #22, #42, #49, #50, #57. Related: F-004, F-005 (test-only defects in this class's tests, see `docs/decisions/core-tests.md`).

## Invariants

### A model file appears under its final name only after size and SHA-256 match (T-016)

- **Defect that produced it:** none yet (found in planning, T-016).
- **What breaks if you violate it:** a truncated or altered model is read as a usable one; the settings validation accepts `builtin_local` for it.
- **Where it is enforced:** one path in `Downloader`: rename `<file>.part` after the byte count equals the catalog size and the streamed SHA-256 equals the pinned value; every other end removes `.part` before the end event. `ModelStore` derives "downloaded" from disk only (final file with the catalog size). Tests `local_download`, `local_store`.
- **Don't:** add a second writer of the final name, or mark a model downloaded from memory state.

### The no-data timeout is per read, never a total timeout (T-016, decision #49)

- **Defect that produced it:** none yet. `RequestBuilder::timeout` is a total deadline over the whole body (reqwest 0.13.5), and `OpenAiCompatibleEngine` uses it; copying it would end every large download after the limit.
- **What breaks if you violate it:** a slow but steady download of a 1.9 GB model is cut off.
- **Where it is enforced:** `ClientBuilder::timeout(download_no_data)` plus `connect_timeout`; the trickle test in `local_download` fails with a total timeout.
- **Don't:** reuse the engine's request timeout.

### The downloader is synchronous and has one active slot (T-016, decisions #22, #42, #49)

- **What breaks if you violate it:** a second concurrent download, or blocking reqwest running on a tokio worker (T-013 hazard).
- **Where it is enforced:** `Downloader::start` runs the work on its own std thread; the reqwest client is built and used only there. Only `Downloader::start` begins a download. The shell adds no reqwest call and drops no reqwest object on a command thread.
- **Don't:** make the downloader async or start a transfer from a command.

### The SHA-256 comes from `sha2` 0.10 (decision #50)

- **Where it is enforced:** the dependency in `voicen-core`, `make licenses`. Catalog commit, sizes and hashes come from the public Hugging Face LFS metadata (R-6); the catalog test checks five entries, unique ids and files, 64-hex hashes, one pinned commit.

### One `LocalModels` per process, made only by `open` (T-044, decision #57)

- **Defect that produced it:** none yet (found in T-044 analysis). The interim `NoDownloadedModels` always said "not downloaded", so every `builtin_local` save was refused.
- **What breaks if you violate it:** a store that was never cleaned (stale `.part`), or two stores that disagree about what is downloaded.
- **Where it is enforced:** `LocalModels::open(models_dir, disk, timeouts, catalog)` runs `cleanup_at_start` before it returns; `run()` builds it once from `paths::models_dir()`, the only resolver of the release models dir. Tests `local_models_service` (open deletes `*.part`), shell `local_models.rs`.
- **Don't:** build a `ModelStore` or `Downloader` elsewhere, or a second models-dir path.

### One `Arc<ModelStore>` is shared by settings validation and IPC (T-044)

- **What breaks if you violate it:** the list says "downloaded" while `settings_save` refuses the model, or the reverse.
- **Where it is enforced:** `LocalModels::store()` is the `SettingsDeps.local_models` of `load_settings` and the store behind every local-model command. Shell test: download through IPC, then `settings_save` with `builtin_local` and that id is Saved.
- **Don't:** pass a separate `DownloadedModels` to settings.

### The in-memory state wins over the disk state (T-044)

- **What breaks if you violate it:** during a download or after a failure the list shows `not_downloaded` and loses the progress or the failure reason.
- **Where it is enforced:** `LocalModels::list` takes the transient `Downloading`/`Failed` entry before the disk state. `Failed` keeps its reason until a retry; a new `open` forgets it. Test `local_models_service`.
- **Don't:** derive the list from `ModelStore::states()` alone.

### Events are emitted after the state update, with no lock held (T-044)

- **What breaks if you violate it:** events out of order, a list after an event that disagrees with it, or a deadlock (`emit` runs Rust `listen_any` handlers inline, and a handler that invokes a command takes the lock again).
- **Where it is enforced:** every `local-model://` event comes only from the download thread's callback: update the transient map, release the lock, then emit. `LocalModels::download` holds the map lock across `start`, since core never calls the callback inside `start`, and restores the previous state on `Err`. The command itself emits nothing. Events go to the settings window only (`emit_to`); "only the settings window may listen" rests on decision #45's capability.
- **Don't:** emit inside the lock, or emit from the command.

### Every refusal maps to a contract code (T-044)

- **What breaks if you violate it:** the UI gets an unmapped error with no message id.
- **Where it is enforced:** `From<&DownloadError>` / `From<&DownloadFailure>` for `ReasonView` in core; the code and message tables are in `contracts/ipc.md`, not repeated here. An id string `ModelId::parse` rejects is `not_in_catalog`; `cancel` with an unknown id returns `false`. Test `local_models_service` covers every `DownloadError`.
- **Don't:** return a core error's text or a reqwest error to the window.

### App commands are not ACL-checked from local origins (T-044, decision #57)

- **Defect that produced it:** none; a finding. tauri 2.12.1 checks app commands only when the app defines permissions (ours defines none, `build.rs` is plain `tauri_build::build()`), so `local_models_*` and `settings_*` pass for any local-origin window. Decision #45's `settings`-label limit covers plugin commands only.
- **What breaks if you violate it:** nothing today; adding permissions files under `src-tauri/permissions/` switches the ACL on for every app command at once and breaks any command without an entry.
- **Where it is enforced:** shell test that `local_models_list` resolves from the settings window under `generate_context!(test = true)`.
- **Don't:** assume the `settings` label protects app commands. Gating them per label is T-049.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Whole local-model feature in one task | three seams, about 35 tests, one review; split core / shell / UI | decisions #49 |
| Async downloader on tokio | core stays synchronous | decisions #22, #42, #49 |
| Coordinator in `src-tauri` | every logic or serialization bug costs a Windows CI round; core is proven on Linux | decisions #57 |
| Async commands with `spawn_blocking` | `start` does no network I/O | T-044 analysis |

## Open

- T-049: ACL for app commands.
