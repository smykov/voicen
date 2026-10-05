# Dictation session

**Code:** `crates/voicen-core/src/dictation.rs` (`DictationSession`), the controller inputs in `crates/voicen-core/src/recording/mod.rs` (`hotkey_registration`, `notice`, `live_id`, `Release::held`), the ports and fakes in `crates/voicen-core/src/platform.rs`, `audio::mix_to_mono`, `crates/voicen-core/src/test_support/realtime.rs`, `crates/voicen-core/src/win32_data.rs` · **Tests that pin it:** `crates/voicen-core/tests/dictation_session.rs` (all), `recording::tests::{failed_hotkey_registration_is_tray_hotkey_error_above_every_state_until_registered, successful_registration_clears_only_hotkey_error, notice_*, press_drops_a_notice_and_it_is_not_reshown, live_id_is_the_recording_on_and_none_otherwise}`, `win32_data::tests::*`, `tests/fakes.rs` (the session fakes)

Tasks: T-051 (split from T-006 by decision #64). Decisions: #47, #48, #63, #64. Class `dictation-session`; no `docs/failures.md` entry of this class yet, so the defects below are the risks the T-051 analysis named, not incidents.

## Invariants

### One FIFO worker is the only caller of `run_job` and `job_finished`; `run_job` never runs under the session lock

- **Defect that produced it:** none here (design risk: decision #48 left "`run_job` on one thread at a time" as a precondition for the code that builds the threads; research R-10).
- **What breaks if you violate it:** with a thread per job, two `deliver`s overlap and one transcript is pasted into another job's window, or jobs finish out of recording order; with `run_job` under the lock or on the input thread, a press waits for a slow engine (FR-029).
- **Where it is enforced:** the worker thread owns the `Pipeline` (no other thread holds a reference, so nothing else can call `run_job` or `Pipeline::pending`). Tests `jobs_never_overlap_in_delivery_and_finish_in_recording_order`, `press_during_a_slow_job_starts_recording_at_once`.
- **Don't:** spawn a thread per job; call `run_job` from `hotkey_released`; keep the session lock across `run_job`; give the pipeline to a second thread (T-011 replaces the worker with its delivery thread, it does not add one).

### Queue order is finish order is recording order

- **Defect that produced it:** none here (analysis option C, `finish` on the worker, rejected).
- **What breaks if you violate it:** a later recording delivered first; with `finish` on the worker, B's job is not counted until A's ends, so the overlay hides between A's end and B's finish and the processing count is wrong.
- **Where it is enforced:** `finish` and the push onto the channel happen in one critical section; the release path (release → stop → convert → finish) is serialised by its own mutex, taken before the session lock, so recordings released from different threads still queue in order. Test `jobs_never_overlap_in_delivery_and_finish_in_recording_order` (gate events in id order).
- **Don't:** push after unlocking; move `finish` to the worker. If T-006 measures the conversion as too slow for the hotkey thread (analysis Q6), add a converter thread FIFO-to-FIFO, not `finish` on the worker.

### Indicator changes reach the port only from inside the session lock, once per change, and never through Hidden between Recording and Processing

- **Defect that produced it:** none here (T-051 Notes (1), (2); analysis hypothesis 2(c) for the second-owner risk).
- **What breaks if you violate it:** published outside the lock, the tray and overlay can receive states out of the controller's order; without the comparison with the last sent value, the shell redraws for nothing; published between a stop's `release` and its `finish` (by the release itself, the worker's job end or the timer), the controller shows neither the recording nor its job, so the overlay window is destroyed and created again (Acceptance: Recording → Processing → Hidden). With that hold left in place by a panic in the release path, nothing is published until the next stop's `finish`.
- **Where it is enforced:** `Shared::publish`, called with the lock held right after each controller call; it compares `(tray, retry_available)` and the overlay with the last sent values (initialised from the controller's initial state, so nothing is sent at start) and sends nothing while `State::stopping` is set (from a stop's `release` to its `finish`, which then sends what changed meanwhile; if the release path panics in between, the drop guard `StopInFlight` clears it and publishes what the controller shows, and the panic reaches the caller). Tests `hold_gives_one_paste_and_overlay_recording_processing_hidden`, `a_job_ending_while_the_next_stop_is_in_flight_does_not_flash_hidden`, `a_panic_in_the_release_path_does_not_freeze_publication`, `hotkey_registration_sets_and_clears_the_tray_hotkey_error`.
- **Don't:** keep a second copy of the indicator state outside the controller; publish from the release path for a stop; clear `stopping` only on the normal path; let an `Indicator` implementation block on another thread or call the session (it is called under the lock: T-052 / T-053 hop to the main thread without waiting).

### A press whose capture fails never shows a recording state

- **Defect that produced it:** none here (FR-009; data-model "Idle --press [device fails]--> Idle + failure (no recording state)").
- **What breaks if you violate it:** the tray and overlay flash Recording for a microphone that never opened.
- **Where it is enforced:** the press publishes once, after `AudioSource::start` returned (`capture_failed` first on an error). Tests `access_denied_publishes_no_recording_state_and_queues_no_job`, `timer_follows_the_latest_deadline`.
- **Don't:** publish between `press` and the capture result; release the lock between them (another thread's publish would show the controller's Recording). Because the lock is held across them, don't let `Paster::capture_start_window` or `AudioSource::start`, nor dropping a `CaptureHandle` (the session's `Drop` does it under the lock while a recording is on), block, call back into the session or wait for another call on the same port: a capture adapter whose error or device-loss path calls a session input inside `start` deadlocks, and a paster that holds one mutex across its methods makes a press wait up to `MODIFIER_WAIT` (1 s) behind the worker's delivery, with the tray, the timer and job ends waiting too. T-006 and T-012 implement these ports against this rule (the trait docs in `platform.rs`, core-traits.md "Platform traits").

### `retry_available` is the pending slot, read on the worker after each job

- **Defect that produced it:** none here (analysis hypothesis 4 and 2(b); T-007 note).
- **What breaks if you violate it:** taken from `JobReport.pending`, a delivered text after a 503 hides the tray Retry while the audio is still pending (`Text`/`NoSpeech` report `pending: None` but leave the slot); read through `Pipeline::pending()` on an input thread, the tray menu or a press blocks while `keep_pending` holds the slot lock across `put_pending`.
- **Where it is enforced:** `run_worker` reads `pipeline.pending().is_some()` right after `run_job`; only the worker holds the pipeline. Tests `retry_available_follows_the_pending_slot`, `input_threads_never_wait_for_the_pending_slot`.
- **Don't:** derive retry from the job's report (a second copy of `release`'s slot rule); hand the pipeline to the tray.

### Capture opens only from idle, after the gate passed and `press` returned `Start`; snapshot and start window are taken once, at that press

- **Defect that produced it:** none here (FR-007, Clarification 4, P-013).
- **What breaks if you violate it:** auto-repeat runs the gate (a settings window per repeat) or opens a second stream; the job pastes into the window in front at release; a setting saved while recording changes the running dictation; a cached snapshot ignores a saved engine.
- **Where it is enforced:** `hotkey_pressed` checks `live_id()` first, then `snapshot()` and `dictation_gate`; blocked presses run `blocked_actions` in order (the notice through `RecordingController::notice` inside the lock, `open_settings` after it). Tests `press_while_recording_runs_no_gate_and_opens_no_second_stream`, `engine_none_opens_no_capture_and_asks_for_the_engine_tab`, `start_window_and_settings_are_taken_once_at_press`, `saved_settings_reach_the_next_press`.
- **Don't:** keep a session-side "recording" flag (P-010: ask `live_id`); read `Settings.mode` before T-009 (decision #63: toggle behaves as hold).

### The microphone is open only between press and release; OS text never leaves the session

- **Defect that produced it:** none here (NFR-02, P-009).
- **What breaks if you violate it:** the device stays open after release (privacy indicator on, device busy for other apps); OS error text reaches the log or the overlay.
- **Where it is enforced:** the capture handle is taken out of the lock at release and stopped before `hotkey_released` returns (a discard and a failing stop included); `Drop` closes a live capture. `CaptureFailed` carries only `MicCause`; the session's own `CaptureError::Other` texts are fixed literals (unconvertible frames, a format change within one capture). Tests `microphone_is_open_only_between_press_and_release`, `capture_error_at_stop_is_microphone_unavailable_and_no_job`.
- **Don't:** stop the capture on the worker; put a `CaptureError` into an event or a message.

### The timer ticks only once the deadline is reached, and follows the latest deadline

- **Defect that produced it:** none here; the timing rule of F-005 (voicen-core tests run on windows-latest too) shapes the tests.
- **What breaks if you violate it:** a message hidden early, or never (a timer armed only for the first deadline it saw).
- **Where it is enforced:** `run_timer` waits on the session condvar, which `publish` notifies after every change, and calls `tick` only when `now >= next_deadline()`. Tests `message_expires_without_input`, `timer_follows_the_latest_deadline` (real time; budgets sized for Windows timer resolution).
- **Don't:** sleep a fixed 3 s; tick on every wake-up; assert exact deadlines in tests.

### Drop lets the job in flight finish, drops the queued ones and joins both threads

- **Defect that produced it:** none here (analysis Q3; core-traits "shutdown: drop in-flight results").
- **What breaks if you violate it:** a detached worker keeps the ports alive after the session is gone, or queued recordings are transcribed and pasted after exit began (`mpsc` still hands out buffered items after the sender is dropped).
- **Where it is enforced:** `Drop` sets `shutdown` and drops the sender under the lock, then joins; the worker checks `shutdown` after each `recv`. Test `drop_finishes_the_job_in_flight_drops_the_queued_one_and_joins_threads` (every port's `Arc` count back to 1).
- **Not on the app's exit path (T-052):** the shell keeps the session as managed state and the process ends by tao's `process::exit` after `RunEvent::Exit` (only the tray "Exit" item ends it, `docs/decisions/windows-shell.md`), so `Drop` never runs at app exit: an in-flight or queued dictation is dropped with the process (spec 001 spec.md:155, "exit while processing"). Deleting the pending audio on exit (FR-032) is T-007's, on `RunEvent::Exit`.
- **Don't:** rely on dropping the sender alone; wait at app exit for the job in flight.

### The port implementations (T-006) and the release-1 defaults the session runs with

- `AudioSource`: `src-tauri/src/win/capture.rs` `CpalSource` (default input device, bounded open); `Clipboard`: `win/clipboard.rs` `WinClipboard`; `Paster`: `win/paste.rs` `WinPaster`; `Indicator`: `dictation.rs` `ShellIndicator` (tray half to `tray::TrayPart`, overlay half to `overlay::OverlayPart`: the newest state into the overlay mailbox, the one overlay thread builds, emits to or destroys the window; T-057, `docs/decisions/overlay.md`); `ShellRequests`: `dictation.rs` `SettingsRequests` (`settings_window::request(Tab(tab, None))`, receipt dropped). Their rules (bounds under the lock, no OS text, one wiring function): `docs/decisions/windows-shell.md`.
- `TempAudioStore`: an interim `NoPendingAudio` in `dictation.rs` whose `put_pending` fails, so a failed dictation leaves no pending recording until T-007's file store replaces it (T-001 Notes).
- Speech gate: `SpeechGate::new(Err(VadError::Unavailable), EnergyDetector::new())` in release 1 (no Silero, #62, T-043): the energy detector decides and the first decision writes one `vad_fallback` line per process (FR-016; T-006 Q3 default).

### Win32 facts come from `win32_data` only

- **Defect that produced it:** none here (analysis hypothesis 2(d), P-010).
- **What breaks if you violate it:** the shell computing modifier bits, virtual keys or "elevated" on its own is a second, Windows-only source: a wrong code or a reversed comparison is caught only on Windows CI, or not at all.
- **Where it is enforced:** `hotkey_codes` (always `MOD_NOREPEAT`; Win polled as left or right), `released` (the first key found up releases), `target_elevated` (unknown own or target level = elevated, analysis Q4); tests against independent literal tables. T-006 cross-checks the values against `windows::Win32` constants on Windows CI.
- **Don't:** poll only `VK_LWIN`; treat an unknown integrity level as safe.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| The session in `src-tauri` (option B) | a second owner of decisions next to the controller, provable only on Windows CI (~25-minute rounds) | decision #64, T-051 analysis |
| `finish` on the worker (option C) | B's job is counted only after A's ends: Hidden flash between the jobs, wrong processing count | T-051 analysis |
| `retry_available` from the last `JobReport.pending` | `Text`/`NoSpeech` report `None` while the slot keeps its entry: Retry hidden with audio pending | T-051 analysis hypothesis 4 |
| Publishing right after `release` for a stop | the overlay goes Recording → Hidden → Processing | T-051 Notes (1) |
| Holding the session lock across stop and conversion | blocks the tray, the worker's job end and the timer for the conversion time; the `stopping` deferral gives the same indicator order without it | T-051 implementation |

## Open

- Converting the frames runs on the hotkey thread at release (analysis Q6); T-006 built the hotkey thread without measuring it (the optional 30 s / 48 kHz stereo timing was not done): still open, measure on Windows before a converter thread is decided.
- If `run_job` ever panicked, the worker would end: later recordings are dropped at the queue and the overlay stays Processing. `run_job` is written not to panic (no `unwrap` on engine or server data); no restart is built.
