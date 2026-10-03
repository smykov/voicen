# Settings and keys (core types and pure rules)

**Code:** `crates/voicen-core/src/{secrets,hotkey_registrar,models}.rs`, `src/settings/{mod,url,validate,gate,hotkey}.rs`, `src/post_process/settings.rs` (T-003); `src/settings/{file,service}.rs`, `src/clock.rs`, `src/test_support.rs` (T-032) · **Tests that pin it:** `settings::tests::{missing_fields_take_defaults, settings_json_has_no_key_field}`, `secrets::tests::{key_slot_target_names, key_edits_debug_never_shows_a_key, key_presence_is_per_slot_and_serializes_only_booleans}` and the `compile_fail` doctest in `secrets.rs`, `settings::hotkey::tests::{rejects_non_canonical_text, parse_then_format_is_identity_for_every_accepted_string, rejects_invalid_strings}`; `settings::file::tests`, `settings::service::tests`, `clock::tests::utc_compact_table` and `tests/settings_apply.rs` (T-032; per invariant below)

Tasks: T-003, T-032. Contract: `specs/004-settings-and-first-run/{data-model.md,contracts/core-traits.md}`. Decisions: #17, #19, #21, #22, #23, #25, #26, #27, #28, #30, #31, #33.

## Invariants

### A key can never reach the settings file or a window

- **Defect that produced it:** none yet (found in planning, T-003). Specs 001–003 each planned their own key port (`CredentialStore`/`SecretString`, `SecretStore`, `SecretRef`) and their own Credential Manager impl beside spec 004's, so a key could travel through several unreviewed paths (P-010, P-011); decision #20.
- **What breaks if you violate it:** an API key in `settings.json`, a `SettingsView`, a log or a debug print (NFR-04, FR-20).
- **Where it is enforced:** `Settings` has no key field; `Secret` has no `Serialize` (compile-fail doctest), prints `***` and is zeroed on drop; windows get only `KeyPresence` booleans; one port `secrets::CredentialStore` over `KeySlot`; only `SettingsService` (T-032) writes or deletes, and it serializes only `Settings` (`settings::service::tests::save_never_writes_key_bytes`: a marker key in all three slots is absent from every file in the data directory, the `Saved` view and `view()`).
- **Don't:** add a key field to `Settings`, derive `Serialize` on `Secret`, or declare a second key trait in a feature.

### Defaults come from one function, and the serde default is container-level

- **Defect that produced it:** none yet (found in analysis, T-003). A field-level `#[serde(default)]` fills a missing field with the field type's `Default` (serde_derive 1.0.229, `de.rs`), so an older file without `auto_paste` loaded `false`, without `history.size` loaded `0` (invalid) and without `api.base_url` loaded `""`. Spec 001 also planned its own defaults and loader.
- **What breaks if you violate it:** a file written by an older version silently changes behaviour or fails validation; two defaults sources diverge (P-010).
- **Where it is enforced:** `settings::defaults(os_tag)` is the only builder; `Settings` and each nested struct use a container-level `#[serde(default = "…")]` that calls it; `missing_fields_take_defaults` covers `{}`, `{"schema_version":1}` and partial nested objects.
- **Don't:** put `#[serde(default)]` on a field, give a settings struct `#[derive(Default)]` for loading, or build defaults in a consumer.

### The consumer owns the shared trait

