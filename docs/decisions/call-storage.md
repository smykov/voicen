# Call storage (call folders on disk)

**Code:** `crates/voicen-core/src/call/storage.rs` (`CallStore`, `write_meta`, `write_atomic`, `storage_kind`, `CallMeta`), `crates/voicen-core/src/settings/roots.rs` (`effective_root`, `CallRoots::probe`, `reason_of`) · **Tests that pin it:** `call::storage` unit tests (`create_*`, `a_failed_*`, `write_meta_*`, `list_*`, `damaged_*`, `missing_*`, `storage_kind_maps_every_op_encode_to_write`)

Tasks: T-125, T-141 (wire, guard, export; `ipc.md` › Built T-141). Contract: `specs/007-call-mode-record/contracts/core-call.md` "Built (T-125)". Decisions: #195, #196, #242 (D1–D5). Class `call-storage` in `docs/failures.md` has no entry yet (first sighting).

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
  - Tests `a_root_that_is_not_current_or_remembered_is_not_found_without_fs`, `an_id_outside_the_grammar_is_not_found`, `export_never_touches_an_existing_tmp_name`, `export_write_or_sync_failure_removes_its_tmp_and_reports_the_reason`.
- **Don't:** call `write_atomic` for a user-chosen folder, `canonicalize` or `starts_with` the wire root, or copy the tail.
- **Developer deviation (T-141):** the tail syncs with `sync_data`, not `sync_all`, so `write_atomic` (meta.json) changed too. Contents and size are flushed, which is what a crash-safe rename needs; other metadata (timestamps) may lag. Not an owner decision.

## Reused, not copied

| Concern | The one place |
|---|---|
| Write probe and its reason | `settings::roots::CallRoots::probe`, `reason_of` |
| Current root | `settings::roots::effective_root` (moved here from `settings/service.rs`, T-125) |
| Local time for the folder stamp, `started_at` and the default title | `diag::format::LocalTime::of` |
| `.opus` writing | `OpusOgg::create` only (T-116) |
| Disk space (polling is a later task) | `local_models::download::DiskSpace`, `FakeDisk`, the shell's `WinDiskSpace`; no call-side port |

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| A second `DiskSpace` port with `OsCause` | the existing port serves the call side; the OS code is `io::Error::raw_os_error()` | T-125 |
| An `encode` log literal | touches events, format, tests and the contract for no reader | #242 D1 |
| Unbounded clash suffixes | settings bounds backups too; past `-99` the start is refused as root `other` | #242 D5 |
| Staging folder renamed at the end | the track files stay open and the codec cannot reopen in append | T-125 analysis |

## Open

- None.
