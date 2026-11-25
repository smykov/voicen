# Call storage (call folders on disk)

**Code:** `crates/voicen-core/src/call/storage.rs` (`CallStore`, `write_meta`, `write_atomic`, `fill_and_rename`, `storage_kind`, `CallMeta`; T-141: `resolve_call`, `CallStore::get`, `export_bytes`, `export_to`), `crates/voicen-core/src/settings/roots.rs` (`effective_root`, `CallRoots::probe`, `reason_of`), `crates/voicen-core/src/call/events.rs` (`CallId::parse`), `crates/voicen-core/src/call/transcript.rs` (`render_markdown`, `export_markdown`, `export_json`) · **Tests that pin it:** `call::storage` unit tests (`create_*`, `a_failed_*`, `write_meta_*`, `list_*`, `damaged_*`, `missing_*`, `storage_kind_maps_every_op_encode_to_write`; T-141: `get_*`, `export_*`), `call::transcript` `export_markdown_is_render_markdown_with_meta_names`, `export_json_carries_meta_title_and_names_and_validates`

Tasks: T-125, T-141, T-144 (recovery, ownership) (wire, guard, export; `ipc.md` › Built T-141). Contract: `specs/007-call-mode-record/contracts/core-call.md` "Built (T-125)". Decisions: #195, #196, #242 (D1–D5). Class `call-storage` in `docs/failures.md` has no entry yet (first sighting).

## Invariants

### A start never touches an existing folder and, when refused, leaves nothing new on disk

