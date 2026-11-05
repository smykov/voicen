# Call storage (call folders on disk)

**Code:** `crates/voicen-core/src/call/storage.rs` (`CallStore`, `write_meta`, `write_atomic`, `storage_kind`, `CallMeta`), `crates/voicen-core/src/settings/roots.rs` (`effective_root`, `CallRoots::probe`, `reason_of`) · **Tests that pin it:** `call::storage` unit tests (`create_*`, `a_failed_*`, `write_meta_*`, `list_*`, `damaged_*`, `missing_*`, `storage_kind_maps_every_op_encode_to_write`)

Tasks: T-125. Contract: `specs/007-call-mode-record/contracts/core-call.md` "Built (T-125)". Decisions: #195, #196, #242 (D1–D5). Class `call-storage` in `docs/failures.md` has no entry yet (first sighting).

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
- **Where it is enforced:** `write_meta` (the first write at create and `ActiveCallFiles::write_meta`) goes through `pub(crate) write_atomic(dir, name, bytes)`: `<name>.tmp` created, written and synced, renamed over `<name>`; on failure the previous file is untouched and the tmp this call created is removed. The key order is the data-model "Call" field order, pinned by `meta_json_keys_follow_data_model_order_and_start_recording`. `transcript.*` and export writers (T-138, T-141) reuse `write_atomic`. Tests `write_meta_replaces_meta_json_and_leaves_no_tmp`, `write_meta_failure_keeps_previous_meta_json_and_no_tmp`, `active_call_write_meta_goes_through_the_atomic_writer`.
- **Don't:** write `meta.json` in place, copy the helper, or swallow a write error at its origin (a failed first write is `Storage(Meta)`).

### The list shows every call of every root, and hides nothing that is a call

- **Defect that produced it:** none yet.
- **What breaks if you violate it:** a damaged call disappears from the Calls window with its audio still on disk (FR-033), or a user folder, the probe file or `meta.json.tmp` shows as a call.
- **Where it is enforced:** `CallStore::list` reads the current root, then every earlier root, and never creates anything.
  - Only names in the `CallId` grammar are calls (D2); anything else is skipped.
  - A call folder whose `meta.json` is missing, unparsable, carries an unknown key (D4, `deny_unknown_fields`) or a value outside a closed set (`device_changes[].cause`, `pass_mode`, `results.<track>`, which is only `pending` until the final-pass write: contract `core-call.md` › Built T-125), is of another schema, names another id, has other tracks, or lacks a track file lists as `ListState::Damaged` with `meta: None`; so does `failed` without its `failure`.
  - A missing default current root lists as empty (it is created at the first start); any other root that cannot be read (missing chosen or earlier root, a file, a `read_dir` error) is one `unavailable` line, and the readable roots still list (D3).
  - Order: newest first by (start stamp, numeric clash suffix), so `-10` precedes `-2`; ties keep root order, current root first. `CallId`'s own `Ord` is string order and is not used.
  - Tests `list_current_and_earlier_newest_first_with_numeric_suffix_order`, `list_skips_names_that_are_not_a_call_id`, `missing_default_root_lists_empty_without_unavailable`, `missing_chosen_or_earlier_root_gives_one_unavailable_line`, `damaged_*`, `meta_json_with_unknown_key_lists_as_damaged`, `meta_json_outside_the_closed_sets_lists_as_damaged`.
- **Don't:** sort by `CallId`, list non-grammar names as damaged, hide a damaged folder, or let one unreadable root empty the list.

### A failed track step maps to the log's storage kind in one place; `Encode` is `write`

- **Defect that produced it:** none yet. `StorageKind` has no `encode` literal and `call_storage_failed kind=` has none in its grammar (D1).
- **What breaks if you violate it:** a new log literal needs `events.rs`, `diag/format.rs`, the `diag_format` tests and the contract to change together.
- **Where it is enforced:** `storage::storage_kind(StorageOp)`: `Write` and `Encode` to `Write`, `Flush` to `Flush`, `Sync` to `Sync`. Test `storage_kind_maps_every_op_encode_to_write`.
- **Don't:** map `Encode` at a call site, or add an `encode` literal without the whole set above.

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