- **Defect that produced it:** none yet (found in `/speckit-analyze`, decisions #20, #21). Each spec planned its own port for the same concern, with dependency cycles T-003 → T-016 → T-004 → T-003 and T-003 → T-006 → T-003.
- **What breaks if you violate it:** parallel ports, parallel impls, and a task that cannot start before the task that waits on it.
- **Where it is enforced:** the settings core defines `CredentialStore`, `HotkeyRegistrar`, `DownloadedModels` and `post_process::settings`; 001, 002 and 003 implement or read them; fakes are public behind feature `test-fakes` (decisions #23 N4).
- **Don't:** declare another key, hotkey or model-list trait in a feature crate or module, or enable `test-fakes` in a release build.

### The hotkey grammar is strict and closed

- **Defect that produced it:** none yet (found in T-003 review of the contract, decisions #25, #26). The first contract added numpad operator keys that research R-6 does not contain.
- **What breaks if you violate it:** two spellings of one hotkey in the file, UI and logs (P-010), or a key the shell cannot map to a virtual-key code.
- **Where it is enforced:** `settings::hotkey::parse_hotkey` accepts the canonical text only (exact case, order `Ctrl`, `Alt`, `Shift`, `Win`, each once, key last) from the closed 82-key set; the rest is `hotkey.invalid`; `rejects_non_canonical_text`, `parse_then_format_is_identity_for_every_accepted_string`, `rejects_invalid_strings` (all in `settings::hotkey::tests`).
- **Don't:** accept lowercase or reordered input and normalize it, or add keys outside R-6 without changing the research and the spec.

### A base URL never carries userinfo

- **Defect that produced it:** none yet (found in T-003 review round 1, decision #27(2)). A base URL such as `https://user:secret@host/v1` is well-formed, so it passed `url.malformed` and would have been stored in `settings.json` and sent to logs and windows.
- **What breaks if you violate it:** key bytes in the settings file, a `SettingsView` or a log line, bypassing the credential store (NFR-04, FR-20). Limit: this closes `user:pass@` only. A query string is allowed (#27(2)) and stored as typed, so a key placed in a query would be stored in `settings.json`.
- **Where it is enforced:** `settings::url::check_base_url` returns `UrlError::Credentials` for a non-empty username or password, and `validate` maps it to `url.credentials`; a query string is allowed and stored as typed. "Never logged" is not enforced yet: T-008 owns it through the log allowlist (#30).
- **Don't:** accept userinfo in a base URL, strip it silently, or log a base URL with its query (until T-008 lands, no code path may log a base URL at all).

### One language list

- **Defect that produced it:** none yet (found in T-003 review round 1, decisions #27, #28). The contract said "a code from the Whisper language list" without naming it, so the validator, the UI picker and the engines could each carry their own list, and Whisper's `jw` is not ISO 639-1 (`jv`).
- **What breaks if you violate it:** the UI offers a language the core refuses, or an engine receives a code it rejects (P-010).
- **Where it is enforced:** `settings::WHISPER_ISO_639_1` is the only list (Whisper `LANGUAGES` minus `jw`, `haw`, `yue`); `validate` gives `language.unsupported` (`engine.speech_language`) for anything else, case-sensitive, for every engine; pinned by `settings::tests::whisper_list_is_unique_two_letter_lowercase` (entries unique, two lowercase ASCII letters) and `settings::validate::tests::speech_language_outside_whisper_list_refused`.
- **Don't:** copy the list into the UI or an engine crate, or accept a code by case-folding it.

### A save is all-or-nothing (I2)

- **Defect that produced it:** none yet (planned in T-003/T-032; spec 004 Clarification Q1, research R-3).
- **What breaks if you violate it:** a refused save leaves a key written or deleted, the file half-applied, or subscribers told about settings that were not stored.
- **Where it is enforced:** `SettingsService::save` makes no side effect before the service is known to be available and the normalized draft is valid; keys, then `write_atomic`, then the commit. A failure undoes the completed key steps in reverse and goes on after an undo error; the only leftover is reported as `FormError::PartiallyRestored { not_restored }` naming exactly the key fields that differ. Tests: `settings::service::tests::{refused_save_changes_nothing, file_write_failure_restores_keys, key_store_failure_refuses_with_key_store_failed, key_read_failure_refuses_before_any_write, undo_failure_reports_partially_restored}`.
- **Don't:** write a key or the file before `validate`, stop the undo at the first error, or return `Saved` before the file is written.

### `settings.json` changes only by an atomic rename (I3)

- **Defect that produced it:** none yet (planned in T-003/T-032; research R-2).
- **What breaks if you violate it:** a crash or a full disk mid-write leaves a truncated file, which the next start treats as unreadable and resets.
- **Where it is enforced:** `FsSettingsFile::write_atomic` writes and syncs `settings.json.tmp`, then renames it over `settings.json`; `read` deletes a leftover `.tmp` and never parses it. Tests: `settings::file::tests::{write_atomic_replaces_and_leaves_no_tmp, leftover_tmp_is_deleted_and_never_read, failed_write_keeps_previous_file}`.
- **Don't:** write `settings.json` in place, or fall back to the `.tmp` when `settings.json` is missing.

### The bytes of an existing settings file are never destroyed (I4)

- **Defect that produced it:** none yet (planned in T-003/T-032; spec 004 Clarification Q4, decision #19).
- **What breaks if you violate it:** a user's settings overwritten by defaults after a transient read error, or an earlier backup overwritten by a later one in the same second.
- **Where it is enforced:** an unparsable file leaves its name only by `move_aside`, which reserves `settings.json.bad-<UTC>` (then `-1` … `-9`) with `create_new` before renaming; a file that cannot be moved aside, or a read error other than not-found, makes the service `Unavailable` for life: no `write_atomic`, no `move_aside`, no credential call, every save refused with `notice.settings_unavailable`, and `view()` reports every key absent (#33(b)). Tests: `settings::file::tests::{read_error_other_than_not_found_is_err, move_aside_with_every_name_taken_fails_and_keeps_file}`, `settings::service::tests::{unreadable_file_reset, move_aside_never_overwrites_existing_backup, move_aside_failure_is_unavailable, read_io_error_is_unavailable, save_refused_while_unavailable, unavailable_view_reports_no_keys_without_credential_calls, defaults_write_failure_keeps_outcome}`.
- **Don't:** map an I/O read error to "no file", write defaults over a file that could not be moved, or rename into a backup name without reserving it.

### What is stored is the normalized draft, and a blank key never reaches the store (I5)

- **Defect that produced it:** none yet (T-003 review round 1 #9: normalize existed only inside `check_base_url`; decisions #30, #33(a)).
- **What breaks if you violate it:** the file, the snapshot and the UI disagree on a URL; a blank `Replace` deletes or overwrites a stored key; a pasted newline in a key causes a 401.
- **Where it is enforced:** `service::normalize` runs every base URL through `url::normalize_base_url` (the rule `check_base_url` uses) and trims every model, selected engine or not; the file, `snapshot()` and the `Saved` view carry the same value. An empty-after-trim `Replace` is `Untouched` after `validate` (which still sees it, so a blank API key with engine `api` is `key.required`); a non-empty one is stored trimmed. Tests: `settings::service::tests::{save_stores_normalized_values, blank_replace_is_untouched_in_every_slot, non_empty_replace_is_stored_trimmed}`, `tests/settings_apply.rs::save_round_trips_through_a_new_service`.
- **Don't:** normalize only the selected engine, trim the hotkey, prompt or microphone, or map a blank `Replace` to `Clear`.

### One snapshot per `Saved`, published after the file is written (I6)

- **Defect that produced it:** none yet (planned in T-003/T-032; research R-4, decision #22).
- **What breaks if you violate it:** a consumer applies settings that were refused, misses a save, or sees a running dictation's settings change under it.
- **Where it is enforced:** the commit swaps `RwLock<Arc<Settings>>` and sends the new `Arc` to every live `mpsc` sender under the save lock, pruning dropped receivers. Tests: `tests/settings_apply.rs::{subscriber_receives_snapshot_after_saved, refused_save_sends_nothing, earlier_snapshot_unchanged, dropped_receiver_does_not_fail_save}`.
- **Don't:** mutate the settings behind a handed-out `Arc`, publish before `write_atomic`, or fail a save because a receiver was dropped.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Field-level `#[serde(default)]` per field | fills with the type's `Default`, not `defaults()` | analysis T-003, hypothesis 4 |
| Each feature plans its own key/settings port | duplicate seams, cycles | decisions #20, #21 |
| Hand-written URL parser | R-8 requires the `url` crate | decisions #17 |
| `tokio::sync::watch` for `subscribe` | no tokio in core | decisions #22 |
| Plain `fs::rename` into the backup name | replaces an existing `to` on Unix and Windows, so it overwrites an earlier backup | T-032 investigation, hypothesis 5 |
| `chrono` for the backup suffix | allowed by #9, but one format string does not need a crate in the core build | T-032 investigation, design 5 |

## Open

- None.