- **Defect that produced it:** none yet (found in planning, T-125). Closest history: settings keeps every backup by reserving names with `create_new` (`docs/decisions/settings.md`).
- **What breaks if you violate it:** a second call in the same second writes into the first call's folder, or a refused start leaves a half-made folder that the Calls window lists as `damaged`.
- **Where it is enforced:** `CallStore::create_with` is the only creator. The folder is made with `fs::create_dir` (never `create_dir_all`): `AlreadyExists` moves on to the suffix `-1` … `-99`; past `-99` the start is refused as `CreateError::Root(Other)`; any other error is `Root(reason_of(kind))`. Order: probe, folder, `meta.json`, `mic.opus`, `system.opus`. Any failure after the folder exists removes the folder (`remove_dir_all`, best effort; the step's own error is returned). Tests `create_names_folder_from_local_start_and_suffixes_on_clash_never_overwrites`, `create_past_suffix_99_is_refused_as_root_other_and_leaves_nothing`, `a_failed_track_write_removes_what_create_made`, `a_failed_system_track_flush_removes_both_tracks_and_the_folder`.
- **Don't:** use `create_dir_all`, treat every `create_dir` error as a clash, create a chosen root that is missing, or leave the folder for recovery to find. A crash between `create_dir` and the last file still leaves a folder that lists as `damaged`; that is accepted (never hidden). A staging-folder rename was rejected: the track files stay open and the codec cannot reopen in append.

### The probe runs first, on the effective root, at every start

- **Defect that produced it:** none yet.
- **What breaks if you violate it:** a start into a read-only or vanished root is found only when the first track write fails (FR-039).
- **Where it is enforced:** `create_with` resolves the root with `settings::roots::effective_root(calls, default_root)` (the chosen root, else the default; the one resolution, shared with `SettingsService::save`) and calls `CallRoots::probe` on it before anything else; a refusal is `CreateError::Root(UnwritableReason)` and touches nothing. There is no second probe. Test `create_runs_the_probe_and_refusal_leaves_nothing`.
- **Don't:** probe another root, probe after the folder, or add a probe of the call side's own.

### Every `meta.json` write is tmp + sync + rename through `write_meta`

- **Defect that produced it:** none yet.
- **What breaks if you violate it:** a crash or a full disk leaves a partial `meta.json`, so the call lists as `damaged` or loses its state.
- **Where it is enforced:** `write_meta` (the first write at create and `ActiveCallFiles::write_meta`) goes through `pub(crate) write_atomic(dir, name, bytes)`: `<name>.tmp` created, written and synced, renamed over `<name>`; on failure the previous file is untouched and the tmp this call created is removed. The key order is the data-model "Call" field order, pinned by `meta_json_keys_follow_data_model_order_and_start_recording`. The tmp+sync+rename tail exists once, as the private `storage::fill_and_rename` (T-141): write, `sync_data`, close, rename, remove the tmp on any failure. `write_atomic` opens its tmp with `File::create` (the call folder is ours, so a stale `<name>.tmp` from a crash is replaced) and calls the tail; `transcript.*` writers (the final pass, T-138) reuse `write_atomic`. Tests `write_meta_replaces_meta_json_and_leaves_no_tmp`, `write_meta_failure_keeps_previous_meta_json_and_no_tmp`, `active_call_write_meta_goes_through_the_atomic_writer`.
- **`drops[]` (T-126, decision #244):** `CallMeta.drops: Vec<TrackDrop { at_ms, track, blocks, ms }>` sits right after `device_changes` in the closed schema (`deny_unknown_fields`, required), is `[]` at create, and is appended by the session's writer thread through `write_meta`. Schema stays 1: no call folder exists outside dev builds. An entry outside the closed shape lists the call as damaged (`meta_json_drops_outside_the_closed_shape_lists_as_damaged`). Whose line and entry: `docs/decisions/call-session.md` I7.
- **Don't:** write `meta.json` in place, copy the helper, or swallow a write error at its origin (a failed first write is `Storage(Meta)`).

### The list shows every call of every root, and hides nothing that is a call

- **Defect that produced it:** none yet.
- **What breaks if you violate it:** a damaged call disappears from the Calls window with its audio still on disk (FR-033), or a user folder, the probe file or `meta.json.tmp` shows as a call.
- **Where it is enforced:** `CallStore::list` reads the current root, then every earlier root, and never creates anything.
  - Only names in the `CallId` grammar are calls (D2); anything else is skipped.
  - A call folder whose `meta.json` is missing, unparsable, carries an unknown key (D4, `deny_unknown_fields`) or a value outside a closed set (`device_changes[].cause`, `drops[]` shape, `pass_mode`, `results.<track>`, which is `pending`, `done{segments}` or `failed{reason}`; the final pass writes the last two, T-138: `docs/decisions/call-final-pass.md`), is of another schema, names another id, has other tracks, or lacks a track file lists as `ListState::Damaged` with `meta: None`; so does `failed` without its `failure`.
  - A missing default current root lists as empty (it is created at the first start); any other root that cannot be read (missing chosen or earlier root, a file, a `read_dir` error) is one `unavailable` line, and the readable roots still list (D3).
  - Order: newest first by (start stamp, numeric clash suffix), so `-10` precedes `-2`; ties keep root order, current root first. `CallId`'s own `Ord` is string order and is not used.
  - Tests `list_current_and_earlier_newest_first_with_numeric_suffix_order`, `list_skips_names_that_are_not_a_call_id`, `missing_default_root_lists_empty_without_unavailable`, `missing_chosen_or_earlier_root_gives_one_unavailable_line`, `damaged_*`, `meta_json_with_unknown_key_lists_as_damaged`, `meta_json_outside_the_closed_sets_lists_as_damaged`.
- **Don't:** sort by `CallId`, list non-grammar names as damaged, hide a damaged folder, or let one unreadable root empty the list.

### A failed track step maps to the log's storage kind in one place; `Encode` is `write`

- **Defect that produced it:** none yet. `StorageKind` has no `encode` literal and `call_storage_failed kind=` has none in its grammar (D1).
- **What breaks if you violate it:** a new log literal needs `events.rs`, `diag/format.rs`, the `diag_format` tests and the contract to change together.
- **Where it is enforced:** `storage::storage_kind(StorageOp)`: `Write` and `Encode` to `Write`, `Flush` to `Flush`, `Sync` to `Sync`. Test `storage_kind_maps_every_op_encode_to_write`.
- **Don't:** map `Encode` at a call site, or add an `encode` literal without the whole set above.

### An export never opens a file the user already has, and a wire `{root, id}` never becomes a path unchecked

- **Defect that produced it:** none yet (found in analysis, T-141). Rejected: export through `write_atomic` unchanged, because `File::create` would truncate, and then rename over or remove, a user's own `<file>.tmp` in the chosen folder.
- **What breaks if you violate it:** an export destroys a user file next to the target, or leaves a partial tmp; a crafted `root` or `id` reads or exports a folder outside the settings roots (P-009).
- **Where it is enforced:**
  - `export_to` reserves `<file>.voicen-tmp`, then `-1` … `-9`, with `create_new` (`reserve_export_tmp`), and calls `fill_and_rename`; all ten names taken is `ioFailed{other}`. A file under a tmp name is never opened, truncated or removed.
  - `resolve_call` / `guard` compare the wire `root` for equality with the current or an earlier root of settings and parse `id` with `CallId::parse`; they make no filesystem call and build no path from wire text. A root that is not valid Unicode lists (`wire_root` is lossy) but cannot be opened.
  - `CallStore::folder(calls, root, id)` (`calls_open_folder`, FR-033, T-142) runs the same `guard` first, then `is_dir` on `root/id`: a damaged call (no `meta.json`) is allowed, a folder that is gone (or not a folder) is `notFound`. A guard miss is `notFound` before any filesystem call.
  - Tests `a_root_that_is_not_current_or_remembered_is_not_found_without_fs`, `an_id_outside_the_grammar_is_not_found`, `export_never_touches_an_existing_tmp_name`, `export_write_or_sync_failure_removes_its_tmp_and_reports_the_reason`.
- **Don't:** call `write_atomic` for a user-chosen folder, `canonicalize` or `starts_with` the wire root, or copy the tail.
- **Developer deviation (T-141):** the tail syncs with `sync_data`, not `sync_all`, so `write_atomic` (meta.json) changed too. Contents and size are flushed, which is what a crash-safe rename needs; other metadata (timestamps) may lag. Not an owner decision.

### Recovery rewrites only `meta.json`, and only for a call this process is not recording (T-144)

- **Defect that produced it:** none yet (found in analysis, T-144).
- **What breaks if you violate it:** a crashed call stays `recording` forever, or recovery rewrites the meta (or touches a track) of a call that is being recorded, or a half-rewritten meta loses the call.
- **Where it is enforced:**
  - Ownership: `CallStore` keeps a set of active folders. `create_with` adds the folder, `release(&Path)` removes it (the session calls it when a call ends, and on a start that fails after the create). `list` and `open_call` map a `recording` meta to Recording only for an active folder; any other is `Recovered`.
  - Ownership is per `CallStore` instance, not per process or per folder on disk. So the process has one `CallStore`, one `Arc<CallStore>` shared by the call session (`CallDeps.store`), the Calls IPC (`list`, `get` / `open_call`) and the start-up `recover`. The shell wiring (T-139, T-145) must not build a second store: a second store sees the live call as not active, lists it as Recovered and lets its `recover` rewrite the meta to `recovered` mid-recording (a primary started with `--call record` may record before recovery runs).
  - A set, not one slot: one session has at most one active call, so the session alone would do with a slot. The set keeps `release(dir)` keyed by the folder (releasing a folder that is not active changes nothing), and every create in one store stays active until its own release, which the store's tests rely on (they create several calls in one store without a session, e.g. `create_names_folder_from_local_start_and_suffixes_on_clash_never_overwrites`).
  - `CallStore::recover(&CallSettings, &Log) -> Recovery { recovered, queue }` runs at start-up over the roots `list` reads. It only reads tracks; the one write is `write_meta` (atomic, above). A `recording` call that is not active becomes `recovered` with `duration_ms` = the shorter track (start offset plus the scanned length; an unreadable track counts 0, offset included) and `ended_at` = `started_at` + that duration (a `started_at` that does not parse keeps the old `ended_at`). A `transcribing` call goes back to `ready-to-transcribe`. `queue` lists every `ready-to-transcribe` call, `recovered` the calls recovered now, both oldest first.
  - A folder whose meta cannot be rewritten stays as found and is in neither list. An unreadable root or a damaged folder is skipped.
  - `call_recovered calls=<n>` is logged always, `0` included.
- **Don't:** recover an active folder, open a track for writing, rewrite anything but `meta.json`, or count a folder whose rewrite failed.

### Rename, delete and the meta lock (T-152, decisions #317)

- **Defect that produced it:** none yet (found in analysis, T-152). The nearest known one is the pass overwriting a rename made during `transcribing` (`docs/decisions/call-final-pass.md`, formerly a Known gap).
- **What breaks if you violate it:** a rename is lost when a pass, a "Transcribe again" transition or recovery writes the `meta.json` it read earlier; a rename leaves `transcript.md`, `transcript.json` and `meta.json` with different names; a delete stops half way (on Windows a sharing violation on `mic.opus`) and leaves a half-deleted folder; the recording call is renamed or deleted under the session.
- **Where it is enforced:**
  - Meta lock: one process-wide `static META_LOCK` in `storage.rs` (`meta_lock()`), not a `CallStore` field, because the pass functions get a folder, not a store; a process has one store. Every `meta.json` read-modify-write of a call that is not recording runs under it: `rename`, `rename_speaker`, the four writes of a pass (start `transcribing`, the cancel write-back, `write_failed`, `write_ready` with its Markdown render), `transcribe_transition` and `recover`; delete runs under it as well. A writer that holds a meta read from earlier calls `carry_disk_names(dir, &mut meta)` right before its write, so the on-disk title and speakers win; `transcribe_transition` reads under the lock and needs no carry. The session writes its own meta only while the call is active, which rename refuses. Lock order: the queue's slots lock, then the meta lock; the meta lock is held only around file reads and writes, never while another lock is taken.
  - Rename (`CallStore::rename`, `rename_speaker`): `guard`, then `recordingCall` for an active call, `ioFailed` for a damaged one (`open_call`), `notFound` for a gone call, a foreign root or an unknown speaker id, `emptyName` for an empty or whitespace-only title or name (`transcript::check_name`); a name is stored as typed, duplicates are allowed. Write order: render every file's bytes first (no fs call), then `transcript.md`, `transcript.json` (only when it exists), `meta.json` last as the commit point, each through `write_atomic`. A failure restores the previous transcript bytes best effort and returns `ioFailed`; no file changes on `emptyName`.
  - Delete (`CallStore::delete_folder(calls, root, id, &dyn FolderDeleter)`): `folder`'s guard (a damaged call is allowed), `recordingCall` for an active call. The port claims every file of the folder by path (`claim(&Path) -> Result<Box<dyn Send>, InUse>`, `InUse` carries the bare file name, never a path); only when all claims are held are the files removed, the claims dropped and the folder removed. A refused claim drops every claim taken and returns `inUse{file}`; the folder is byte-identical. `StdFolderDeleter` never refuses (Linux, tests). The Windows impl (share mode) and its real-lock proof belong to T-153 (G39 T076) in `src-tauri`; until then the lock is unverified. The queue part (`PassQueue::delete`) is in `docs/decisions/call-final-pass.md`.
- **Don't:** write a non-session `meta.json` outside `meta_lock()`, write a meta read earlier without `carry_disk_names`, take the slots lock while holding the meta lock, remove any file of a call folder before every file is claimed, or delete through `remove_dir_all` directly.
- **Tests:** `rename_*`, `a_write_fault_at_any_of_the_three_files_…`, `a_locked_file_gives_in_use_…`, `a_damaged_folder_can_be_deleted` in `storage.rs`; the pass side in `final_pass/queue_tests.rs`. Known untested: the `recover` carry (no seam to place a rename between its read and write; review item).

## Reused, not copied

| Concern | The one place |
|---|---|
| Write probe and its reason | `settings::roots::CallRoots::probe`, `reason_of` (also the `ioFailed` reason of T-141's get and export) |
| Call id grammar | `call::events::CallId::parse` (the listing and T-141's `resolve_call`) |
| Transcript Markdown | `call::transcript::render_markdown` (T-141's `export_markdown` is it with the `meta.json` title and names) |
| tmp + sync + rename | `storage::fill_and_rename`, the one tail of `write_atomic` and T-141's `export_to` |
| Current root | `settings::roots::effective_root` (moved here from `settings/service.rs`, T-125) |
| Local time for the folder stamp, `started_at` and the default title | `diag::format::LocalTime::of` |
| `.opus` writing | `OpusOgg::create` only (T-116) |
| Disk space | `local_models::download::DiskSpace`, `FakeDisk`, the shell's `WinDiskSpace`; no call-side port type, the session holds it as `CallDeps.disk` and polls it (`docs/decisions/call-session.md` I12) |

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| A second `DiskSpace` port with `OsCause` | the existing port serves the call side; the OS code is `io::Error::raw_os_error()` | T-125 |
| An `encode` log literal | touches events, format, tests and the contract for no reader | #242 D1 |
| Unbounded clash suffixes | settings bounds backups too; past `-99` the start is refused as root `other` | #242 D5 |
| Staging folder renamed at the end | the track files stay open and the codec cannot reopen in append | T-125 analysis |

## Open

- None.
