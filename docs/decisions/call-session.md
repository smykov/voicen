# Call session (start, stop, indicator, drops)

**Code:** `crates/voicen-core/src/call/session.rs` (`CallSession`, `CallDeps`, the ports `CallIndicator`, `CallShell`, `CallSettingsPort`, `SubtitleConsumer`; the device thread and the writer's device accounting), `crates/voicen-core/src/call/capture.rs` (the ports `DeviceEvents`, `DeviceEventsFactory`, `CommunicationsCheck`), `crates/voicen-core/src/call/indicator.rs` (the pure indicator model, `WarningList` and its wire types), `CallWarning::ProviderNotice` in `call/events.rs`, `SettingsService::remember_warned_host` · **Tests that pin it:** `tests/call_session.rs` (`one_session_for_repeated_on_from_every_source`, `a_track_that_fails_to_open_refuses_the_start_and_leaves_nothing`, `a_root_refusal_leaves_nothing_and_asks_for_the_calls_tab`, `a_storage_failure_after_the_probe_logs_its_kind_then_refuses_root_other`, `indicator_changes_inside_the_lock_and_shell_requests_after_it`, `provider_notice_once_per_new_non_loopback_host`, `dropped_blocks_are_logged_once_and_kept_in_meta`, `a_gap_without_a_device_change_leaves_no_line_and_no_entry`, `a_failed_final_meta_write_is_logged_and_the_stop_still_completes`, `a_failed_meta_write_at_a_drop_is_logged_and_the_call_goes_on`, `subtitles_only_writes_nothing`, `a_subtitles_start_that_cannot_open_a_track_is_refused_and_leaves_nothing`, `record_shares_the_captures_of_running_subtitles_and_survives_their_record_off`, `subtitles_off_while_recording_keeps_the_call_and_the_consumer_stops`, `subtitles_and_record_toggle_independently_under_one_lock`), `tests/call_indicator_wire.rs` (`e2e_call_indicator_wire_fixture_matches_core`, fixture `e2e/fixtures/call-indicator-wire.json`), `tests/dictation_session.rs` (`dictation_and_esc_never_change_a_running_call`)

Tasks: T-126; T-144 (I12); T-149 (device changes, silent sides, the communications check, `dismiss_warning`; I8-I11). Contract: `specs/007-call-mode-record/contracts/core-call.md` "Built (T-126)", "Built (T-149)". Decisions: #195 (2) (`ProviderNotice` moved here), #243 (D1/D2), #244. Class `call-session` in `docs/failures.md` has no entry yet (first sighting).

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

### I3. No byte of call audio or meta reaches disk unless record is on (FR-019)

- **What breaks if you violate it:** a Subtitles-only session (1b) leaves call audio or a folder on disk the user never asked to record (decision #164).
- **Where it is enforced:** in the session, not by the missing Subtitles toggle of 1a. The session holds both toggles (`record`, `subtitles`); the two captures are open exactly while `record || subtitles`. `toggle_subtitles(on, source)` runs under the same lock as `toggle_record`: on opens the captures (system, mic) unless record holds them, and the writer feeds the `SubtitleConsumer` port; it never reaches `CallStore`, the root probe, the Calls tab, the indicator or the log. `CallStore` is reached only from `open_call` (the record-on path). Record on while subtitles run reuses their captures (no second open) and attaches a new call to the running writer; record off while subtitles run finishes the call folder and keeps the captures; subtitles off while recording only stops the feed; the captures close when both are off. A toggle that changes a running writer is a synchronous command: blocks queued before it returns are handled under the old state. Tests `subtitles_only_writes_nothing` (fake consumer fed, root empty, no probe, no line), `a_subtitles_start_that_cannot_open_a_track_is_refused_and_leaves_nothing`, `record_shares_the_captures_of_running_subtitles_and_survives_their_record_off`, `subtitles_off_while_recording_keeps_the_call_and_the_consumer_stops`, `subtitles_and_record_toggle_independently_under_one_lock`; `a_session_that_is_off_writes_nothing` covers the idle session.
- **Don't:** call `CallStore` (or write under the root) from the subtitles path, or open a second capture for the second toggle.

### I4. Indicator inside the lock, shell after it

- **What breaks if you violate it:** the dictation-session race class (`docs/decisions/dictation-session.md`): a window showing a state the session has left, or a shell call that re-enters the session and deadlocks.
- **Where it is enforced:** `CallIndicator` (`set_state`, `set_tick`, `set_call_recording`) is called only with the session lock held, once per change, and must never block; `CallShell` (`notice`, `open_calls_tab`) only after it. The writer thread takes the lock to publish a tick and skips the tick while the session holds the lock to wait for it (a stop joining it, a toggle command). Subtitles alone publish no indicator call. The reminder and the headphones hint show from start until start + 10 s of the monotonic clock (`next_deadline` / `tick`, driven by the shell's timer and by the fake clock in tests) and never when `calls.reminder` is off. Tests `indicator_changes_inside_the_lock_and_shell_requests_after_it`, `the_reminder_shows_for_10_seconds_at_each_start`.
- **Don't:** call a shell port inside the lock or an indicator port outside it.

### I5. The provider notice comes from one rule and is remembered by one writer

- **Where it is enforced:** `provider_host` over the API base URL (`check_base_url`) at start; the notice shows only when the host is not in `calls.warned_hosts`, then `CallSettingsPort::remember_warned_host` (`SettingsService::remember_warned_host`, `docs/decisions/settings.md`). A loopback host gives no notice. When the write fails the notice still shows and shows again at the next call. The host reaches only the indicator's rendered `warning.provider_notice` text (`CallWarning::ProviderNotice` serializes `host` through `as_str`); it never reaches a log line.

### I6. No Esc input and no shared state with `DictationSession`

- **Where it is enforced:** the session takes no Esc; test `dictation_and_esc_never_change_a_running_call` runs both directions.

### I7. A channel drop is one log line and one `drops[]` entry; a stall gap is neither (#244)

- **Where it is enforced:** the writer thread reads `at_ms` = `Timeline::written() / 16` before `Timeline::dropped`; each `Report::Dropped` gives one `call_track_dropped track= blocks= ms= at_ms=` WARN line (`docs/decisions/diagnostics-log.md`) and one `TrackDrop` appended to the in-memory meta, written through `storage::write_meta` (`docs/decisions/call-storage.md`). `Report::Gap` is ignored, a revive's own included: a gap tied to a device change is measured by the writer from the track's `Lost` to the first block of the new stream and stays one `call_device ... gap_ms` line and one `device_changes` entry (I9, T-149).
- **Don't:** log a gap as a drop, or write `meta.json` another way.

### I8. One device decision per track, on the device thread (T-149 D1, D2)

- **What breaks if you violate it:** two handles of one track open at once (two callbacks writing one timeline), a reopen racing a close, or an open of up to 3 s (`OPEN_BUDGET` in the shell) blocking the healthy track's drain or the tray and hotkey.
- **Where it is enforced:** `session.rs` `DeviceThread`, one per capture lifetime (spawned in `spawn`, stopped and joined first in `Captures::close`). It owns both `CaptureHandle`s after start and is the only code that stops or opens a track handle while the captures are open; the old handle is stopped before the new one opens. Its triggers: a `TrackEvent::Lost` of a track (its `OpenSink` cuts the old stream off and keeps a lost flag the thread polls) and `DeviceEvents::next` from `DeviceEventsFactory::open`, called on the device thread (`DefaultOutputChanged` -> system; `DefaultInputChanged` -> mic only when the mic follows the default; `DeviceRemoved(id)` -> the track whose device or endpoint it names). A trigger while a reopen is pending is absorbed, and after a successful open the events already queued for that track are absorbed. The first attempt is at once (system on `DefaultOutputLoopback`, mic on `DefaultInput`); at the first round with 2 s of the monotonic clock since the trigger the track is declared silent (cause `no_device` when the last open said `NoDevice`, else `track_lost`) and retried once per 2 s mark until it opens (cause `reopened`) or the captures close. The thread takes no session lock; it sends typed outcomes (`Reopened`, `Silent`) to the writer. Tests `a_default_output_change_reopens_the_system_track_with_its_gap_in_meta_and_log`, `a_default_output_event_and_its_loss_reopen_once`, `a_lost_selected_mic_reopens_on_the_default_input_with_mic_changed_and_keeps_recording`, `a_default_mic_follows_a_default_input_change`, `a_track_not_reopened_within_2_s_is_silent_warned_and_retried_every_2_s`, `no_device_left_gives_no_device`, `closing_while_a_reopen_is_pending_leaves_no_handle_open`.
- **Don't:** open or stop a track handle on the writer, under the session lock, or anywhere but the device thread while the captures are open.

### I9. No hole in a track's timeline; one entry per device change (T-149 D3)

- **What breaks if you violate it:** a dead track or a paused loopback (no packets, #244) stops growing, its file is short, `duration_ms` (the shorter track) shrinks and later audio shifts; or a device change is logged twice or not stored.
- **Where it is enforced:** the writer's `advance`: a lost track follows the other track's latest block end at once; a live one that delivered nothing for 500 ms of the monotonic clock follows it too (200 ms behind, `Timeline::advance_to`); when neither delivers, both follow `TrackSource::now`. At record off both are brought to the other's latest block before the flush. A device change's gap starts at the track position after its `Lost` and ends where the new stream's first block lands (`Timeline::position_ms`, never before the position already written); a reopened stream that sends nothing for 2 s has its gap closed where the track stands. Each change gives one `call_device track= cause= gap_ms= to=` line and one `DeviceChange` appended and written at once through `storage::write_meta` (a failure logs `call_storage_failed kind=meta` and the call goes on). A silent declaration gives `call_device ... to=none` with `gap_ms` = the silence so far, plus `call_track_silent rule=lost`; the later reopen gives a second line and entry (cause `reopened`) with the whole gap. A record start after a loss logs the reopen with gap 0. Tests: the I8 tests and `a_silent_loopback_still_advances_the_system_track`.
- **Don't:** take `gap_ms` from `Report::Gap` (silent under 200 ms), or write `device_changes` only at stop.

### I10. One warning list, raised and dismissed under the lock (T-149 D4)

- **What breaks if you violate it:** a warning listed twice, a dismissed warning that never comes back, or an indicator state published outside the lock (I4).
- **Where it is enforced:** `indicator.rs` `WarningList` is the call's only list (`Call.warnings`): `raise` adds nothing when an equal warning (kind, and track for `TrackLost`) is listed; `dismiss(id)` removes by id, so the next trigger adds a new entry with a new id. The writer raises warnings with `try_lock` and keeps them pending while the session holds the lock (never skipped, unlike ticks); `CallSession::dismiss_warning(id) -> bool` removes and republishes under the lock and returns false for an unknown id or when no call records. FR-013: each track gets a speech verdict (`EnergyDetector`) per second of placed 16 kHz audio; more than 60 silent seconds while the other track had a speech second in that stretch gives one `cant_hear_other_party` (system silent) or `cant_hear_you` (mic silent) and one `call_track_silent rule=cant_hear` per stretch, and the tick's `*_silent` stays true until the next speech second. A second with lost-track audio ends the stretch, so a lost track never gives it. A `TrackLost` warning stays after a later reopen until dismissed (FR-006). Tests `cant_hear_other_party_after_60_s_once_per_stretch_and_rearmed_by_speech`, `cant_hear_you_after_60_s_of_mic_silence_mirrors_the_system_case`, `no_cant_hear_when_nobody_speaks_or_the_silent_track_is_lost`, `a_dismissed_warning_comes_back_on_its_next_trigger_and_a_listed_one_is_not_duplicated`; unit tests in `indicator.rs`.
- **Don't:** keep a second warning list, or publish a warning from the writer without the lock.

### I11. The communications check, once per record start (T-149 D5, D6)

- **Where it is enforced:** `begin_call` calls `CommunicationsCheck::differs` before `call_started`; only `Ok(true)` gives `CallWarning::CommunicationsDeviceDiffers` and `comms_differs=1`; `Ok(false)`, `Err(E_NOTFOUND)` and any other `Err` are no difference and no line (the grammar has none). A subtitles start never asks. With subtitles only (no record) device outcomes and warnings are neither logged, stored nor shown; reopens still happen (I3). Test `the_communications_check_gives_the_warning_only_for_a_difference`.

### I12. `tick` is the one place for non-user stops; the writer only raises a flag (T-144)

- **What breaks if you violate it:** a call that keeps recording after a dead disk, a stop from the writer thread racing the lock (I4), or a warning list entry raised twice.
- **Where it is enforced:**
  - `CallSession::tick` (the shell's timer) stops the call, with saving, for: the storage failure flag, 4 h of monotonic time (`MaxLength`), and low disk. The disk is polled every 10 s, rescheduled from when the poll runs; the first poll is due at start. Below 1 GB one `LowDisk` warning per call goes through `WarningList::raise`; below 200 MB `call_storage_failed kind=low_disk` is logged, then the `LowDisk` stop. An `Err` from the disk port is skipped silently.
  - The writer's first encoder push error drops both encoders, sets the failure flag in `Control`, and logs `call_storage_failed kind=write` once. After it there is no `write_meta` mid-call: device changes and drops stay in memory. A `StorageError` stop leaves `meta.json` at `recording`, so the next start-up recovers it (`docs/decisions/call-storage.md`); the other stops write the final meta.
  - Notices: `MaxLength`, `LowDisk` and `StorageError` give `CallNotice::Stopped { reason }`; `User`, `Hotkey`, `Exit`, `SystemEnd` and `Sleep` give `CallSavedTranscribing`.
  - `next_deadline` is now after a failure; otherwise the minimum of the reminder, the next poll and the 4 h limit, never more than 10 s ahead while recording, `None` otherwise.
  - Every end of a call (`finish`, a start that fails after the create) calls `CallStore::release` on its folder.
  - `CallDeps.open_track` returns `TrackFile = Box<dyn SyncWrite + Send>` (`impl SyncWrite for Box<W: ?Sized>` in `codec.rs`), so the rig's `FlakyOpener::fail_next_write` injects a write fault.
- **Don't:** stop a call from the writer thread, write `meta.json` after a write failure, poll the disk on a fixed grid, or raise the low-disk warning twice.

## Failure handling the developer chose beyond the analysis

- A failed encoder `push` in the writer: since T-144 it stops the whole call (I12); before it, only that track stopped.
- A failed encoder `finish` at stop logs `call_storage_failed kind=<kind>` and the stop goes on.
- A failed `meta.json` write at a drop logs `call_storage_failed kind=meta` (with `os` when known) and the call goes on; the stop writes the full `drops` list again. A failed final `meta.json` write at stop is logged the same way and the stop still completes.
- The writer publishes a tick after any placed audio once 100 ms of the monotonic clock passed (at most 10 per second), with each track's latest level; before T-149 it ticked only on a step that closed a level window.
- Any mic reopen (selected device removed, or the default input changed while the mic follows the default) raises `MicChanged` (FR-012 "loss or change ... with a notice"); after a reopen the mic follows the default input.
- A stop before a pending reopen opens or is declared silent gives no `call_device` line for it.
- `CallSession` keeps the start's endpoint ids; after a reopen the ids are only in the `call_device` line and `device_changes`.
- Known residual: a `Lost` marker that finds the track's channel full is kept in the channel's lost flag and may reach the writer after the new stream's first blocks; the gap is then measured from where that marker lands.
- A writer thread that is gone at record off (it panicked) logs `call_storage_failed kind=write` (no `os`); the stop still writes `meta.json` from the meta of the create (`ready-to-transcribe`, `duration_ms` 0) and logs `call_stopped`. Not covered by a test (no portable way to make the writer panic).
- At record off the writer flushes both timelines (`Timeline::flush`), so the files keep the audio still inside the resamplers up to the last block.
- The subtitle consumer gets each block downmixed to mono at 16 kHz, without placement on the call's axis (a 16 kHz block as is, another rate through a streaming FFT resampler); `Lost` and `Dropped` events give it nothing.
- Subtitles toggles write no log line (the closed grammar has none for them); a subtitles start that cannot open a track returns `Refused { part: system | mic, reason }`, and one whose writer thread cannot be spawned (or is gone) `Refused { part: system, reason: other }`. A record start on running subtitles whose writer is gone is refused `part=root reason=other` and its folder removed.
- `call_started` keeps `subtitles=0` and `meta.json` `mode.subtitles` stays `false` also for a record start while subtitles run: the contract fixes them for 1a; the 1b task decides (OQ-77).
- `storage::started_at` is `pub(crate)`, so the stop writes `ended_at` in the one date format.
- Not covered by a test: the encoder finish failure and the lost writer at record off. The mutants 36 and 37 of T-126 survived because a `File` whose write fails mid-call had no portable fake; since T-144 the write fault is injectable (`FlakyOpener::fail_next_write`, I12).

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| The session drives the encoders from a `pump()` call, no writer thread | the contract says `push` runs on the session's writer thread; the shell would own the timing | T-126 analysis, option B |
| `warned_hosts` kept in the session or a shell-side store | FR-031 "once" must hold across restarts; a second settings writer would bypass `save_lock` | T-126 analysis, option C |
| A drop recorded as a `DeviceChange` with a new `DeviceCause` | puts a non-device fault into the `call_device` cause set; `device_changes` would mean two things | T-126 analysis, option D, #244 |
| Choosing the fallback mic through `microphone::choose` over `AudioSource::devices()` | needs a second port in the session; the track source already opens "the selected id or the default input" | T-126 investigation |
| Option C of T-144: the writer wakes the shell's timer at once on a failure | the stop may wait up to 10 s (`next_deadline` bound); accepted, not built | T-144 analysis |
| Reopening a track on the writer thread under `try_lock` | an open of up to 3 s (`OPEN_BUDGET`) stops the healthy track's drain (channel drops), and the 2 s deadline cannot fire while it blocks | T-149 analysis, option B |
| Driving reopens from `CallSession::tick` on the shell's timer | the 2 s bound would depend on the shell's cadence; an open under the session lock blocks tray and hotkey for up to 3 s; R4 says core decides | T-149 analysis, option C |
| The writer forwarding each `Lost` to the device thread over a channel | a second path for the same signal; the `OpenSink` lost flag carries it without the writer | T-149 implementation |

## Open

- None.
