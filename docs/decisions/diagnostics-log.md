# Diagnostics log

**Code:** `crates/voicen-core/src/diag/` (`event.rs` the typed events and the input tables, `format.rs` the line format and the output tables, `log.rs` the writer, `observer.rs` `LogObserver`), `crates/voicen-core/src/clock.rs` (`LocalOffset`), `src-tauri/src/diag.rs` (`start`, the Windows UTC offset), `src-tauri/src/settings_ipc.rs` (`load_settings`, `settings_save`, the bridge warning), `src-tauri/src/lib.rs` (`run`, `assemble`, `build_app`), `.github/workflows/ci.yml` (windows job, install smoke) · **Tests that pin it:** `tests/diag_format.rs`, `tests/diag_log.rs`, `tests/diag_observer.rs`, `tests/diag_pipeline.rs`, `tests/settings_log.rs`, the `compile_fail` doctests in `diag/mod.rs`, `src-tauri/tests/diag.rs` and `settings_ipc::canary_key_never_in_data_dir_or_log` (Windows CI), the install smoke (start, `settings load outcome=first_run`, `autostart reconcile action=removed`)

Tasks: T-008 (narrowed by decision #64: the one-time notice and the tray "Open logs folder" are T-054), T-051 (the `CaptureFailed` line). Class `diagnostics-log` in `docs/failures.md` has no entry yet. Decisions: #9 (no new crate), #23, #30, #45, #64.

## Invariants

### Every byte under `logs/` comes from the one `Log`, through a closed output

- **Defect that produced it:** none in this project yet. The class is P-009's: keys reached plaintext logs on earlier projects through a library's request logging and through exception text. Here, `Engine::kind`, `SpeechDetector::name` and `FailureReason::code` are `&'static str` from open sources (`String::leak` makes one), and every logging facade takes formatted strings. Closest history: T-003 review 2 #2, a contract that overclaimed what was logged about a base URL's query.
- **What breaks if you violate it:** a transcript, a key, a URL query, a host or OS error text in a file the user attaches to a bug report (FR-20, NFR-04).
- **Where it is enforced:**
  - `LogEvent` fields are integers, closed enums of the crate and `BuildInfo` only; the `compile_fail` doctests in `diag/mod.rs` pin that a `&str` / `String` cannot be written and that a raw engine, detector or failure string cannot be put on a dictation line.
  - An outside string passes through `EngineTag::from_kind`, `DetectorTag::from_name` or `FailureTag::from_code`: exact match, anything else is `other`.
  - `format_line` writes integers, literals of its own tables (engine, detector, failure, `MicCause`, warning kind, load outcome), closed `as_str` / `code` matches of the crate (settings field ids and codes, `ReconcileAction`, `DeliveryResult`, `FormError::kind`) and `BuildInfo` (kept to its allowed characters). `diag_format.rs` checks every event of an exhaustive enumeration, with planted strings, against a hand-written grammar and closed value sets.
  - The redaction run (`diag_pipeline.rs`): the eight leak scenarios through a real `Pipeline` + `LogObserver` + `Log`, plus the settings lines; no key, query or transcript byte in any file.
  - `LogObserver` matches `DictationEvent`, `OutcomeCode`, `RecordingEnd` and `WarningCode` exhaustively, and `format_line`'s microphone table matches `MicCause`: a new variant does not compile until it has a mapping (T-051's `CaptureFailed` was added this way).
  - T-006 added three `WarningKind`s for shell failure points, each a literal in `format.rs`'s `warning_kind` (no text): `hotkey_thread_failed` (hidden window or thread could not start), `hotkey_register_failed` (`os_code` = the Win32 code of `RegisterHotKey`, e.g. 1409 when another process holds the combination; no code when the hotkey string does not parse), `dictation_start_failed` (`start_dictation`: session start failed, a managed session already exists, or the settings service is not managed).
  - The shell writes only typed lines (`Started`, the settings lines, `Warning{kind, os_code}`); there is no `eprintln!` and no `log` / `tracing` logger or subscriber (review; no guard yet). tauri-plugin-single-instance (T-052) brings the `tracing` facade, already in the lock; no subscriber is installed, so its events go nowhere.
