# Call session (start, stop, indicator, drops)

**Code:** `crates/voicen-core/src/call/session.rs` (`CallSession`, `CallDeps`, the ports `CallIndicator`, `CallShell`, `CallSettingsPort`), `crates/voicen-core/src/call/indicator.rs` (the pure indicator model and its wire types), `CallWarning::ProviderNotice` in `call/events.rs`, `SettingsService::remember_warned_host` · **Tests that pin it:** `tests/call_session.rs` (`one_session_for_repeated_on_from_every_source`, `a_track_that_fails_to_open_refuses_the_start_and_leaves_nothing`, `a_root_refusal_leaves_nothing_and_asks_for_the_calls_tab`, `a_storage_failure_after_the_probe_logs_its_kind_then_refuses_root_other`, `indicator_changes_inside_the_lock_and_shell_requests_after_it`, `provider_notice_once_per_new_non_loopback_host`, `dropped_blocks_are_logged_once_and_kept_in_meta`, `a_gap_without_a_device_change_leaves_no_line_and_no_entry`, `a_failed_final_meta_write_is_logged_and_the_stop_still_completes`, `a_failed_meta_write_at_a_drop_is_logged_and_the_call_goes_on`), `tests/call_indicator_wire.rs` (`e2e_call_indicator_wire_fixture_matches_core`, fixture `e2e/fixtures/call-indicator-wire.json`), `tests/dictation_session.rs` (`dictation_and_esc_never_change_a_running_call`)

Task: T-126. Contract: `specs/007-call-mode-record/contracts/core-call.md` "Built (T-126)". Decisions: #195 (2) (`ProviderNotice` moved here), #243 (D1/D2), #244. Class `call-session` in `docs/failures.md` has no entry yet (first sighting).

## Invariants

### I1. `toggle_record(on, source)` is the only start/stop path, under one lock

- **What breaks if you violate it:** two captures or two folders for one call; a stop racing a start.
- **Where it is enforced:** `CallSession::toggle_record` from tray, hotkey and switch; while record is on there is one active call (one capture per track, one folder) and a repeated on returns `AlreadyOn` and changes nothing. Tests `one_session_for_repeated_on_from_every_source`, `concurrent_starts_give_one_session`.
- **Don't:** add a second start path or open a track outside the session.

### I2. A start reaches Recording or leaves nothing

- **What breaks if you violate it:** a handle left open, an orphan folder listed as `damaged`, or two `call_refused` lines for one start.
- **Where it is enforced:** `open_call` opens the system track, then the mic, then `CallStore::create_with` (the probe lives there); a failure stops what was opened and gives one `call_refused` naming the first failing part. A saved mic that is gone (`NoDevice` on `Selected`) is retried once on the default input with `CallWarning::MicChanged`; busy and access_denied are not retried.
  - Root refusals map `UnwritableReason` as: missing to `missing`, access_denied to `access_denied`, not_a_folder / relative / disk_full to `unwritable`, other to `other`.
  - **#243 D1:** every `part=root` refusal, `missing` included (a removed USB drive, spec scenario 10), asks the shell to open the Calls tab, once, after the lock. A track refusal never does.
  - **#243 D2:** `CreateError::Storage(kind)` after the probe logs `call_storage_failed kind=<kind>` (no `os`), then `call_refused part=root reason=other`; the tab opens by D1.
  - The writer thread cannot be spawned: refused as `part=root reason=other` (no `call_storage_failed` line), both captures stopped, the folder just made removed, the Calls tab opens (T-126 implementation choice, same rule as D2).
- **Don't:** probe before the tracks, leave a capture open on a later failure, or open the tab for a track refusal.

### I3. No byte of call audio or meta reaches disk unless record is on

- **Where it is enforced:** `CallStore` is reached only on the record-on path of `start`; Subtitles-only writes nothing (`subtitles_only_writes_nothing`).

### I4. Indicator inside the lock, shell after it

