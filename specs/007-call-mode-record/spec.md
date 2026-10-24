# Feature Specification: Call Mode 1a — Record and Read

**Feature Branch**: `007-call-mode-record` (spec directory; work happens on `main` per project rules)

**Created**: 2026-10-22

**Status**: Draft (clarifications answered by the owner 2026-10-22)

**Input**: User description: "Call mode, increment 1a 'Record and read': record a call as two tracks (microphone = Me, system loopback = Other party), store them under the call folder, after stop run the final pass through the dictation settings' API engine in chunks of at most 120 s, and show the transcript in the Calls section with copy and export. Not in 1a: subtitles (1b), diarization (1c), local engines (1d)."

**Source requirements** (`docs/requirements.md` v7, approved 2026-10-22, decisions #183): the §3 increment row "1a — Record and read": FR-30…FR-42, FR-45…FR-52, FR-59 (language storage only), FR-60, FR-63…FR-65, FR-67, FR-71…FR-77, FR-80 (the 1a items, see FR-040 below), FR-81…FR-84; NFR-13 (API final only), NFR-15…NFR-19, NFR-21, NFR-22; the release-1 amendments FR-22 `(v6, r3#18)`, FR-28 `(v6, r3#1)` and `(v7, r4#1, r4#12)`, NFR-02 `(v6, r3#2)`, NFR-06 `(v6, r3#1)`; §2 release-2 goal, flow, success criteria 1, 4 (API) and 5; §7 release-2 storage layout and `transcript.json` schema 1; §9 release-2 rows (stack, "deployed", verification, UI verification). Owner decisions honoured: #159–#178, #180–#183 (notably #164 subtitles-only writes nothing, #165 consent, #170 calls kept until deleted, #171 increment order, #175 Silero in 1a, #183 OQ-64 (a): 1a stays as written). Package ADRs read: ADR-001…ADR-012 of `docs/briefs/call-mode-r1/adr/` (owner-accepted parts = decisions #159–#166; proposed parts = OQ-41…OQ-49, CA-10).

**Prerequisites** (requirements §3 1a row, §6): T-102 — the Silero detector ships in the installer (decisions #175, OQ-51 (a)); T-007 — toasts with actions (OQ-56 (a)). Both are TODO today; this feature's notices and its "no invented text" guarantee depend on them.

**Not in 1a** (requirements §3): the Subtitles toggle, subtitle window and subtitle engine (FR-53…FR-58, FR-66; 1b); diarization and speaker merge (FR-62, FR-74; 1c); built-in/local call engines, echo handling, quick final, summaries, playback, auto-delete (FR-43, FR-44, FR-61, FR-68…FR-70, FR-78, FR-79, NFR-14, NFR-20; 1d); translation (decisions #166).

## Clarifications

### Session 2026-10-22

- Q: How does the install smoke switch Record on in the shipped app? → A: A command-line switch handed to the running instance, e.g. `voicen.exe --call record`; it is both the user-facing and the CI way to start Record; the recording indicator is always shown; trade-off: any local process can start a recording (decisions #184)
- Q: Which open English corpus and subset for the final's WER (OQ-65)? → A: A pinned subset of LibriSpeech test-clean (CC BY 4.0), at least 30 min, fetched by checksum; read speech makes the WER optimistic, the owner's Russian call set covers real calls (decisions #185)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Record a call as two tracks (Priority: P1)

The owner joins a call in Telegram Desktop, a browser meeting, Zoom or Teams. From the tray ("Call" › "Record") or with the call hotkey (Ctrl+Alt+R) they start call mode. A movable call indicator appears with "● Recording 00:00", the level of both tracks, and for 10 s the reminder to tell the other party about the recording plus the "better with headphones" hint. Their microphone is recorded as "Me" and what the PC plays as "Other party", never mixed, into a new call folder. They stop from the tray, the hotkey or the indicator; the files are closed and the notice "Call saved — transcribing" appears.

**Why this priority**: Recording is the first half of the increment's done-when ("the owner recorded and read a real one-to-one call", requirements §3) and the base of every other story.

**Independent Test**: On the Linux host, the call session with WAV-backed fake sources: start → a call folder with `meta.json` in state `recording` and two growing track files → stop → both files closed and readable, state `ready-to-transcribe`/`transcribing`. In Windows CI, the same through the real session with WAV sources; on the device-less runner the installed app refuses Record with "no output device" and keeps running. On the owner's PC, a real call.

**Acceptance Scenarios**:

1. **Given** call mode is off, **When** the user checks "Call" › "Record" in the tray, **Then** within ≤ 1 s the call indicator shows "● Recording" with a running timer, the tray shows the call-recording state, and a new folder `<root>\<YYYY-MM-DD_HHMMSS>\` holds `meta.json` (state `recording`) (req FR-30, FR-32, FR-34, §7).
2. **Given** a call in a call app, **When** both sides speak, **Then** the user's voice is only in `mic.opus` and the other party's only in `system.opus` (req FR-39).
3. **Given** first use in 1a, **When** the user presses Ctrl+Alt+R, **Then** call mode starts with Record only; **When** it is pressed again, **Then** the call stops (req FR-33, `(v7, r4#7)`).
4. **Given** a recording runs, **When** "Record" is chosen again in the tray or the hotkey's start is repeated through any entry, **Then** there is still one session, one capture per track and one call folder (req FR-37).
5. **Given** any recording start, **Then** the reminder and the headphones hint are visible in the indicator for 10 s; **Given** the reminder is turned off in the Calls settings tab, **Then** neither is shown (req FR-35).
6. **Given** a recording runs, **When** Record is turned off, **Then** `mic.opus` and `system.opus` are closed and readable, the Calls list shows the call as "Queued" or "Transcribing…", and the notice "Call saved — transcribing" appears (req FR-31, FR-48, FR-71, FR-81).
7. **Given** a call was stopped, **When** Record is turned on again, **Then** a second call folder is created (req FR-31, r3#12).
8. **Given** no output device exists (the CI runner), **When** Record is turned on, **Then** call mode does not start, the notice and the log name the system track and the reason, and the app keeps running (req FR-30, r3#5; §9 "deployed").
9. **Given** the microphone is denied by Windows privacy settings or no input device exists, **When** Record is turned on, **Then** call mode does not start and the notice names the mic track and the reason; there is no one-track recording (req FR-30).
10. **Given** the storage root is on a removed USB drive or is unwritable, **When** Record is turned on, **Then** Record is refused with the reason and the Calls settings tab opens (req FR-30, r3#7).
11. **Given** the selected USB microphone is unplugged and a built-in microphone exists, **When** Record is turned on, **Then** the call starts on the Windows default input and the notice names it (req FR-30, `(v7, r4#22)`).
12. **Given** the app starts, **Then** call mode is off and no call window is shown (req FR-30, r3#19).
13. **Given** Voicen is running, **When** `voicen.exe --call record` is run, **Then** the running instance starts Record exactly as the tray does (same refusals, the indicator always shown) and no second instance starts (decisions #184, FR-001).
14. **Given** the README or the help, **When** the user looks for recording consent, **Then** a section in English and in Russian says the user is responsible for the other party's consent and gives a sample opening phrase (requirements §3 1a row, decisions #165, r3#34).

---

### User Story 2 - Get the final transcript after the call (Priority: P1)

After stop, Voicen transcribes both whole tracks again with the API engine configured for dictation (base URL, model, key, speech language), in chunks of at most 120 s cut at speech pauses. Silence and music produce no text. The lines of both tracks are merged by time — "Me" for the microphone, "Other party" for the system track — and saved as `transcript.json` and `transcript.md` in the call folder. The notice "Transcript ready — Open" opens the call.

**Why this priority**: The second half of the done-when; without it the recording is only audio.

**Independent Test**: On the Linux host against a mock OpenAI-compatible server: a 30-min WAV pair gives chunks of ≤ 120 s ending at pauses, a pause-less 3 min is cut hard at 120 s, the `verbose_json` → `json` retry works, the merged transcript validates against schema 1, and silence/music tracks give 0 lines. On the owner's PC: WER and the one-hour timing with the owner's key.

**Acceptance Scenarios**:

1. **Given** a stopped call and a configured API engine, **When** the final pass ends, **Then** the call folder holds `transcript.json` (valid against schema 1, requirements §7) and `transcript.md` (time, speaker, text), `meta.json` says `ready`, and the notice "Transcript ready — Open" opens the call in the Calls section (req FR-60, FR-64, FR-81).
2. **Given** a 30-min track, **Then** every uploaded chunk is ≤ 120 s of 16 kHz mono audio and ends at the last speech pause before 120 s where one exists; **Given** 3 min of speech without a pause, **Then** it is cut hard at 120 s (req FR-60, `(v7, r4#2)`).
3. **Given** a 1 h track, **Then** no upload exceeds the provider's limit (25 MB for OpenAI) and the lines' times stay in call order (req FR-60, r3#4).
4. **Given** a model that returns no segment times, or rejects `verbose_json` with a 4xx, **Then** that chunk is sent again with `json`, and it and every later chunk of the pass are cut into single speech phrases of ≤ 30 s, each line timed by its chunk's start and end (req FR-60, `(v7, r4#3)`).
5. **Given** the user interrupts the other party, **Then** both lines are in the transcript in start order, the overlap kept (req FR-63).
6. **Given** the dictation speech language is Russian, **Then** every final request carries Russian; **Given** an engine that returns a segment's language and an English segment, **Then** that segment has `lang = en`; **Given** an engine that returns one language per request, **Then** each segment of the chunk carries it; **Given** an engine that returns none, **Then** `lang = null` (req FR-59, FR-60, `(v7, r4#21)`).
7. **Given** 2 min of silence and 2 min of music without vocals on the system track, **Then** no request is made for them and the transcript has 0 lines for them (req FR-45).
8. **Given** the dictation engine is built-in and the API engine has no key, **When** the final pass starts, **Then** it fails per FR-65 with "set up the API engine" (req FR-60, r3#11).
9. **Given** the API engine's base URL host is not loopback (e.g. `api.openai.com` or `192.168.1.5`), **Then** the Calls settings tab shows "the other party's audio goes to provider <host>", and the first call-mode session that uses this host shows it once; **Given** `http://localhost:8000/v1`, **Then** no warning (req FR-67, `(v7, r4#11)`).
10. **Given** a one-hour call on the owner's PC with OpenAI `whisper-1` or Groq `whisper-large-v3-turbo` on ≥ 20 Mbit/s, **Then** the final transcript is ready ≤ 5 min after stop (req NFR-13 API final, CA-3).
11. **Given** the API does not answer a chunk within the FR-24 API timeout, **Then** FR-65 applies with reason "timeout" (req FR-82).

---

### User Story 3 - Read, search, copy and export a call (Priority: P1)

The owner opens "Calls" from the tray. A window lists every call — title ("Call DD.MM HH:MM"), date and time, duration, state — newest first. Opening a call shows its transcript with time, speaker (colour and label) and text; a search highlights matches. "Copy" puts a readable Markdown transcript on the clipboard without it entering Windows clipboard history; "Export" writes Markdown or JSON to a chosen place; "Open folder" opens the call folder.

**Why this priority**: "Read" is the done-when; copying and exporting are how the transcript reaches agents (requirements §1 release-2 JTBD).

**Independent Test**: Playwright with mocked IPC: list, empty list, states including "Queued" and `damaged`, view, search with and without matches, copy, export failure. On the Linux host: the export writes nothing partial. On Windows CI: clipboard exclusion of the copied transcript.

**Acceptance Scenarios**:

1. **Given** 3 calls, **When** the Calls window opens, **Then** 3 rows, newest first, each with title, date and time, duration and state (req FR-71).
2. **Given** no calls, **Then** "No calls yet" with how to start one (req FR-71, r3#9).
3. **Given** a call whose `meta.json` says `ready-to-transcribe`, or whose pass waits in the queue, **Then** its row shows "Queued" (req FR-71, `(v7, r4#15)`).
4. **Given** an open call, **When** the user searches for "contract", **Then** the matching lines are highlighted; **When** nothing matches, **Then** "No matches" (req FR-72).
5. **Given** "Copy", **Then** the clipboard holds a readable transcript with speaker names and Win+V history does not list it (req FR-76, r3#33).
6. **Given** an export to a read-only folder, **Then** the reason is shown and no partial file remains (req FR-76, r3#9).
7. **Given** a call row, **When** "Open folder" is chosen, **Then** Explorer opens that call folder (req FR-76).

---

### User Story 4 - Never lose a recording (Priority: P1)

The app crashes, Windows shuts down, the storage drive is pulled, the disk fills, the PC sleeps, or the call runs for four hours. In every case what was recorded is kept: at most the last 10 s are lost to a crash; a clean end (exit with "Yes", installer close, shutdown, logoff, sleep, 4 h limit, low disk) saves the call and its final pass runs; an unfinished call is found at the next start and offered for transcription.

**Why this priority**: Success criterion 1 ("none is lost or damaged") and 5 (an engine failure never loses the recording); decisions #170 rules out silent loss.

**Independent Test**: On the Linux host with fake sources, a fake filesystem and fake clocks: truncation and recovery of the track files, a `recording` folder with no owning session listed as `recovered`, the disk thresholds, the 4 h stop, write failures. On Windows CI: a forced process kill during a WAV-backed recording and the next start's recovery; `WM_ENDSESSION` close.

**Acceptance Scenarios**:

1. **Given** the process crashes at minute 20, **When** the app starts again, **Then** the call is listed as `recovered` ("recovered after a crash") with ≥ 19 min 50 s of readable audio and the offer to transcribe; the pass does not start by itself (req FR-47, FR-49, FR-83).
2. **Given** a recording, **When** the user exits from the tray, **Then** a confirmation is asked; "Yes" saves the call and its final pass runs; "No" keeps the app running and recording (req FR-38).
3. **Given** a recording, **When** the installer closes the app or Windows shuts down or logs off (`WM_ENDSESSION`), **Then** no question is shown, the files are readable, `meta.json` says `ready-to-transcribe`, and at the next start the final pass starts by itself (req FR-38, `(v7, r4#8)`).
4. **Given** a recording, **When** the process is ended by force, **Then** the next start lists the call as `recovered` with the offer (req FR-38, FR-49).
5. **Given** the storage root's drive is removed at minute 15, **Then** the recording stops, the notice gives the reason, the log records the storage error, and when the drive returns the call is listed as `recovered` with the flushed audio and offered (req FR-47, `(v7, r4#5)`).
6. **Given** free space on the storage root falls below 1 GB, **Then** a warning is shown in the indicator and as a toast; **below 200 MB**, **Then** the recording stops with saving and the files are readable (req FR-50, CA-6).
7. **Given** a recording reaches 4 h, **Then** it stops with saving and notifies "maximum length reached" (req FR-50).
8. **Given** the disk is full after the stop, **When** the final pass writes `transcript.json`, **Then** the call is `failed: storage` with "Transcribe again" (req FR-50, `(v7, r4#4)`).
9. **Given** the PC sleeps at minute 5, **When** it wakes, **Then** the call is saved, its pass runs and a notice says so; **Given** a 5-min screen lock, **Then** the recording is continuous (req FR-52).
10. **Given** a crash during a dictation leaves stale dictation temp audio, **When** the app starts, **Then** the dictation cleanup deletes that audio and never touches a call folder (req NFR-06 `(v6, r3#1)`).

---

### User Story 5 - Recover from a failed final pass (Priority: P2)

The final pass fails — the key is wrong, the network drops, the provider times out. The call keeps its audio, shows "failed" with the reason, and "Transcribe again" retries only the track that failed. Passes run one at a time; the user can record the next call while one is transcribing.

**Why this priority**: FR-65/FR-83 are Must failure branches of US2; separated because they can be built and tested after the happy path.

**Independent Test**: On the Linux host with the mock server returning 401, timeouts and per-track failures; the pass queue with two calls; exit during a pass and restart.

**Acceptance Scenarios**:

1. **Given** an invalid API key, **When** the pass runs, **Then** the call shows "failed: key" and the audio stays; **When** the key is fixed and "Transcribe again" is chosen, **Then** the transcript is produced (req FR-65).
2. **Given** the mic track succeeds and the system track fails, **Then** the call is `failed`, the mic result is kept, and "Transcribe again" sends only the system track (req FR-83, `(v7, r4#16)`).
3. **Given** a ready call, **Then** "Transcribe again" is offered and redoes both tracks; the speaker names are kept (req FR-65, FR-83, §7 `(v7, r4#17)`).
4. **Given** call A is transcribing, **When** call B is recorded and stopped, **Then** B is listed as "Queued" and starts after A (req FR-83).
5. **Given** the app exits or crashes during a pass, **When** it starts again, **Then** that pass restarts by itself (req FR-83).
6. **Given** a failed pass, **Then** the notice gives the reason per FR-65 instead of "Transcript ready" (req FR-81).
7. **Given** an engine error of any kind during the pass, **Then** the app does not crash and the audio is unchanged (req NFR-15).

---

### User Story 6 - Keep recording through device changes and stay warned (Priority: P2)

During a call the owner plugs in a headset, a Bluetooth headset switches profile, or the USB microphone falls out. The recording continues on the new or default device; gaps are filled with silence so times do not shift; the levels of both tracks are visible, and warnings ("Can't hear the other party", "Can't hear you", communications device differs, track lost, microphone changed, low disk) stay in the indicator until dismissed, even when Windows holds the toasts.

**Why this priority**: Real calls hit device changes (NFR-19); without these the recording silently misses a side. P2 because a stable-device call already works with US1.

**Independent Test**: On the Linux host with fake sources and fake clocks: drift at 50 ppm over 60 min, a device change gap, a lost track filled with silence, the 60 s silence warnings. Playwright: the indicator's warning list. On the owner's PC: real device switches and the NFR-19 short set.

**Acceptance Scenarios**:

1. **Given** fake sources drifting 50 ppm, **Then** at minute 60 the timestamps of both tracks differ by ≤ 100 ms (req FR-40, CA-4).
2. **Given** Bluetooth headphones are connected mid-call, **Then** capture continues from the new default output with a gap of ≤ 2 s filled with silence, and `meta.json` and the log record the change with its gap in ms (req FR-41, FR-84).
3. **Given** the selected USB microphone is unplugged mid-call, **Then** the recording continues from the Windows default input and the notice is shown; the call is not stopped (req FR-42).
4. **Given** the loopback stream dies at minute 10 and cannot be reopened within 2 s (or no input device remains), **Then** the recording continues, that track holds silence from minute 10, the indicator shows the lost track and the log records it (req FR-39, FR-42, r3#5).
5. **Given** a headset is the default communications device and speakers the default output, **When** recording starts, **Then** the indicator warns (req FR-39, r3#16).
6. **Given** the call audio goes to a non-default device so the system track stays silent > 60 s while the mic carries speech, **Then** "Can't hear the other party — check the output device"; **Given** the mic is muted > 60 s while the other party talks, **Then** "Can't hear you — check the microphone" (req FR-46).
7. **Given** Windows Do Not Disturb holds toasts during a full-screen call, **When** a call-time warning fires, **Then** it stays visible in the indicator until the user dismisses it (req FR-34, `(v7, r4#6)`).
8. **Given** a browser in full screen, **Then** the indicator is visible above it, can be moved, and its position is remembered (req FR-34).

---

### User Story 7 - Dictate during a call (Priority: P2)

While a call records, the owner dictates a message into the call's chat. Dictation works as usual; the dictated phrase is also in the call's mic track and appears as "Me" in the transcript. Esc cancels only the dictation, never the call.

**Why this priority**: Decisions #169; a Must, but it extends existing dictation rather than adding a new flow.

**Independent Test**: In Windows CI with WAV sources: a dictation during a call opens its own mic stream, pastes, and the call track is continuous. On the Linux host: the tray-state precedence. On the owner's PC: the NFR-02 start timing during a call.

**Acceptance Scenarios**:

1. **Given** a recording runs, **When** the user dictates a phrase into a chat, **Then** the text is pasted and the call transcript holds the phrase as "Me" (req FR-36).
2. **Given** a recording runs, **When** a dictation starts, **Then** hotkey press → dictation capture start stays p95 ≤ 200 ms on a wired or USB microphone (req NFR-02 `(v6, r3#2)`).
3. **Given** a call records and a dictation starts, **Then** the tray shows the dictation-recording state, and the call-recording state returns when the dictation ends (req FR-34, r3#18).
4. **Given** a call recording and no dictation, **When** Esc is pressed, **Then** Esc reaches the focused application and the call continues; **Given** a dictation during a call, **When** Esc is pressed, **Then** only the dictation is discarded (req FR-22 `(v6, r3#18)`).
5. **Given** call mode is off and no dictation records, **Then** the Windows "microphone in use" indicator is off (req NFR-02).

---

### User Story 8 - Manage calls: rename, delete, damaged folders (Priority: P2)

The owner renames a call ("Call 22.10 14:05" → "Weekly sync") and the other party ("Other party" → "Colleague"); the names reach the list, the view and the exports. They delete calls they no longer need, and a broken call folder is still shown so it can be opened or deleted.

**Why this priority**: Must requirements, but the done-when holds without them.

**Independent Test**: Playwright with mocked IPC for the rename and delete flows and the `damaged` row; on the Linux host, `meta.json` as the source of names, rewriting of `transcript.*`, delete with a locked file (fake filesystem) and cancel of a running pass.

**Acceptance Scenarios**:

1. **Given** a rename of "Other party" to "Colleague", **Then** the view, `transcript.md` and `transcript.json` show "Colleague"; **Given** a rename to "   ", **Then** it is refused and the old name stays; **Given** both speakers renamed "Client", **Then** both keep the name (req FR-73).
2. **Given** a call renamed "Weekly sync", **Then** the list and `transcript.json` `title` show it; **Given** an empty title, **Then** it is refused and the old title stays (req FR-75).
3. **Given** a ready call, **When** it is deleted after confirmation, **Then** its folder no longer exists (req FR-77, NFR-17).
4. **Given** the call that is recording, **Then** "Delete" is unavailable; **Given** a transcribing call, **When** it is deleted, **Then** the pass is cancelled and the folder is gone (req FR-77).
5. **Given** `mic.opus` is open in another program, **When** the call is deleted, **Then** "could not delete: mic.opus is in use" and the folder is unchanged (req FR-77).
6. **Given** a call folder whose `meta.json` is corrupt or whose tracks are missing, **Then** the row shows `damaged` with "Open folder" and "Delete", never hidden (req FR-71, r3#9).

---

### User Story 9 - Configure call mode in the Calls settings tab (Priority: P2)

The owner opens the Calls settings tab: the call hotkey, the reminder on/off, the primary language, the storage folder, and the provider warning for the API engine. Changes apply without a restart. Moving the storage folder applies to new calls; earlier calls stay listed.

**Why this priority**: The defaults let US1–US3 work without visiting the tab; the tab is needed for the refusals of US1 and for moving the folder.

**Independent Test**: Playwright with mocked IPC for the tab, its refusals and the call-hotkey conflict mark; on the Linux host, settings validation, earlier roots and listing across roots.

**Acceptance Scenarios**:

1. **Given** a changed Calls setting is saved, **Then** it takes effect without a restart and persists across restarts (req FR-80).
2. **Given** the user sets the call hotkey to Ctrl+Alt+Space while that is the dictation hotkey, **Then** save is refused; **Given** the call hotkey is Ctrl+Alt+R, **When** the dictation hotkey is set to Ctrl+Alt+R, **Then** save is refused (req FR-33, `(v7, r4#13)`).
3. **Given** the call hotkey is taken by another program at startup, **Then** a notice only: no settings window opens, the tray shows no hotkey-error state, dictation works, and the Calls tab marks the call hotkey (req FR-33).
4. **Given** a storage folder on a read-only or missing drive, **Then** save is refused with the field highlighted and the reason (req FR-80, r3#7).
5. **Given** 3 calls in the default root, **When** the folder is changed to `D:\Calls` and a call is recorded, **Then** the new call is in `D:\Calls` and all 4 calls are listed (req FR-80, FR-71, `(v7, r4#12)`).
6. **Given** a fresh install, **Then** the primary language equals the UI language and is only stored (req FR-59, FR-80).

---

### User Story 10 - Uninstall never silently deletes calls (Priority: P2)

A user uninstalls Voicen. The uninstaller asks separately "remove recorded calls in <folder>?", default keep. A silent or passive uninstall keeps the calls; only `/PURGECALLS` removes the default calls folder; a moved storage root is never deleted.

**Why this priority**: Decisions #170 and #180 (a silent uninstall today would delete every recorded call); verified once per change in Windows CI rather than in daily use.

**Independent Test**: In Windows CI, plant a `calls\<folder>`, run `uninstall.exe /S`, check it survives; `/S /PURGECALLS` removes it.

**Acceptance Scenarios**:

1. **Given** uninstall with "yes" to the first question and the default answer to the calls question, **Then** `%LOCALAPPDATA%\Voicen\calls` and its call folders remain (req FR-28 `(v6, r3#1)`).
2. **Given** `uninstall.exe /S` (or `/P`) with a planted call folder, **Then** the calls folder and the call folder remain; **Given** `/S /PURGECALLS`, **Then** the default calls folder is removed (req FR-28 `(v7, r4#1)`).
3. **Given** a storage root moved to `D:\Calls`, and an earlier root, **Then** both remain whatever the answers (req FR-28 `(v7, r4#12)`).
4. **Given** an upgrade, **Then** nothing is asked and no call is removed (req FR-28).

---

### Edge Cases

- **Exit during a final pass (not recording)**: no question; the pass restarts at the next start (req FR-83).
- **Record turned on while a pass runs**: allowed; the new call records and its pass is queued (req FR-83).
- **Dictation hotkey and call hotkey pressed together**: independent; equal hotkeys cannot be saved (req FR-33).
- **Call hotkey pressed while call mode is starting or stopping**: one session only; the press applies to the current session (req FR-37).
- **Track opens but the other fails at start**: neither is kept open; no call folder remains from the refused start (req FR-30, no one-track recording).
- **Storage root unavailable when listing** (an earlier root on an unplugged drive): see Assumptions; its calls reappear when the drive returns, as FR-47 expects.
- **Clock change or time-zone change during a call**: track times come from the capture clock, not the wall clock; only the folder name and `started_at` use the start's wall time.
- **Two calls started in the same second**: impossible (one session, FR-37); a folder name collision with a leftover folder gets a distinct name rather than overwriting.
- **A call folder edited or partly deleted by the user**: listed as `damaged` (req FR-71).
- **`transcript.*` exists but `meta.json` says `transcribing`** after a crash during the pass: the pass restarts (req FR-83) and replaces the files only when it succeeds.
- **The API engine settings change while a pass runs**: the running pass keeps its settings; the next pass or "Transcribe again" uses the new ones (FR-13 "takes effect" applies to the next job, as FR-24 does).
- **Very short call (< 1 s, or no speech at all)**: saved; the pass makes no request for speechless audio and the transcript has 0 lines; the view says the call has no speech.
- **The provider rejects a chunk as too large or of a bad format** (4xx other than `response_format`): the track fails per FR-65 with the reason.
- **Log directory not writable during a call**: FR-20's failure branch; the recording is unaffected (req FR-84).
- **Device names**: the log holds endpoint IDs only, never friendly names (req FR-84, `(v7, r4#20)`); `meta.json` may hold names (it is the user's own data, §7).
- **UI language**: every new string (tray items, indicator, notices, Calls window, Calls tab, warnings, uninstaller question) exists in English and Russian (req FR-15); the default speaker labels are "Me"/"Other party" and "Я"/"Собеседник".
- **Subtitles-only in 1a**: the user cannot start it (no Subtitles toggle until 1b); the session still guarantees that without Record nothing is written (req FR-51, see Assumptions).

## Requirements *(mandatory)*

### Functional Requirements

**Call session and control**

- **FR-001** (req FR-30, FR-31, FR-37, decisions #167): The system MUST hold one call session with the two toggles Record and Subtitles; call mode is active while at least one is on and is off at every app start. A repeated activation MUST apply to the current session and never open a second capture or call folder. Turning Record off ends the call; turning it on again starts a new call in a new folder.
- **FR-002** (req FR-30, r3#5, r3#7, `(v7, r4#22)`): Before Record starts, the system MUST open both tracks and check the storage root; if either track cannot open or the root is missing or unwritable, the start MUST be refused with a notice and a log line naming the track or the root and the reason, with no one-track recording and no call folder left behind; an unwritable root also opens the Calls settings tab. A missing selected microphone with a default input present starts on the default input with a notice.
- **FR-003** (req FR-32, FR-71, r3#10): The tray menu MUST have "Call" with "Record" as a check item and "Stop", and "Calls", which opens the Calls window. The command-line switch `voicen.exe --call record` (decisions #184) MUST be handed to the running instance and start Record through the same session path as the tray (FR-001, FR-002, FR-005).
- **FR-004** (req FR-33, FR-25, FR-34, `(v7, r4#7, r4#13)`): The call hotkey (default Ctrl+Alt+R) MUST start call mode with the last used toggle combination (Record only in 1a) and stop it on the next press. Equal call and dictation hotkeys MUST be refused on save from either side. A call-hotkey conflict MUST notify, mark the hotkey in the Calls tab, and MUST NOT open settings, set the tray hotkey-error state or affect the dictation hotkey.
- **FR-005** (req FR-34, FR-35, r3#27, OQ-52 (a)): While recording, the system MUST show the call indicator — a new interactive, movable window above other windows, position remembered; the dictation overlay is unchanged — with "● Recording" and a timer, the levels of both tracks, a stop control, and at each start the reminder and the headphones hint for 10 s (reminder setting, default on).
- **FR-006** (req FR-34, r3#18, `(v7, r4#6)`): The tray MUST have a call-recording state with the precedence dictation hotkey error > dictation recording > call recording > error > idle. Every call-time warning (FR-39 communications device and lost track, FR-42, FR-46, FR-50) MUST be shown in the indicator until dismissed and as a toast.
- **FR-007** (req FR-38, `(v7, r4#8)`): Exit while recording MUST ask for confirmation ("Yes" saves and stops, "No" keeps recording). An installer close, OS shutdown or logoff MUST stop with saving without a question, leaving `meta.json` in `ready-to-transcribe`; such a call's pass MUST start by itself at the next start.
- **FR-008** (req FR-52): Sleep while recording MUST end the call as a stop and notify on wake; a screen lock MUST NOT interrupt the recording.
- **FR-009** (req FR-36, NFR-02 `(v6, r3#2)`, FR-22 `(v6, r3#18)`): Dictation MUST keep working while call mode is active, on its own microphone stream on the same device, with the NFR-02 start figure. Esc MUST affect only a dictation recording. The microphone and the loopback MUST be open only while a dictation records or call mode is active.

**Capture**

- **FR-010** (req FR-39, decisions #160, r3#16): The system MUST capture two never-mixed tracks: `mic` (the microphone selected in Voicen settings, speaker "Me") and `system` (loopback of the default console output device, speaker "Other party"). It MUST warn in the indicator at start when the default communications device differs from the default output device.
- **FR-011** (req FR-40, CA-4): Both tracks MUST be timed from one clock and the drift between the two devices compensated, so that misalignment at minute 60 is ≤ 100 ms under 50 ppm drift.
- **FR-012** (req FR-41, FR-42, FR-39, r3#5): On a default-output change the system track MUST continue from the new device with a gap ≤ 2 s; on a microphone loss or change the mic track MUST continue from the Windows default input with a notice; a track lost for another reason and not reopened within 2 s (or with no input device left) MUST continue as silence, shown in the indicator. Every gap MUST be filled with silence so later times do not shift, and every change recorded in `meta.json` and the log.
- **FR-013** (req FR-46, r3#17): The system MUST warn "Can't hear the other party — check the output device" after > 60 s of system-track silence while the mic carries speech, and "Can't hear you — check the microphone" after > 60 s of mic silence while the system track carries speech.
- **FR-014** (req NFR-22): Each track's source MUST be replaceable by a WAV file, so the whole chain capture → storage → final → merge runs on the Linux host and in Windows CI without audio devices.

**Storage and recovery**

- **FR-015** (req FR-47, FR-48, §7, NFR-18, OQ-47 (a)): While Record is on, the system MUST write `mic.opus` and `system.opus` (Ogg Opus, mono 16 kHz) incrementally into `<root>\<YYYY-MM-DD_HHMMSS>\`, flushed at least every 10 s, with `meta.json` from the start (§7 states and fields); at stop it MUST close both files and start the final pass. Both tracks together MUST stay ≤ 40 MB per hour (CA-9).
- **FR-016** (req FR-47, `(v7, r4#5)`, r3#7): On a write failure mid-recording, the system MUST stop the recording, keep what was flushed, notify with the reason and log the storage error, leaving `meta.json` in `recording`.
- **FR-017** (req FR-49, FR-83, `(v7, r4#5, r4#16)`): At start, every call folder whose `meta.json` says `recording` and that no running session owns MUST be recovered (readable tracks up to the last flushed page), marked `recovered` ("recovered after a crash") and offered for transcription, never started by itself.
- **FR-018** (req FR-50, CA-6, `(v7, r4#4)`): The system MUST stop with saving at 4 h ("maximum length reached"), warn when free space on the storage root falls below 1 GB, and stop with saving below 200 MB. The final pass MUST decode one chunk at a time in memory and write no temp files (this bounds decoding, not requests: at most 3 chunk requests may be in flight, decisions #187); a failed `transcript.*` write MUST leave the call `failed: storage` with "Transcribe again".
- **FR-019** (req FR-51, NFR-17, decisions #164): The session MUST write no call audio or text to disk while Record is off, whatever the other toggle; this is enforced in the session itself, not by the absence of the Subtitles toggle in 1a.
- **FR-020** (req NFR-06 `(v6, r3#1)`): The dictation temp-audio cleanup MUST never touch a storage root or call folder.
- **FR-021** (req NFR-16, r3#24): Calls MUST be stored unencrypted, readable by the Windows user; the docs MUST say so.

**Final pass**

- **FR-022** (req FR-60, r3#11, OQ-53 (a), `(v7, r4#3)`): In 1a the final engine MUST be the API engine of the dictation settings (base URL, model, key slot) with the dictation speech-language setting, whatever dictation engine is active. If it is not usable (no key, no base URL), the pass MUST fail per FR-65 with "set up the API engine".
- **FR-023** (req FR-60, FR-45, `(v7, r4#2)`, decisions #175): The pass MUST recognise each whole track again (live subtitles never enter it), cutting it into chunks of at most 120 s at the last speech pause before 120 s, hard at 120 s when there is none, using the bundled Silero detector so that silence and music without vocals are never sent and give no lines; each chunk MUST be uploaded as 16 kHz mono WAV. If Silero cannot be loaded, the pass goes on with energy spans and logs `vad_fallback` (decisions #188); in that case music or noise may be sent, and SC-008 holds only with Silero.
- **FR-024** (req FR-60, r3#4, `(v7, r4#3)`): Each chunk MUST ask for segment times; a line is an engine segment, placed at chunk start + segment time. A 4xx rejecting the response format, or a response without segment times, MUST make the engine time-less for the rest of the pass: that chunk is sent again without segment times, and it and every later chunk are cut into single speech phrases of ≤ 30 s, each line timed by its chunk's start and end.
- **FR-025** (req FR-59, r3#20, `(v7, r4#21)`, OQ-49 (a)): The pass MUST store each segment's language in `segments[].lang` when the engine returns it (per segment, or per request applied to each segment of the chunk), else `null`.
- **FR-026** (req FR-63, FR-64, §7): The pass MUST merge the lines of both tracks by start time, keeping overlaps, and save `transcript.json` (schema 1, validated before it is written; `speaker` `me` for mic and `s1` for system, `source` `final`) and `transcript.md` (time, speaker name, text); names come from `meta.json`.
- **FR-027** (req FR-65, FR-83, r3#15, `(v7, r4#16, r4#17)`): A failed pass MUST keep the audio and show `failed` with the reason (key, network, timeout, provider error, storage); each track's result MUST be kept and the call stays `failed` until both succeed. "Transcribe again" MUST be offered on failed calls (redoing only the failed track) and on ready calls (redoing both), keeping all speaker names in 1a.
- **FR-028** (req FR-83, `(v7, r4#15, r4#16)`): Passes MUST run one at a time in a queue, a new call may record while a pass runs, a queued or `ready-to-transcribe` call is shown as "Queued", and an exit or crash during a pass MUST restart that pass at the next start.
- **FR-029** (req FR-82, FR-24, `(v7, r4#2)`): Each chunk request MUST use FR-24's API connect and transcription timeouts unchanged; a timeout fails the track per FR-027 with reason "timeout".
- **FR-030** (req FR-81, r3#10): At stop the system MUST notify "Call saved — transcribing"; on success "Transcript ready" with "Open", which opens the call in the Calls window; on failure a notice with the FR-65 reason instead.
- **FR-031** (req FR-67, FR-29, r3#35, `(v7, r4#11)`): When the 1a final engine's base URL host is not loopback, the Calls settings tab MUST show "the other party's audio goes to provider <host>", and the first call-mode session that uses a newly configured cloud host MUST show it once.
- **FR-032** (req NFR-15): No capture, storage or engine failure during a recording or a pass may crash the app or change the recorded audio.

**Calls window**

- **FR-033** (req FR-71, `(v7, r4#12, r4#15)`, r3#9): The Calls window MUST list the calls of the current storage root and every earlier root, newest first, with title (default "Call DD.MM HH:MM", local time), date and time, duration, and state (recording, queued, transcribing, ready, failed with reason, recovered, damaged); an empty list shows "No calls yet" with how to start one; an unreadable folder is listed as `damaged` with "Open folder" and "Delete".
- **FR-034** (req FR-72): Opening a call MUST show its lines with time, speaker (colour and label) and text, and a search that highlights matches or says "No matches".
- **FR-035** (req FR-73, FR-75, r3#9, r3#37): The user MUST be able to rename a speaker within a call and rename a call; empty or whitespace-only names are refused; duplicate speaker names are allowed; the change is stored in `meta.json` and applied to `transcript.json`, `transcript.md` and exports.
- **FR-036** (req FR-76, r3#33, r3#9): The user MUST be able to copy the transcript as Markdown (clipboard excluded from history and cloud sync as in FR-10), export Markdown or JSON to a chosen place with no partial file on failure and the reason shown, and open the call folder.
- **FR-037** (req FR-77, NFR-17): The user MUST be able to delete a call after confirmation, removing the whole call folder; the recording call cannot be deleted; deleting a transcribing call cancels its pass first; if a file is in use, "could not delete: <file> is in use" and nothing is deleted.

**Settings, installer, logs, docs**

- **FR-038** (req FR-59, FR-80): Settings MUST hold the primary language (default the UI language), stored only.
- **FR-039** (req FR-80, r3#7, `(v7, r4#12)`): Saving a storage folder that cannot be written MUST be refused with the field highlighted and the reason; a new folder applies to new calls, earlier roots are remembered and stay listed.
- **FR-040** (req FR-80 partly): The Calls settings tab in 1a MUST hold: the call hotkey, the reminder on/off, the primary language, the storage folder, and the provider warning of FR-031; every change applies without a restart and persists. The other FR-80 items arrive with their increments (subtitle and final engines, subtitle window: 1b; echo, quick final, summary timeout, auto-delete: 1d).
- **FR-041** (req FR-28 `(v6, r3#1)`, `(v7, r4#1, r4#12)`): The uninstaller MUST ask separately "remove recorded calls in <folder>?" (default keep), never remove the calls folder through the first question, answer keep in a silent or passive uninstall unless `/PURGECALLS` is given, and never delete any storage root other than the default `%LOCALAPPDATA%\Voicen\calls`; Windows CI MUST prove that a planted call folder survives `/S`.
- **FR-042** (req FR-84, FR-20, NFR-16, `(v7, r4#20)`): The log MUST record call-mode events — session start and stop with toggles and device endpoint IDs, device changes with the gap in ms, final-pass stage durations per track, recoveries, storage errors, refusals — and never transcript text, audio, keys or device friendly names.
- **FR-043** (§9 "deployed", FR-30, NFR-22): The Windows install smoke MUST switch call-mode Record on in the installed app on the device-less runner and check the log for the "no output device" refusal and that the app still runs; the indicator is not shown at start and is a product window, never a listed helper. The trigger is a command-line switch handed to the running instance (`voicen.exe --call record`; decisions #184), which is also the user-facing way to start Record; the smoke uses it unchanged. A switch-started recording follows every rule of a tray start (FR-002 refusals, FR-005 indicator always shown, FR-006 tray state, reminder). Trade-off accepted by the owner: any local process running as the user can start a recording this way; the always-visible indicator and tray state are the safeguard (decisions #165: no secret mode).
- **FR-044** (req NFR-21, r3#26): Every new bundled component (Opus/Ogg encoding and anything else added in 1a) MUST have an MIT-compatible licence and be listed in THIRD-PARTY-NOTICES; each new dependency needs the owner's consent when 1a starts (OQ-63 (a); given for `opus`, `opusic-sys`, `ogg`, the `windows` features and `cmake`, decisions #186).
- **FR-045** (decisions #165, r3#34): The README (the project's help) MUST have a recording-consent section in English and Russian: the user is responsible for the other party's consent, with a sample opening phrase; and the statement of FR-021.
- **FR-046** (req FR-15): Every user-visible string added by this feature MUST exist in English and Russian.

**Measurement**

- **FR-047** (req FR-60, CA-7, OQ-65, `(v7, r4#14)`): The final's WER MUST be measured on the owner's PC with the owner's key, outside CI: Russian on the owner's 2 × 10-min calls transcribed word for word by the owner; English on a named open-corpus subset (≥ 30 min, fetched by checksum, licence verified), which is a pinned subset of LibriSpeech test-clean (CC BY 4.0), at least 30 min, fetched by checksum (decisions #185, OQ-65 (a)). Read speech makes this English WER optimistic compared with real calls; the owner's Russian call set covers real call audio.

### Key Entities

- **Call**: one session with Record on — id, title, start, end, duration, state (`recording` → `ready-to-transcribe` → `transcribing` → `ready` / `failed` with reason / `recovered`; listed as "Queued" or `damaged`), devices and device changes, engine used, app version, speaker names. Its `meta.json` is the source of state and names.
- **Call folder**: `<root>\<YYYY-MM-DD_HHMMSS>\` with `mic.opus`, `system.opus`, `meta.json`, `transcript.json`, `transcript.md` (§7).
- **Track**: `mic` ("Me") or `system` ("Other party"); its file, rate, start offset; gaps filled with silence.
- **Chunk**: a ≤ 120 s piece of one track for one request (≤ 30 s single phrase once the engine is time-less), with its start offset.
- **Segment (line)**: start_ms, end_ms (relative to the call start), track, speaker (`me` / `s1`), lang or null, text, source `final`.
- **Speaker**: `me` and, in 1a, one `s1` ("Other party"), each with a display name per call.
- **Final pass**: per call, per track result (pending, done, failed with reason), queued one at a time; restarts after an exit or crash during it.
- **Storage root**: the current root plus remembered earlier roots; only the default root is ever deleted by the uninstaller, and only with `/PURGECALLS` or an explicit answer.
- **Call settings (1a part)**: call hotkey, last toggle combination, reminder on/off, primary language, storage folder and earlier roots, cloud hosts already warned about.
- **Call warning**: kind (communications device, lost track, microphone changed, can't hear you, can't hear the other party, low disk), shown until dismissed.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The owner records a real one-to-one call and reads its transcript with "Me" and "Other party" lines in the Calls window (the 1a done-when, requirements §3).
- **SC-002**: The owner's real calls of 30–90 min recorded in 1a (Telegram Desktop, a browser meeting, Zoom or Teams) are 100 % present in the Calls window with readable audio; none is lost or damaged (success criterion 1, checked for the calls made in 1a).
- **SC-003**: For a one-hour call, the final transcript is ready ≤ 5 min after stop via OpenAI `whisper-1` and via Groq `whisper-large-v3-turbo` on ≥ 20 Mbit/s, measured from the log timings on the owner's PC (success criterion 4 API, NFR-13, CA-3).
- **SC-004**: The final's WER is ≤ 15 % on Russian and ≤ 10 % on English on the sets of FR-047 (CA-7).
- **SC-005**: After a forced crash at minute N, the recovered call holds at least N min − 10 s of audio (FR-47).
- **SC-006**: In 100 % of fault-injection runs (capture loss, storage failure, engine errors, timeouts), the app keeps running and the recorded audio is unchanged (NFR-15, success criterion 5).
- **SC-007**: Both tracks of a 60-min recording together take ≤ 40 MB (NFR-18, CA-9), and the tracks are aligned within ≤ 100 ms at minute 60 under 50 ppm drift (FR-40, CA-4).
- **SC-008**: Silence and music without vocals produce 0 transcript lines (FR-45).
- **SC-009**: After a session with a call, a device change and a failed pass, the logs and crash files contain 0 occurrences of transcript text, audio, keys or device friendly names (FR-84, NFR-16).
- **SC-010**: A silent uninstall keeps 100 % of call folders in the default root, and no uninstall ever removes a moved or earlier root (FR-28).
- **SC-011**: The owner's short NFR-19 set (their real call apps and headset) passes for this delivery; the full matrix is run once before release 2 is final (NFR-19).

## Assumptions

- **No Subtitles toggle in 1a**: the §3 row says "Control call mode … 1a (Subtitles toggle: 1b)". The tray, the hotkey and the indicator expose Record only; the session's model still has both toggles, so FR-31, FR-37 and FR-51 are verified at core level with a fake subtitle consumer in 1a and through the UI in 1b. FR-67's "subtitles-only included" applies from 1b.
- **FR-74 (speaker merge)** falls inside the §3 1a range "FR-71…FR-77" but its own Release column says 2 / 1c; with one "Other party" there is nothing to merge in 1a, so it is left to 1c.
- **"Other party" is one speaker `s1` in 1a**; diarization into `s1…sN` comes in 1c. In 1a "Transcribe again" keeps all names (§7, `(v7, r4#17)`).
- **Provider warning in 1a**: the "newly selected cloud engine" of FR-67 is the host of the dictation API engine's base URL; changing it to a new non-loopback host shows the warning again at the next session.
- **Output device to capture** (FR-80, Could, no increment named): not in 1a; FR-39 and §8 fix the default console output device.
- **Earlier root unavailable** (e.g. an unplugged drive): the Calls window shows a line naming the unavailable folder instead of its calls; the calls reappear when the drive returns (consistent with FR-47's "when the drive returns"). A call folder inside an available root with bad contents is `damaged` (FR-71).
- **`--call record` when Voicen is not running**: the launched process becomes the primary instance and then applies the switch; FR-30's "call mode is off at app start" holds for every start without the switch. Only `record` is defined in 1a; a stop switch is not required (decisions #184 names only the start).
- **After sleep** (FR-52): call mode is off on wake; nothing restarts the recording by itself.
- **Chunk concurrency within one pass** and the chunk upload order are plan decisions, bounded by FR-83 (one pass at a time) and SC-003.
- **Consent help** lives in the README and the project's help docs; the in-app reminder text is FR-35's; no in-app link is required by the requirements.
- **Default speaker labels**: "Me"/"Other party" in English, "Я"/"Собеседник" in Russian (ADR-003, FR-15).
- **Verification placement** (requirements §9 release 2): session state machine, drift, Ogg Opus write/truncation/recovery, disk thresholds, write failures, FR-51, chunking and the `verbose_json` → `json` retry against a mock server, recovery of an unowned `recording` folder, merge, schema validation, delete, log cleanliness — Linux host with fakes; shell integration with WAV sources, the device-less refusal, indicator window style, Silero on the fetched model, uninstall `/S` keeps calls, the install smoke — Windows CI; Calls window, Calls tab, indicator content and warning list, "Queued" — Playwright with mocked IPC; real capture, device switches, Bluetooth, window behaviour above full-screen apps, NFR-13 and WER — the owner's PC (`verify_exception` where no kit adapter exists).
- **Open questions touching 1a** (`docs/open-questions.md`):
  - OQ-44 (open): the final is a separate pass, not glued subtitles — this spec follows FR-60, which already says so; the open part (a subtitle version as a quick final) is FR-61, 1d.
  - OQ-45 (open): Tauri overlay windows vs native windows for the subtitle window and the call indicator, spike first; the indicator (1a) is an interactive window per OQ-52 (a); its window mechanism is a plan decision, and the install smoke must treat it as a product window.
  - OQ-48 (open): the ADR-011 split of core modules; taken as the starting architecture for `/speckit-plan`, amendable there.
  - OQ-42 (open, 1c): track-based roles; 1a uses "Me" / "Other party" by track only, which OQ-42's options all keep.
  - OQ-65 answered (#185 (a)): LibriSpeech test-clean subset — FR-047.
  - OQ-58 (open, non-blocking): the package's stale facts; the repository wins.
  - OQ-64 answered (#183 (a)): 1a stays as written.
- **Dependencies**: T-102 (Silero in the installer) and T-007 (toasts) before the end of 1a; owner consent for the Opus/Ogg dependencies when 1a starts (OQ-63 (a), requirements §8; given, decisions #186); the owner's 2 × 10-min labelled calls, transcribed word for word, for the Russian WER; the owner's key for SC-003 and SC-004; release-1 parts reused as they are: the API engine client and key slots (features 001, 004), settings window (004), log (006), installer and uninstaller (006), tray and notifications (001).
