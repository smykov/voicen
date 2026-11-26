# Transcript history

**Code:** `crates/voicen-core/src/history/` (`mod.rs` `History`, `follow_settings`; `recorder.rs` `HistoryRecorder`; `store.rs` `FileHistoryStore`; `model.rs`; `error.rs`), `crates/voicen-core/src/pipeline.rs` (`TranscriptSink`, `PipelineDeps::transcript_sink`, the call in `Pipeline::release`), `src-tauri/src/history.rs` (`start_history`, `managed_sink`, `transcript_sink`, `open_history`, the `history_*` commands, `HistoryWire`), `src/lib/history/`, `src/routes/history/+page.svelte` · **Tests that pin it:** `crates/voicen-core/tests/history.rs` (C1-C11), `history_pipeline.rs`, `history_follower.rs`, `diag_format.rs` (`history_failed`), `history_start_log.rs` (`history_start_failed`), `src-tauri/tests/app_acl.rs`, `history_ipc.rs`, `history_wiring.rs` (Windows CI), `e2e/history.spec.ts`, `src/lib/history/*.test.ts`

Task: T-022 (spec `specs/005-transcript-history/`, FR-16, NFR-06). Class `history`; no `docs/failures.md` entry of this class yet, so the invariants below come from the T-022 analysis and the failure-branch mutant catalog (`docs/tasks/T-022.mutants/`), not from a past defect.

## Invariants

### A text enters the history only at `Pipeline::release`'s delivered branch, through the required `TranscriptSink`

- **Defect that produced it:** none; a design rule (T-022 analysis). Recording through `PipelineObserver` / `DictationEvent` would put the text next to the log (`Delivered` carries no text on purpose, FR-20), and recording from the shell's `JobReport` would put text into a type that flows to the controller.
- **The rule:** `Pipeline::release` calls `deps.transcript_sink.delivered(&text, engine)` once per job whose text was delivered (`Pasted`, `CopiedOnly`, `CopyManual`), after `delivery::deliver` returned `Ok`, with the text as delivered (post-processed, or raw when the stage was off or skipped) and `EngineLabel::of(job.settings)`. A failed job, `NoSpeech`, `ClipboardUnavailable` and a cancelled recording never reach it. A successful retry records once. `PipelineDeps::transcript_sink` is a required field (a defaulted no-op would let a wiring forget it silently, P-013); tests use `pipeline::DiscardTranscripts` (feature `test-fakes`). The engine enum is the settings' (`EngineLabel::of`, P-010).
- **What breaks if you violate it:** a second recording path stores text the user never saw delivered, or the sink placed before `deliver` records a text whose clipboard write failed.
- **Where it is enforced:** `history_pipeline.rs` (one entry per delivered job, none for the other outcomes; `clipboard_unavailable_records_nothing`); mutants 01 and 02.
- **Don't:** call the sink from anywhere else, route the text through an observer or a log line, or record call-mode transcripts (`calls_copy_markdown` copies a stored call, not a dictation).

### The history file is written or deleted only through `History<S>`, and off or Clear leaves no `history.json*`

- **The rule:** `History<S: HistoryStore>` is the only owner of `history.json` / `history.json.tmp` (`FileHistoryStore`, atomic replace). While disabled it makes no `save` call; the only disk operation then is a delete. `apply_settings(false, _)`, `clear()` and `open(enabled = false)` end with neither file in the directory (a failed delete falls back to saving an empty document; if both fail the notice is `delete_failed` and the next operation retries, also while off). Blank text is never recorded; ids rise across Clear and off/on; order is insertion order, not clock order. A save failure keeps the entry in memory with notice `save_failed`.
- **What breaks if you violate it:** transcripts stay on disk after the user turned history off or cleared it (NFR-06, SC-003).
- **Where it is enforced:** `tests/history.rs` C1-C11, `history_follower.rs` (through the real `SettingsService::save` and `subscribe()`, P-013); mutants 03-13 and 15.
- **Don't:** write the file from the shell, add a second `FileHistoryStore` user, or add an HTTP dependency to the module (C12, dependency review).

### No history text appears in an error, a log line, an event or an IPC error

- **The rule:** `HistoryError` holds only `op: HistoryOp` and `kind: HistoryErrorKind` (an `io::ErrorKind` or `Corrupt`); the type has no room for text. A disk error reaches the log only as `history_failed op=<load|save|delete> kind=<corrupt|not_found|permission_denied|not_a_directory|read_only|storage_full|other>` (WARN; `LogEvent::HistoryFailed`, closed enums, no path or OS text; `diag/format.rs`). `history_copy` fails only with `{code: "not_found" | "clipboard_unavailable"}`, `settings_open` only with `{code: "unknown_tab"}`; the UI's copy alert holds neither the text nor the code.
- **Where it is enforced:** `c8_display_and_debug_of_every_produced_error_hold_no_entry_text`, `diag_format.rs::history_failed_lines_carry_the_op_and_a_closed_kind_only`, the e2e copy-rejection tests; mutants 14 and 25.

## Design notes