- **What breaks if you violate it:** the dictation-session race class (`docs/decisions/dictation-session.md`): a window showing a state the session has left, or a shell call that re-enters the session and deadlocks.
- **Where it is enforced:** `CallIndicator` (`set_state`, `set_tick`, `set_call_recording`) is called only with the session lock held, once per change, and must never block; `CallShell` (`notice`, `open_calls_tab`) only after it. The writer thread takes the lock to publish a tick and skips the tick while a stop holds the lock to join it. The reminder and the headphones hint show from start until start + 10 s of the monotonic clock (`next_deadline` / `tick`, driven by the shell's timer and by the fake clock in tests) and never when `calls.reminder` is off. Tests `indicator_changes_inside_the_lock_and_shell_requests_after_it`, `the_reminder_shows_for_10_seconds_at_each_start`.
- **Don't:** call a shell port inside the lock or an indicator port outside it.

### I5. The provider notice comes from one rule and is remembered by one writer

- **Where it is enforced:** `provider_host` over the API base URL (`check_base_url`) at start; the notice shows only when the host is not in `calls.warned_hosts`, then `CallSettingsPort::remember_warned_host` (`SettingsService::remember_warned_host`, `docs/decisions/settings.md`). A loopback host gives no notice. When the write fails the notice still shows and shows again at the next call. The host reaches only the indicator's rendered `warning.provider_notice` text (`CallWarning::ProviderNotice` serializes `host` through `as_str`); it never reaches a log line.

### I6. No Esc input and no shared state with `DictationSession`

- **Where it is enforced:** the session takes no Esc; test `dictation_and_esc_never_change_a_running_call` runs both directions.

### I7. A channel drop is one log line and one `drops[]` entry; a stall gap is neither (#244)

- **Where it is enforced:** the writer thread reads `at_ms` = `Timeline::written() / 16` before `Timeline::dropped`; each `Report::Dropped` gives one `call_track_dropped track= blocks= ms= at_ms=` WARN line (`docs/decisions/diagnostics-log.md`) and one `TrackDrop` appended to the in-memory meta, written through `storage::write_meta` (`docs/decisions/call-storage.md`). `Report::Gap` is ignored; a gap tied to a device change stays `call_device ... gap_ms` (a later task).
- **Don't:** log a gap as a drop, or write `meta.json` another way.

## Failure handling the developer chose beyond the analysis

- A failed encoder `push` in the writer logs `call_storage_failed kind=<kind>` (kind from `storage_kind(op)`, no `os`) and stops writing that track only; the other track and the call go on.
- A failed encoder `finish` at stop logs `call_storage_failed kind=<kind>` and the stop goes on.
- A failed `meta.json` write at a drop logs `call_storage_failed kind=meta` (with `os` when known) and the call goes on; the stop writes the full `drops` list again. A failed final `meta.json` write at stop is logged the same way and the stop still completes.
- `IndicatorTick.mic_silent` and `system_silent` are always `false` until the FR-013 silence task.
- `storage::started_at` is `pub(crate)`, so the stop writes `ended_at` in the one date format.
- Not covered by a test: the encoder push and finish failures (the mutants 36 and 37 of T-126 survived; a `File` whose write fails mid-call has no portable fake).

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| The session drives the encoders from a `pump()` call, no writer thread | the contract says `push` runs on the session's writer thread; the shell would own the timing | T-126 analysis, option B |
| `warned_hosts` kept in the session or a shell-side store | FR-031 "once" must hold across restarts; a second settings writer would bypass `save_lock` | T-126 analysis, option C |
| A drop recorded as a `DeviceChange` with a new `DeviceCause` | puts a non-device fault into the `call_device` cause set; `device_changes` would mean two things | T-126 analysis, option D, #244 |
| Choosing the fallback mic through `microphone::choose` over `AudioSource::devices()` | needs a second port in the session; the track source already opens "the selected id or the default input" | T-126 investigation |

## Open

- None.