- **Don't:** add a `String`, `&str`, path, `io::Error` or `FailureReason` field to `LogEvent`, or a `From<&str>` for it; echo an outside `&'static str`; `Debug`- or `Display`-format a value into a line; install `tauri-plugin-log`, `env_logger`, `tracing-subscriber` or any other logger; write any other file under `logs/` outside `diag::Log`.

### The log files never total more than 10 MiB, at any instant

- **Defect that produced it:** a spec gap found in the T-008 analysis. Spec 006 data-model "LogWriter" retained after a roll "while the total of `voicen*.log` > 10 MB". The active file is then near 0, so the rolled files kept up to 10 MB, and the active file grew to 2 MB on top: 12 MB on disk, against FR-20.
- **What breaks if you violate it:** FR-20's bound, on the user's disk.
- **Where it is enforced:** `log.rs`. The active `voicen.log` is rolled before a line would make it larger than `roll_bytes` (2 MiB) or at the local date change; retention at open and after each roll deletes rolled files older than `max_age` (7 days, by modification time), then the oldest rolled files while they total more than `total_bytes − roll_bytes` (8 MiB). `size_bound_holds_after_every_write_with_a_small_config` and `…_with_the_real_config` assert the bound after every write. A rolled name is never reused (`-1`, `-2`, …), so several rolls in one second lose no line (`fs::rename` replaces an existing file on every platform).
- **Don't:** retain against `total_bytes`; roll after writing the line; let retention match anything but `voicen-*.log` (crash files, the session marker and the installer note live in the same folder); judge age with a wrapping subtraction (a file from the future must look new).

### A log that cannot be written stops nothing and is reported once

- **Defect that produced it:** none; spec 006 FR-009 and T-008's Acceptance failure branch.
- **What breaks if you violate it:** a full disk, a file at the logs path or a denied folder crashes or blocks the app, or floods the user with notices.
- **Where it is enforced:** `Log::open` never fails and `Log::write` never panics or returns an error. A failed open, write or roll makes the log degraded and takes `on_unwritable` (called at most once per `Log`, outside the lock); a reopen is tried at most every `reopen_every` (60 s) after the last failure, and a successful one writes `LogsRecovered` first; dropped lines are not replayed. `NotADirectory` is decided from the path (the nearest existing part of the logs dir is not a directory), because `create_dir_all` over a file reports `AlreadyExists`. Tests: `a_file_at_the_logs_path_degrades_with_one_not_a_directory_callback`, `a_full_disk_degrades_with_one_disk_full_callback` (`/dev/full`), `a_degraded_log_reopens_at_most_every_interval_and_writes_logs_recovered`, `src-tauri/tests/diag.rs::an_unwritable_logs_dir_does_not_stop_the_start`. `PermissionDenied` cannot be produced in the core container (uid 0 ignores mode bits); its proof is T-054's ACL-denied folder on Windows.
- **Don't:** return a `Result` from `open` or `write`; `unwrap`/`expect` there; call the callback per failure; buffer lines while degraded; retry on every write.

### Only the primary instance opens the log, after tauri's `build()`

- **Defect that produced it:** none in the failure log (T-008 review round 1 #3, carried out by T-052). `run()` used to call `diag::start` before tauri's `build()`, which is where the single-instance plugin decides, so a second launch would write a `started` line into the primary's `voicen.log` and, on a roll, rename the file the primary still holds open. The primary's handle then follows the renamed file past `roll_bytes`, which breaks the 10 MiB bound above.
- **What breaks if you violate it:** a second process's lines in the primary's log, and a log over its size bound.
- **Where it is enforced:** `src-tauri/src/lib.rs` `assemble`: `build()` first (the plugin, registered first and only by `run()`, ends a second instance inside it with exit 0), then `parts()`, whose first step in `run()` is `diag::start`. Tests: `src-tauri/tests/single_instance.rs` (`startup_side_effects_run_only_after_the_plugins_setup`, `a_failing_plugin_setup_leaves_no_side_effect`) and the install smoke's second launch (exit 0, no `started pid=<second>` line). The start order as a whole: `docs/decisions/windows-shell.md`.
- **Don't:** open the log, or write any line, before `assemble`'s `build()`, or from the second-instance callback (`tray::on_second_instance` writes nothing).

### One dictation is one line, joined by `RecordingId`, never by event order

- **Defect that produced it:** none; `Pipeline::process` may run concurrently (T-011), so "the last `RecordingStarted`" is not the job's recording. `JobFinished` carries `recording` (T-008) and `Delivered` is joined by its job's `seq`.
- **What breaks if you violate it:** a line with another recording's timings.
- **Where it is enforced:** `observer.rs`; closing rules: `Delivered` for a text job, `JobFinished` for no-speech or failed, `RecordingEnded{TooShort}` for a discarded hold, `CaptureFailed` for a capture that failed at press (its only event) or at stop (after `RecordingStarted` / `RecordingEnded{Released}`; no job follows), written `outcome=capture_failed mic=<no_device|access_denied|busy|other>` at WARN (T-051); a pipeline `Warning` is its own line. At most 64 open records and 64 text jobs awaiting `Delivered`. Past that the oldest is dropped without a line: its timings are lost, and its late `Delivered` writes nothing. Tests: `interleaved_jobs_are_joined_by_recording_id`, `delivered_is_joined_to_its_job_by_seq`, `a_job_without_recording_events_still_gets_its_line`, `open_records_are_bounded_and_the_oldest_lose_their_timings_without_a_line`, `text_jobs_awaiting_delivery_are_bounded_and_the_oldest_get_no_line`, `a_capture_failure_at_press_is_one_warn_line_with_the_mic_literal`, `a_capture_failure_at_stop_closes_the_open_record_in_one_line`.
- **Don't:** join by event order; write one line per event (option B below); add a wildcard arm to the observer's matches.

## Windows notes

- The active file is held open by the one `Log` (read + append; std's shared read / write / delete) and closed before the roll's rename. The install smoke reads it while the app runs with `Select-String` / `Get-Content`, which open with shared read-write access; a reader that denies shared writes would fail to open it.
- A reader can block the roll. The rename needs every other open handle on `voicen.log` to share delete. A reader that shares read and write but not delete (.NET `FileShare.ReadWrite`: a tail with `Get-Content -Wait`, a log viewer) makes it fail with a sharing violation. The log then goes Degraded, `on_unwritable` gets `Other(Some(32))` (if it was not called before), and lines are dropped. A reopen every `reopen_every` (60 s) tries the roll again, and lines are written again once the reader closes. This follows from the code (T-008 review round 1 #4) and has not been observed on Windows. T-054 decides whether its notice treats this as "not writable" or whether the roll is retried without degrading (Open).
- The UTC offset is `SystemTimeToTzSpecificLocalTime` with the active time zone (`windows` feature `Win32_System_Time`, no new crate); UTC when the conversion fails. The log clamps it to ±14:00 and whole minutes.
- Shell code is type-checked on Linux for `x86_64-pc-windows-gnu` by `make check-shell-windows` (T-056). It is first built, linked and run on the Windows job (decisions #5, #66; `docs/decisions/ci-toolchain.md`).

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| A logging crate (`log` / `tracing` with `tracing-appender`, `tauri-plugin-log`) | no total-size cap; a facade takes formatted strings, the P-009 leak class | T-008 Investigation H1, spec 006 R1 |
| One line per event, correlated by `rec=` / `seq=` (option B) | no state, but breaks FR-20's "a line with the engine and the three timings"; a requirement change for the owner | T-008 |
| A pipeline summary event `DictationFinished` (option C) | moves press → first frame into `FinishedRecording` (T-042 controller, T-001 contract); a too-short hold still needs its own line | T-008 |
| Retention "while the total of `voicen*.log` > 10 MB" after a roll | 12 MB on disk (spec gap above) | T-008 Investigation |
| `chrono::Local` for the offset | a new direct dependency for one call the existing `windows` crate makes | T-008 |

## Open

- T-054: the one-time `notice.logs_unwritable` from `on_unwritable`, the tray "Open logs folder", the ACL-denied folder proof on Windows. T-054 also decides whether a sharing violation on the roll's rename (Windows notes) is a "not writable" notice reason, or is retried on the next write without degrading.
- Proposed T3 guard (T-008 analysis, no task yet): `make check` fails when `Cargo.lock` names a logger backend.
- The failed `.part` removal inside `Downloader` is not logged (core has no log port there; release-2 follow-up, T-008 Q5).
