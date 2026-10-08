# Settings and keys (core types and pure rules)

**Code:** `crates/voicen-core/src/{secrets,hotkey_registrar,models}.rs`, `src/settings/{mod,url,validate,gate,hotkey}.rs`, `src/post_process/settings.rs` (T-003); `src/settings/{file,service}.rs`, `src/clock.rs`, `src/test_support.rs` (T-032); `src-tauri/src/{credentials,paths,locale,settings_ipc,lib}.rs` (T-030); `crates/voicen-core/src/autostart.rs`, `src-tauri/src/autostart.rs` (T-014) · **Tests that pin it:** `settings::tests::{missing_fields_take_defaults, settings_json_has_no_key_field}`, `secrets::tests::{key_slot_target_names, every_slot_target_starts_with_prefix, purge_*, key_edits_debug_never_shows_a_key, key_presence_is_per_slot_and_serializes_only_booleans}` and the `compile_fail` doctest in `secrets.rs`, `settings::hotkey::tests::{rejects_non_canonical_text, parse_then_format_is_identity_for_every_accepted_string, rejects_invalid_strings}`; `settings::file::tests`, `settings::service::tests`, `clock::tests::utc_compact_table` and `tests/settings_apply.rs` (T-032; per invariant below); the wire-form tests `settings::service::tests::save_outcome_wire_form`, `secrets::tests::{key_edits_deserialize_wire_form, key_edit_deserialize_error_never_echoes_input}`, `i18n::tests::message_id_serializes_as_id` and, on Windows CI, `src-tauri/tests/{credentials,settings_ipc}.rs` (T-030) and `src-tauri/tests/purge_credentials.rs` (T-061); `autostart::tests`, the autostart and `reconcile_autostart_*` cases of `settings::service::tests`, `tests/fakes.rs::fake_autostart_records_calls_and_fails_per_value` and, on Windows CI, `src-tauri/tests/autostart.rs` plus the install-smoke Run-value checks in `.github/workflows/ci.yml` (T-014); `settings::service::tests::{settings_view_wire_form_carries_unavailable, e2e_settings_wire_fixture_matches_core}` and `settings::tests::error_codes_match_data_model` over the one hand-kept `ErrorCode::ALL` (T-004; the settings window's own invariants are in `docs/decisions/settings-ui.md`); `settings::url::tests::is_insecure_remote_table`, the T-015 `save_*warn*` cases of `settings::service::tests`, `save_outcome_wire_form` and `e2e_settings_wire_fixture_saved_insecure_outcome_matches_core` (T-015)

Tasks: T-003, T-032, T-030, T-014, T-004 (`SettingsView.unavailable`, the e2e wire fixture), T-015 (insecure-endpoint warning). Contract: `specs/004-settings-and-first-run/{data-model.md,contracts/core-traits.md}`. Decisions: #5, #17, #19, #21, #22, #23, #25, #26, #27, #28, #30, #31, #33, #34, #35, #52.

## Invariants

### A key can never reach the settings file or a window

- **Defect that produced it:** none yet (found in planning, T-003). Specs 001–003 each planned their own key port (`CredentialStore`/`SecretString`, `SecretStore`, `SecretRef`) and their own Credential Manager impl beside spec 004's, so a key could travel through several unreviewed paths (P-010, P-011); decision #20.
- **What breaks if you violate it:** an API key in `settings.json`, a `SettingsView`, a log or a debug print (NFR-04, FR-20).
- **Where it is enforced:** `Settings` has no key field; `Secret` has no `Serialize` (compile-fail doctest), prints `***` and is zeroed on drop; windows get only `KeyPresence` booleans; one port `secrets::CredentialStore` over `KeySlot`; only `SettingsService` (T-032) writes or deletes, and it serializes only `Settings` (`settings::service::tests::save_never_writes_key_bytes`: a marker key in all three slots is absent from every file in the data directory, the `Saved` view and `view()`).
- **Shell part (J1, T-030):** a key moves only UI → `settings_save` → `SettingsService` → `CredentialStore`. Tauri puts a command argument's serde error into the rejection the window receives, and serde's own messages quote the input (``unknown variant `sk-...` ``; serde_json raises `invalid type: string "..."` itself, whatever the visitor), so `Secret`, `KeyEdit`, `KeyEdits` and `SaveRequest` deserialize through private derived helpers and replace every error with a fixed text (`key_edit_deserialize_error_never_echoes_input`). `credentials::WinCredentialStore` is the first of two callers of Credential Manager; the release store uses exactly `KeySlot::target_name()`, and slot targets are built from `voicen_core::secrets::CREDENTIAL_TARGET_PREFIX` (`"Voicen/"`). The second caller is `src-tauri/src/win/purge.rs` (T-061: `CredEnumerateW`/`CredDeleteW`/`CredFree`): it only enumerates and deletes entries under store prefix + `CREDENTIAL_TARGET_PREFIX`, wipes blobs in place and never copies them out, and is reached only from `main()` via `--purge-credentials` before `run()`; and a credential failure is returned, never stored elsewhere (`src-tauri/tests/credentials.rs`, `settings_ipc.rs::{key_store_write_refused_is_key_store_failed_and_nothing_written, canary_key_never_in_data_dir_or_log}`, Windows CI).
- **Don't:** add a key field to `Settings`, derive `Serialize` on `Secret`, derive `Deserialize` on a key-bearing type (or format the input into its error), or declare a second key trait in a feature.

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

### A saved hotkey is registered in two steps, and only a changed one

- **Defect that produced it:** none yet (spec 004 R-3, T-055). Registering the new hotkey before the file is written leaves a working hotkey the file does not hold when a later step refuses; releasing the old one first leaves no hotkey when the new one is taken.
- **What breaks if you violate it:** a refused save that changes which keys dictate, or a taken combination that silently disables dictation.
- **Where it is enforced:** `SettingsService::save` calls `HotkeyRegistrar::prepare` after validation and before autostart, only when the hotkey text differs from the snapshot in force (a taken one → `Refused [recording.hotkey: hotkey.unavailable]`, nothing touched); every later refusal calls `abort` after its undo; `commit` runs after `write_atomic` and before the snapshot swap and publish. Tests: `taken_hotkey_refuses_with_hotkey_unavailable_and_changes_nothing`, `every_later_refusal_aborts_the_prepared_hotkey`, `a_saved_hotkey_is_committed_after_the_file_and_before_the_swap_and_publish` (`settings::service` tests).
- **Don't:** register or release a hotkey outside `prepare` / `commit` / `abort`, or prepare an unchanged hotkey.

### The hotkey grammar is strict and closed

- **Defect that produced it:** none yet (found in T-003 review of the contract, decisions #25, #26). The first contract added numpad operator keys that research R-6 does not contain.
- **What breaks if you violate it:** two spellings of one hotkey in the file, UI and logs (P-010), or a key the shell cannot map to a virtual-key code.
- **Where it is enforced:** `settings::hotkey::parse_hotkey` accepts the canonical text only (exact case, order `Ctrl`, `Alt`, `Shift`, `Win`, each once, key last) from the closed 82-key set; the rest is `hotkey.invalid`; `rejects_non_canonical_text`, `parse_then_format_is_identity_for_every_accepted_string`, `rejects_invalid_strings` (all in `settings::hotkey::tests`).
- **Don't:** accept lowercase or reordered input and normalize it, or add keys outside R-6 without changing the research and the spec.

### A base URL never carries userinfo

- **Defect that produced it:** none yet (found in T-003 review round 1, decision #27(2)). A base URL such as `https://user:secret@host/v1` is well-formed, so it passed `url.malformed` and would have been stored in `settings.json` and sent to logs and windows.
- **What breaks if you violate it:** key bytes in the settings file, a `SettingsView` or a log line, bypassing the credential store (NFR-04, FR-20). Limit: this closes `user:pass@` only. A query string is allowed (#27(2)) and stored as typed, so a key placed in a query would be stored in `settings.json`.
- **Where it is enforced:** `settings::url::check_base_url` returns `UrlError::Credentials` for a non-empty username or password, and `validate` maps it to `url.credentials`; a query string is allowed and stored as typed. "Never logged" is the log's typed allowlist (T-008, #30): no `LogEvent` has a field for a URL, host or settings value; the settings lines carry the outcome, field ids and codes only (`diag::LogEvent::settings_load` / `settings_save`, pinned by `tests/settings_log.rs` and the redaction run `tests/diag_pipeline.rs`; docs/decisions/diagnostics-log.md).
- **Don't:** accept userinfo in a base URL, strip it silently, or log a base URL in any form (no `LogEvent` field may hold one; T-008).

### The request URL is decided only by `transcription_url`; the stored URL is kept as typed (T-072)

- **Defect that produced it:** owner log 2026-10-07 (`dictation rec=4 engine=local_server outcome=failed failure=server_error http_status=404`, decision #96). The join appended `audio/transcriptions` unconditionally, so a full endpoint URL copied from another client was requested at `…/audio/transcriptions/audio/transcriptions`.
- **What breaks if you violate it:** a pasted full endpoint gets a doubled path and a 404; a second URL rule (in the UI, the shell or on save) disagrees with the one the client connects with (P-010, F-003 class); rewriting the stored value on save makes the file, the snapshot and the UI differ from what the user pasted.
- **Where it is enforced:** `engine::openai::transcription_url` (the only caller is `OpenAiCompatibleEngine::transcribe`; api, local_server and Test connection all reach it through `engine::engine_for`): drop one empty trailing segment; if the last two segments equal `audio`, `transcriptions` ASCII case-insensitively, keep the path as typed, otherwise append them; the query is always kept. `settings::url::check_base_url` / `normalize_base_url` are unchanged (trim, strip one `/`). Tests: `engine::openai::tests::{join_full_endpoint_kept, join_near_misses_append}`, `tests/openai_client.rs::full_endpoint_url_is_requested_as_is`, `tests/local_server.rs::{full_endpoint_local_server_url_is_requested_as_is, wrong_path_reports_server_404}`.
- **Don't:** strip `/audio/transcriptions` from the stored value on save, "repair" a doubled path, match on the path text (`ends_with("audio/transcriptions")` accepts `/xaudio/transcriptions`), or build the request URL anywhere else.

### One language list

- **Defect that produced it:** none yet (found in T-003 review round 1, decisions #27, #28). The contract said "a code from the Whisper language list" without naming it, so the validator, the UI picker and the engines could each carry their own list, and Whisper's `jw` is not ISO 639-1 (`jv`).
- **What breaks if you violate it:** the UI offers a language the core refuses, or an engine receives a code it rejects (P-010).
- **Where it is enforced:** `settings::WHISPER_ISO_639_1` is the only list (Whisper `LANGUAGES` minus `jw`, `haw`, `yue`); `validate` gives `language.unsupported` (`engine.speech_language`) for anything else, case-sensitive, for every engine; pinned by `settings::tests::whisper_list_is_unique_two_letter_lowercase` (entries unique, two lowercase ASCII letters) and `settings::validate::tests::speech_language_outside_whisper_list_refused`.
- **Don't:** copy the list into the UI or an engine crate, or accept a code by case-folding it.

### A save is all-or-nothing (I2)

- **Defect that produced it:** none yet (planned in T-003/T-032; spec 004 Clarification Q1, research R-3).
- **What breaks if you violate it:** a refused save leaves a key written or deleted, the file half-applied, or subscribers told about settings that were not stored.
- **Where it is enforced:** `SettingsService::save` makes no side effect before the service is known to be available and the normalized draft is valid; then the autostart step (only when `start_with_windows` differs from the snapshot; T-014), the keys, `write_atomic`, the commit. A failure undoes the completed steps in reverse — keys in reverse slot order, then autostart — and goes on after an undo error; the only leftover is reported as `FormError::PartiallyRestored { not_restored }` naming exactly the fields that differ, in step order (`general.start_with_windows`, then key fields). Tests: `settings::service::tests::{refused_save_changes_nothing, file_write_failure_restores_keys, key_store_failure_refuses_with_key_store_failed, key_read_failure_refuses_before_any_write, undo_failure_reports_partially_restored, autostart_failure_refuses_and_changes_nothing, autostart_step_runs_before_the_keys_and_the_file, key_failure_restores_autostart, file_write_failure_restores_keys_then_autostart, autostart_undo_failure_reports_partially_restored}`.
- **Don't:** write a key or the file before `validate`, stop the undo at the first error, or return `Saved` before the file is written.

### The Run value follows `start_with_windows`, through one writer (T-014)

- **Defect that produced it:** none yet (planned in T-014; spec 004 FR-019, research R-3 step 3, R-5; decision #35).
- **What breaks if you violate it:** the app starts at logon although the user turned it off (or the reverse); a refused save leaves the Run value changed; a user whose `settings.json` merely could not be read loses their logon entry; an uninstall leaves a Run value pointing at a removed exe.
- **Where it is enforced:**
  - The `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `Voicen` is written or removed only by `src-tauri` `autostart::WinAutostart` (`RUN_VALUE_NAME`, the one constant; data `REG_SZ` `"<current exe>" --autostart`; an absent value on delete is `Ok`; errors carry only the Win32 code, never the exe path). Only `SettingsService` calls the `Autostart` port, under the save lock.
  - `save`: the step runs only when the value changes against the snapshot, after `validate` and before the keys; a failure is `Refused [general.start_with_windows: autostart.failed]` with nothing else touched; when the value changes, `Saved` is returned only after `set(new)` succeeded; an unchanged value is not re-applied by save, the Run value is brought in line by `reconcile_autostart` at start (a failure there is logged by T-008) (I2 above).
  - `reconcile_autostart`, called once by `settings_ipc::load_settings` right after `load_or_init` (J4): `Unavailable` → no call; on → `set(true)` (refreshes a stale path); off → removed only if present; any error → `Failed`; never touches the file or the credential store. `FirstRun` / `Reset` remove a leftover.
  - 006's uninstaller hook deletes the same `Voicen` value unconditionally (spec 006 FR-023, contracts/installer-ci.md).
  - Tests: `settings::service::tests::{autostart_applied_only_when_changed, reconcile_autostart_*}`, `save_refused_while_unavailable`; Windows CI `src-tauri/tests/autostart.rs` and the install smoke (a leftover value removed at first launch; on → written with the installed exe; off → removed).
- **Known limit (R-5):** a value disabled in Task Manager (`StartupApproved\Run`) is not overridden.
- **Don't:** touch the Run key outside `WinAutostart`, call `Autostart` outside `SettingsService`, reconcile while `Unavailable`, run the step on every save (an unrelated save would fail while the Run key is unwritable), or put the exe path in an error or log.

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

### Request timeouts: one bounds table, derived per job, clamped on conversion (T-073, decisions #97, #99)

- **Defect that produced it:** none (feature, owner request #97). The FR-24 durations were compile-time values in one `Timeouts` held by `Pipeline` for its lifetime; making them settings opened the risk of three copies of the numbers (defaults, save rule, UI hints) and of a stale value reaching a running job.
- **What breaks if you violate it:** the defaults, the save-time range and the clamp disagree (P-010); a hand-edited file with `0` or `4294967295` seconds reaches a request (a zero limit fails every job, a huge one hangs it); a save during a recording changes that job's limits (breaks the one-snapshot-per-job rule of `docs/decisions/dictation-session.md`).
- **Where it is enforced:** `timeouts::TimeoutRole::bounds` is the only table (default, min, max in whole seconds per role); `settings::defaults` reads it, `settings::validate` refuses a value outside it for every engine (`timeout.range` on `timeouts.<role>`), and `Timeouts::from_settings` clamps into it. `Pipeline::process` calls `from_settings` on the job's settings snapshot, once per job; `Pipeline::with_timeouts` is a test-only override. `Timeouts::default()` is `from_settings` of the default settings. Model downloads keep the fixed `download_no_data` (decision #99 Q1). Pinned by `timeouts::tests::{default_is_from_settings_defaults, from_settings_clamps}`, `settings::validate::tests::timeouts_out_of_range_are_refused_for_every_engine` and `pipeline::tests::request_carries_the_job_settings_timeouts`.
- **Don't:** write a number of seconds outside `TimeoutRole::bounds` (UI hint, test default, engine), read a timeout from the live settings instead of the job snapshot, or make the pipeline subscribe to settings changes to swap its timeouts.

### An insecure endpoint is decided once, in core, at save, and never blocks (T-015)

- **Defect that produced it:** none yet (found in T-015 analysis, decision #52). The task's Acceptance asked for a TS rule in the UI: a second copy of the loopback rule over another parser (WHATWG `URL` in the webview vs the `url` crate the HTTP client connects with), so the warning and the real connection could disagree (P-010; F-003 class).
- **What breaks if you violate it:** the user is told a remote endpoint is local (or the reverse) because the rule read the text or another parser's host; a warning turned into a refusal blocks a legitimate LAN setup; a warning on a URL not in use is noise.
- **Where it is enforced:** `settings::url::is_insecure_remote(&NormalizedUrl)` re-parses with the `url` crate and is true exactly when the scheme is `http` and the host is not loopback: `Domain("localhost")` (the crate lower-cases it), `Ipv4` in 127.0.0.0/8 (shorthand such as `127.1`, `0x7f.1`, `2130706433` is decoded by the crate), `Ipv6 == ::1`. Everything else over `http` warns, including `[::ffff:127.0.0.1]`, `0.0.0.0`, `[::]`, `localhost.` and `foo.localhost` (the spec's closed list; fail-safe since a warning never blocks). `service::save_warnings(&Settings)` lists the URLs in use (the selected engine's `api`/`local_server` base URL; `post_processing.base_url` while `post_processing.enabled`), classifies only those that pass `check_base_url` (no unwrap; a malformed one gets no warning), and the save's commit step (8) puts the result in `Saved.warnings`; a `Refused` never carries one. `Warning` serializes `{field, code, message}` with `message = WarningCode::message_id()`. Tests: `settings::url::tests::is_insecure_remote_table`, `settings::service::tests::{save_warns_on_http_remote_api_endpoint_and_still_saves, save_warns_on_http_remote_local_server_endpoint, save_does_not_warn_on_loopback_local_server, save_ignores_http_remote_url_of_an_engine_not_in_use, save_warns_on_http_remote_post_processing_endpoint_only_while_enabled, save_lists_every_insecure_url_in_use, save_gives_no_warning_for_an_unchecked_post_processing_url, refused_save_carries_no_warning_and_writes_nothing, save_outcome_wire_form}`.
- **Don't:** classify a URL by its text (`starts_with("http://")`, `contains("localhost")`), add a loopback rule in the UI or the shell, warn on a URL that is not in use, or let a warning refuse or change a save.

### One data directory, one service construction, one change event (J2–J4, T-030)

- **Defect that produced it:** none yet (planned in T-030). Before it, `log_dir()` was the only `%LOCALAPPDATA%` resolver, nothing built the service in the app, and a second consumer of the data dir would have added a second resolver (P-010).
- **What breaks if you violate it:** logs and settings in different folders; a release app wired differently from the tested one; a window that misses a save made outside `settings_save` (FR-28's "deleted model → engine none"), or gets two events for one save (P-011).
- **Where it is enforced:**
  - J2: `src-tauri` `paths::data_dir()` is the only resolver (`%LOCALAPPDATA%\Voicen`, temp-dir fallback); `log_dir()` derives from it, and `settings.json` is reached only through `FsSettingsFile::new(data_dir)` inside `SettingsService`. Test: `data_dir_is_localappdata_voicen_and_logs_live_under_it` (Windows CI).
  - J3: `settings://changed` is emitted only by `settings_ipc::spawn_change_bridge`, a `subscribe()` consumer; core's I6 gives exactly one message per `Saved` and none per `Refused`. Test: `saved_emits_changed_once_refused_emits_nothing` (Windows CI, includes a save that does not come through IPC).
  - J4: `run()` and the shell tests build the service through `settings_ipc::load_settings` and the app through one assembly body, `assemble(builder, context, parts)` (`src-tauri/src/lib.rs`; the tests through `build_app(builder, context, service, local_models, log)`, which is `assemble` with those parts), which registers the commands, calls `build()`, only then makes the parts (in `run()`: the log, `LocalModels::open`, `load_settings`; T-052, `docs/decisions/windows-shell.md`), manages the service and starts `spawn_change_bridge` on the built app's handle (tauri 2.12.1 runs `.setup()` only from `run` / `run_iteration`, never from `build()`, so the bridge is not in `.setup()`); `run()` only calls `.run(…)` on the result. They differ only in the builder and context (`tauri::Builder::default()` with the single-instance plugin + `generate_context!()` vs `mock_builder()` + `mock_context(noop_assets())`), the credential store, the autostart entry and the data dir; `load_settings` reconciles the autostart entry right after the load (T-014); the tests never start the bridge themselves, so `saved_emits_changed_once_refused_emits_nothing` covers the release wiring. `test-fakes` and `tauri/test` are dev-dependencies only: `cargo tree -p voicen --target x86_64-pc-windows-msvc -e normal,features | grep -c 'test-fakes\|"test"'` is 0.
- **Don't:** resolve `%LOCALAPPDATA%` anywhere else, open `settings.json` outside `SettingsService`, emit `settings://changed` from a command, or construct `SettingsDeps` a second time in the shell.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Field-level `#[serde(default)]` per field | fills with the type's `Default`, not `defaults()` | analysis T-003, hypothesis 4 |
| Each feature plans its own key/settings port | duplicate seams, cycles | decisions #20, #21 |
| Hand-written URL parser | R-8 requires the `url` crate | decisions #17 |
| A TS loopback rule in the UI for the insecure-endpoint warning | a second rule over another parser (WHATWG `URL`), can disagree with the `url` crate the client connects with | decision #52, T-015 investigation |
| Strip a trailing `/audio/transcriptions` from the URL on save | rewrites what the user typed; a second normalization rule in `settings/service.rs` | decision #96, T-072 investigation, option B |
| `tokio::sync::watch` for `subscribe` | no tokio in core | decisions #22 |
| Plain `fs::rename` into the backup name | replaces an existing `to` on Unix and Windows, so it overwrites an earlier backup | T-032 investigation, hypothesis 5 |
| `chrono` for the backup suffix | allowed by #9, but one format string does not need a crate in the core build | T-032 investigation, design 5 |
| Wire DTOs in the shell | the contract would be tested only on Windows and every field/code string spelled twice (P-010) | T-030 investigation, option B |
| Emit `settings://changed` inside `settings_save` | saves from other callers (model deletion, later tray toggles) would never reach the windows (P-011) | T-030 investigation, option C |

## Open

- None.
