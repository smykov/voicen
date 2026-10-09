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
- **Where it is enforced:** `LocalModels::open(models_dir, disk, timeouts, catalog)` runs `cleanup_at_start` before it returns; `run()` builds it once from `paths::models_dir()`, the only resolver of the release models dir, only after tauri's `build()` (so a second instance never deletes the primary's `.part`; T-052, `docs/decisions/windows-shell.md`). Tests `local_models_service` (open deletes `*.part`), shell `local_models.rs`.
- **Don't:** build a `ModelStore` or `Downloader` elsewhere, or a second models-dir path.

### One `Arc<ModelStore>` is shared by settings validation and IPC (T-044)

- **What breaks if you violate it:** the list says "downloaded" while `settings_save` refuses the model, or the reverse.
- **Where it is enforced:** `LocalModels::store()` is the `SettingsDeps.local_models` of `load_settings` and the store behind every local-model command. Shell test: download through IPC, then `settings_save` with `builtin_local` and that id is Saved.
- **Don't:** pass a separate `DownloadedModels` to settings.

### The in-memory state wins over the disk state (T-044)

- **What breaks if you violate it:** during a download or after a failure the list shows `not_downloaded` and loses the progress or the failure reason.
- **Where it is enforced:** `LocalModels::list` takes the transient `Downloading`/`Failed` entry before the disk state. `Failed` keeps its reason until a retry; a new `open` forgets it. Test `local_models_service`.
- **Exception (T-044 review round 1 #1):** a `Failed` never hides a disk `Downloaded`. If the final file appears outside a download (a manual copy, a second instance), `list` shows `Downloaded` and drops the stale `Failed`; an `already_downloaded` refusal drops it too instead of restoring it. Otherwise the list says `failed`, Retry is refused, and `settings_save` accepts the model through the same store. Test `a_failed_never_hides_a_model_downloaded_on_disk_and_a_retry_does_not_bring_it_back`.
- **Don't:** derive the list from `ModelStore::states()` alone, or let a transient `Failed` win over a disk `Downloaded`.

### Events are emitted after the state update, with no lock held (T-044)

- **What breaks if you violate it:** events out of order, a list after an event that disagrees with it, or a deadlock (`emit` runs Rust `listen_any` handlers inline, and a handler that invokes a command takes the lock again).
- **Where it is enforced:** every `local-model://` event comes only from the download thread's callback: update the transient map, release the lock, then emit. `LocalModels::download` holds the map lock across `start`, since core never calls the callback inside `start`, and restores the previous state on `Err`. The command itself emits nothing. Events go to the settings window only (`emit_to`); "only the settings window may listen" rests on decision #45's capability.
- **Relies on:** `Downloader::start` never calls `events` on the caller's thread (stated in its doc); a call there would deadlock `download` on its own lock.
- **Accepted gap (T-044 review round 1 #2):** `Job::run` frees the slot (`guard.release()`) before its end event, so a new `download` of the same model can start before the old end event is recorded. The old end state then overwrites the new `Downloading` (and its state event reaches the UI after the new start) until the new download's first progress event, or until its own end event if that comes first (a download that ends before any progress). If the new download had already ended before the late event was recorded, the stale state stays until the next start; a late `Failed` over a finished download is still hidden by the disk `Downloaded` rule above (T-044 review round 2 Low 2, reworded in T-019). The gap is a few statements long, the UI offers Retry only after the end event, and it cannot be tested deterministically; a per-start token that ignores superseded events is the fix if it ever matters.
- **Don't:** emit inside the lock, or emit from the command. A delete emits no `local-model://` event at all (T-019, below).

### A model is deleted only by `LocalModels::delete`, in one order (T-019)

- **Defect that produced it:** none; a gap (FR-28 deletion part, spec US4). The contract had `ModelStore::delete` beside the coordinator (decision #57), which could not clear the coordinator's transient `Failed`, and a reset through `SettingsService::save` that a credential read failure or an invalid hand-edited field refuses.
- **What breaks if you violate it:** a model is removed while a transcription holds it or while the residency reloads it; the window shows a stale `failed` for a deleted model; the settings keep naming a model that is gone, or are reset although the file is still there.
- **Where it is enforced:** `LocalModels::delete(id, residency: &dyn ModelRelease, settings: &SettingsService)` is the one delete path; the shell command `local_model_delete` only delegates, with `NoResidency` until T-017's `ModelResidency` implements `ModelRelease`. Order: `not_downloaded` unless the final file has the catalog size and no download of `id` runs (an unknown id too; the residency is not asked) → `release_for_delete` (`InUse` → `model_in_use`, nothing changed; the guard blocks a reload until it drops) → remove the final file (`NotFound` counts as removed; any other error → `delete_failed`, the settings untouched) → drop the transient state of `id` → `SettingsService::forget_model(id)` (a write failure is `resetFailed`, the removal is not undone, OQ-26 (a)) → drop the guard. The transient map stays locked throughout, which serializes the delete with `download` and `list`; `forget_model` takes only the settings `save_lock`, and nothing under it reads the transient map. Tests: `tests/local_model_delete.rs`, `local_models::service::delete_tests` (the removal error, injected through the private `remove` field the gate needs because it runs as uid 0), Windows CI `src-tauri/tests/local_models.rs` (a file held open without `FILE_SHARE_DELETE` → `delete_failed`).
- **Don't:** remove a model file anywhere else, delete before the release or drop the guard before the removal, reset the selection through `save`, cancel a running download from a delete, or emit an event from the delete (the UI re-lists after the invoke settles; a reset reaches it as `settings://changed`).

### Every refusal maps to a contract code (T-044)

- **What breaks if you violate it:** the UI gets an unmapped error with no message id.
- **Where it is enforced:** `From<&DownloadError>` / `From<&DownloadFailure>` for `ReasonView` in core; the code and message tables are in `contracts/ipc.md`, not repeated here. An id string `ModelId::parse` rejects is `not_in_catalog`; `cancel` with an unknown id returns `false`. Test `local_models_service` covers every `DownloadError`. `From<&DeleteError>` maps the delete refusals (`model_in_use`, `not_downloaded`, `delete_failed`; `delete.*` ids); none shares a code with a download reason (`e2e_local_models_wire_fixture_matches_core`).
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
| `ModelStore::delete` (contracts before T-019) | cannot clear the coordinator's transient `Failed`; a second entry point beside decision #57 | T-019 analysis B |
| Selection reset through `save(snapshot, keys Untouched)` | a credential read failure or an invalid hand-edited field refuses it; a snapshot taken outside `save_lock` loses a concurrent save | T-019 analysis C |
| `local-model://state` "deleted" emitted by the command | breaks "events only from the download thread"; the UI re-lists instead | T-019 analysis E |

## Open

- T-049: ACL for app commands.