### The core and the file are snake_case, the IPC wire is camelCase

The stored document (`history.json`, `schema: 1`) and core's `HistoryView` serialize with the contract's names (`ready_at_ms`, `engine.kind` = `api` | `local` | `server`, notice `save_failed` | `delete_failed`). The IPC contract is camelCase (`readyAtMs`), so the shell converts: `history::HistoryWire` / `HistoryEntryWire` (`#[serde(rename_all = "camelCase")]`, `From<HistoryView>`) are what `history_get`, `history_clear` and `history://changed` carry. Nothing else renames fields; a new field is added in both places.

### The shell's pipeline sink is a required choice of the dictation's caller

`start_history(app, dir, clipboard)` builds the one `HistoryRecorder<FileHistoryStore>` and manages it as the private `HistoryPart` (with the clipboard writer of the copy). The pipeline's sink is the required field `dictation::DictationPorts::history: HistorySink` (no `Default`, no `Option`): `HistorySink::Managed` takes the managed recorder (`history::managed_sink`); on an app with no history part (never started, started after the dictation, or failed) `start_dictation_with` is `Err` of kind `NotFound`, writes one `warning kind=dictation_start_failed` line and starts nothing (the sink is resolved before the pending-audio cleanup, the session and the hotkey thread), never a keep-nothing fallback. `HistorySink::Discard` is the explicit keep-nothing sink (`NoHistory`): the shell tests that look at no history, and `run()` after a failed start. `run()` calls `start_history` before the dictation and passes `Managed` when it returned `Ok`, `Discard` when it failed (dictation goes on, NFR-07). Every `Err` of `start_history` after the managed `Log` is found writes one `warning kind=history_start_failed [os_code=<n>]` line (`WarningKind::HistoryStartFailed`; the OS code from `io::Error::raw_os_error`, none for `AlreadyExists`; no path, no error text). The commands answer an empty view / `not_found` on an app without a history part. `history::transcript_sink(app)` (the managed recorder, or the keep-nothing sink) is for callers outside the dictation wiring; the dictation does not use it.

- **Chosen over:** finding the recorder in app state inside `start_dictation_with` with a keep-nothing fallback (round 1): a `run()` that stopped calling `start_history`, called it after the dictation, or whose call failed compiled, passed CI and recorded nothing, with no line in the log (T-022 review 1 #1, #3; P-013).
- **Cost:** every `DictationPorts { .. }` constructor names a sink (the shell tests pass `Discard`). `run()`'s own order and its `Managed`/`Discard` choice are not callable in a test; `history_wiring.rs::managed_history_without_a_started_history_part_refuses_the_dictation_start` makes a missing or late `start_history` a refused start under `Managed`, and `history_ipc.rs::a_failed_start_history_writes_one_closed_tag_line_and_leaves_the_first_part_working` pins the failure line (both Windows CI only).

### The history write is synchronous on the pipeline thread

`TranscriptSink::delivered` runs inside `Pipeline::release`, on the pipeline's thread, after `delivery::deliver` returned and the `Delivered` event was emitted: the recorder updates memory and saves `history.json` (and emits `history://changed`) before the job ends. The paste has already happened, so the save never delays the text reaching the field; a slow disk delays only the end of the job (the next job's start). No separate writer thread exists.

### Recorder disk errors are logged, never raised

`HistoryRecorder::new(history, clock, on_change)` announces every changed view (the shell emits `history://changed` to the `history` window only; nothing hears it while the window is closed); `.with_errors(on_error)` sets the error callback (without it errors are dropped), which the shell points at `Log::write(LogEvent::history_failed(error))`. Both callbacks run under the recorder's lock, so views arrive in order, and must not call back into the recorder. `TranscriptSink::delivered` returns nothing: a disk failure never fails or delays a dictation (NFR-07, SC-004); the user sees it as the window's notice. `follow_settings(recorder, rx)` blocks on `SettingsService::subscribe()`'s channel until it closes; the shell runs it on the "history-settings" thread, subscribing before it reads the snapshot so a save in between is not missed.

### The window follows the Calls window's opener pattern

At most one window labelled `history` at `/history`, built only on the "history-window" thread (`open_history` posts and returns; a second open unminimizes, shows and focuses; closing destroys). "Open settings" in the off state invokes `settings_open { tab: "history" }` (`settings_window::settings_open`, posts `request(OpenTarget::Tab(tab, None))`, the same path as `ShellRequests::open_settings`). ACL: `docs/decisions/ipc-acl.md`.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Record from the shell's `JobReport` | the report carries no text; adding it puts text into a type that reaches the controller and the log, and makes the record path Windows-CI-only | T-022 Investigation option B |
| Record through `PipelineObserver` | the observer feeds `LogObserver`; the text must travel on a port that never reaches the log | T-022 Investigation hypothesis 2 |
| A defaulted no-op `transcript_sink` in `PipelineDeps` | a forgotten wiring would pass silently | P-013 |
| The shell's sink found in app state with a keep-nothing fallback | a `run()` without a started history recorded nothing silently | T-022 review 1 #1, P-013 |

## Open

- none
