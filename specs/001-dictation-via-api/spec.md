# Feature Specification: Dictation via an OpenAI-compatible API

**Feature Branch**: `001-dictation-via-api`

**Created**: 2026-10-02

**Status**: Approved (clarifications confirmed by the owner 2026-10-02)

**Input**: User description: "The end-to-end dictation flow via an OpenAI-compatible transcription API: the app runs in the tray; a global hotkey (hold or toggle) records the selected microphone with a non-focus-stealing indicator and Esc to cancel; a voice-activity gate drops silence; the audio is sent to the configured OpenAI-compatible endpoint; the text goes to the clipboard and is pasted with Ctrl+V into the window focused at recording start; failures are reported (toast + overlay + tray state) with the audio kept for retry; results are delivered in recording order; the hotkey survives sleep/lock; a missing microphone falls back to the default device."

**Source of truth**: `docs/requirements.md` v3 (approved). Requirement ids in scope: FR-01, FR-02, FR-03, FR-04, FR-05, FR-06, FR-10, FR-11, FR-12, FR-22, FR-23, FR-24 (connect and API transcription timeouts), FR-25, FR-26, FR-27; NFR-01 (API part), NFR-02, NFR-06, NFR-07 (engine failures), NFR-08. Integration points only: FR-09 (post-processing, feature 003), FR-20 (log timings, feature 006). Every functional requirement below cites its source as `(req FR-NN)`.

## Clarifications

### Session 2026-10-02

- Q: What exactly does the 30 s API transcription timeout measure? → A: The total time of the request, from its start until the full response is received — connecting and uploading included; within it, the connection must be established within 5 s (connect timeout). Both values are defined once and shared with the other engines and post-processing. (confirmed by the owner 2026-10-02)
- Q: What happens if modifier keys are still held 1 s after the text is ready? → A: No Ctrl+V is sent; the text stays in the clipboard and "copied — paste manually" is shown — a paste with extra modifiers (e.g. Ctrl+Alt+V) could trigger a command in the target instead of pasting. (confirmed by the owner 2026-10-02)
- Q: Where does a retry take its place in the delivery order? → A: A retry is a new entry in the delivery order at the moment Retry is clicked: it is delivered after every recording already sent to an engine and before any recording that stops later; it is still delivered to its original start window. (confirmed by the owner 2026-10-02)
- Q: Does a retry use the settings at the time of the failure or the current ones? → A: The settings current at the moment Retry is clicked (engine, base URL, model, key, language), so the user can fix the cause — e.g. a wrong key or base URL — and retry the same audio. (confirmed by the owner 2026-10-02)
- Q: What does the overlay show between the end of recording and delivery? → A: A processing state while at least one dictation is being transcribed or post-processed and no recording is on; a new recording replaces it with the recording state; the overlay hides when nothing is recording or processing and no message is being shown. (confirmed by the owner 2026-10-02)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Dictate into the field I am typing in (Priority: P1)

The app runs in the background with a tray icon. The user focuses a text field in any application, holds the global hotkey, speaks, and releases it. The speech is checked for voice activity, sent to the configured OpenAI-compatible transcription endpoint, and the returned text is placed in the clipboard and pasted into the window that was focused when recording started. While recording, a small overlay and the tray icon show that recording is on, and the focused application keeps keyboard focus.

**Why this priority**: This is the one flow that must work end to end for release 1 (requirements §2); without it the product has no value.

**Independent Test**: With the API engine configured (base URL, model, key), focus Notepad, hold Ctrl+Alt+Space for 4 s while speaking, release — the spoken text appears in Notepad and is in the clipboard. In automation: the core pipeline with a fake audio source, a mock OpenAI-compatible server and a fake text sink delivers the mock transcript to the recorded start window.

**Acceptance Scenarios**:

1. **Given** the app is launched (not first run), **When** start-up completes, **Then** a tray icon is present and no window takes focus. (req FR-01)
2. **Given** the app is already running, **When** a second instance is launched, **Then** no second instance starts and the running instance brings its settings to front. (req FR-01)
3. **Given** mode = hold, hotkey = Ctrl+Alt+Space and Notepad focused, **When** the user holds the hotkey 4 s and releases it, **Then** about 4 s of audio is captured and processing starts. (req FR-02)
4. **Given** mode = hold, **When** the hotkey is held less than 0.3 s, **Then** the recording is discarded and nothing is sent. (req FR-02)
5. **Given** Notepad is focused, **When** recording starts, **Then** the overlay is visible, the tray icon is in its recording state, and Notepad keeps keyboard focus. (req FR-04)
6. **Given** 3 s of silence, or a 1 s cough or keyboard noise only, **When** recording stops, **Then** no request is sent, nothing is pasted, the clipboard is unchanged and "no speech detected" is shown. (req FR-12)
7. **Given** 3 s of speech, **When** recording stops, **Then** the recording is sent to the engine. (req FR-12)
8. **Given** the speech-detection model cannot be loaded, **When** a recording stops, **Then** an energy-threshold check is used instead and a warning is logged. (req FR-12)
9. **Given** base URL `https://api.openai.com/v1`, model `whisper-1` and a valid key — or the Groq base URL with its model and key — **When** a recording with speech stops, **Then** the audio is sent as 16 kHz mono WAV to `<base URL>/audio/transcriptions` and the transcript text is returned. (req FR-06)
10. **Given** Notepad was focused at start and auto-paste is on, **When** the text is ready, **Then** the text appears in Notepad, is in the clipboard, and Windows clipboard history (Win+V) does not list it. (req FR-10)
11. **Given** the hotkey is Ctrl+Alt+Space, **When** the user dictates into Notepad, **Then** neither the hotkey press nor its release produces any character or menu activation in Notepad, and the paste is not affected by still-held modifier keys. (req FR-10)
12. **Given** auto-paste is off, **When** the text is ready, **Then** the text is only in the clipboard and a notification says so. (req FR-10)
13. **Given** the start window is no longer in front when the text is ready, or its process runs elevated, **Then** the text stays in the clipboard and "copied — paste manually" is shown. (req FR-10)

---

### User Story 2 - Failures are reported and the dictation can be retried (Priority: P1)

When transcription fails (no network, timeout, HTTP error, engine error), nothing is pasted, the user is told why through a Windows notification, the overlay (3 s) and the tray icon's error state, and the audio is kept so the user can retry from the notification or the tray menu once the cause is fixed.

**Why this priority**: Constitution principle III — never lose the user's words, never paste garbage. A silent failure makes the product look broken (r1#8).

**Independent Test**: With a mock endpoint that returns 401, then 200: dictate → "invalid API key" notification, overlay message, tray error state, nothing pasted; click Retry after the endpoint is fixed → the text is delivered per US1 rules.

**Acceptance Scenarios**:

1. **Given** the API returns 401 or 403, **When** a recording is transcribed, **Then** "invalid API key" is shown and nothing is pasted. (req FR-06)
2. **Given** the network is off and the API engine is selected, **When** a recording is transcribed, **Then** "network unavailable — Retry" is shown, nothing is pasted and the audio is kept. (req FR-11)
3. **Given** the network returns, **When** the user clicks Retry in the notification or the tray menu, **Then** the text is delivered — pasted only if the start window is in front again, otherwise copied with "copied — paste manually". (req FR-11)
4. **Given** a retry fails, **Then** the reason is shown again and the audio is still kept. (req FR-11)
5. **Given** the endpoint accepts the connection but does not answer within 30 s, **Then** the failure reason is "timeout" and the failure branch of scenario 2 applies; **Given** the connection cannot be established within 5 s, **Then** the failure is reported as unreachable host. (req FR-24, FR-11)
6. **Given** Focus Assist is on and the API key is invalid, **When** a dictation fails, **Then** the overlay shows "invalid API key" for 3 s and the tray icon shows the error state. (req FR-25)
7. **Given** the tray icon is in the error state, **When** the next dictation succeeds or the user opens the tray menu, **Then** the tray icon leaves the error state. (req FR-25)
8. **Given** a pending failed recording exists, **When** a newer recording also fails, **Then** the newer one replaces it as the only pending recording; a successful newer dictation does not remove the pending one. (req NFR-06)
9. **Given** the endpoint returns a malformed body, an unexpected status, or the engine reports any internal error, **Then** the app reports the failure per scenario 2 and keeps running; the hotkey keeps working. (req NFR-07)

---

### User Story 3 - Toggle mode and cancel (Priority: P2)

The user who dictates longer texts sets the mode to toggle: one press starts, the next press stops. In either mode, pressing Esc while recording discards the recording. Recording stops automatically at 10 minutes.

**Why this priority**: Both modes and cancel are Must in release 1, but the hold mode of US1 alone already gives a usable product.

**Independent Test**: Mode = toggle: press, speak 10 s, press → 10 s processed; press, speak, Esc → nothing sent or pasted, indicator off; with no recording, Esc reaches Notepad.

**Acceptance Scenarios**:

1. **Given** mode = toggle, **When** the user presses the hotkey, speaks 10 s and presses it again, **Then** 10 s of audio is processed. (req FR-03)
2. **Given** a recording reaches 10 minutes, **Then** it stops, what was recorded is processed and "maximum length reached" is shown. (req FR-03, decisions #1)
3. **Given** recording is on, **When** Esc is pressed, **Then** nothing is sent or pasted and the indicator goes off. (req FR-22)
4. **Given** no recording is on, **When** Esc is pressed, **Then** Esc reaches the focused application as usual. (req FR-22)

---

### User Story 4 - The hotkey is always working or the user is told (Priority: P2)

If the hotkey cannot be registered because another application or the OS owns it, the user is told and the app keeps or requires a working hotkey. The hotkey and recording keep working after sleep/resume, lock/unlock and an Explorer restart.

**Why this priority**: A silently dead hotkey makes the app useless with no way to diagnose (pre-mortem, requirements §10).

**Independent Test**: Register Ctrl+Alt+Space from another process, start Voicen → tray error state, notification, settings open on the hotkey field. In settings pick a taken hotkey → "hotkey unavailable", the old hotkey still works. Sleep and resume the PC → the hotkey still starts recording.

**Acceptance Scenarios**:

1. **Given** the saved hotkey cannot be registered at start-up, **Then** the tray icon shows the error state, a notification is shown, and settings open on the hotkey field; the error state persists until a hotkey is registered. (req FR-05, FR-25)
2. **Given** a working hotkey, **When** the user picks a hotkey that is taken, **Then** "hotkey unavailable" is shown and the old hotkey stays active. (req FR-05)
3. **Given** the PC sleeps and resumes, or is locked and unlocked, or Explorer restarts, **When** the hotkey is pressed, **Then** recording starts, and after an Explorer restart the tray icon is present again. (req FR-26)
4. **Given** re-registering the hotkey after resume fails, **Then** the start-up failure branch (scenario 1) applies. (req FR-26, FR-05)

---

### User Story 5 - Dictate again while the previous one is still processing (Priority: P3)

The user dictates A into one window and, while A is still being transcribed, dictates B into another. Both texts arrive, A first, each in its own window.

**Why this priority**: Required (Must), but only matters to fast users; US1 is usable without it if the user waits.

**Independent Test**: Core pipeline with a mock endpoint that answers A slowly and B fast: B's delivery waits for A; A goes to A's start window, then B to B's.

**Acceptance Scenarios**:

1. **Given** dictation A is processing, **When** the user dictates B in another window, **Then** A is delivered to A's window and B to B's window, A first. (req FR-23)
2. **Given** A fails, **Then** B is still delivered and A follows the failure rules of US2. (req FR-23)

---

### User Story 6 - Microphone changes (Priority: P3)

If the selected microphone is missing, the default Windows input device is used and the user is told once per device change. If the microphone disappears during recording, what was captured is processed.

**Why this priority**: Required (Must) robustness; affects users with USB or Bluetooth headsets.

**Independent Test**: Select a USB mic, unplug it, press the hotkey → recording uses the default device and a one-time notification names it; unplug the mic at second 5 of a recording → 5 s processed.

**Acceptance Scenarios**:

1. **Given** the selected USB microphone is unplugged, **When** the hotkey is pressed, **Then** recording uses the Windows default input device and a one-time notification names it. (req FR-27)
2. **Given** the fallback notification was shown, **When** further recordings use the same default device, **Then** no further notification is shown until the device situation changes; **When** the selected microphone returns, **Then** it is used again. (req FR-27)
3. **Given** the microphone is unplugged at second 5 of a recording, **Then** recording stops and the 5 s are processed. (req FR-27)
4. **Given** no input device exists at all, or microphone access is denied by Windows privacy settings, **When** the hotkey is pressed, **Then** no recording state is shown and "microphone unavailable" is shown with the reason. (req FR-27, FR-04)

---

### Edge Cases

- Hotkey pressed while engine = none (first run not completed): no recording; "choose a transcription engine" is shown and settings open (req FR-21 failure branch; FR-21 itself belongs to feature 004).
- Hotkey pressed while a recording is already on in hold mode (key auto-repeat): ignored; one recording only.
- Hold mode, Esc pressed while the hotkey is still held: the recording is discarded; the later release does nothing.
- Esc cannot be claimed while recording (another application owns it): recording still works; cancel is unavailable for that recording and a warning is logged.
- Recording of exactly 10 minutes in hold mode: stopped and processed as in toggle mode (decisions #1).
- The endpoint returns HTTP 200 with an empty transcript: treated as "no speech detected" — nothing pasted, nothing kept for retry.
- The endpoint returns 413 (payload too large), 429 (rate limited) or 5xx: failure with the HTTP reason; audio kept for retry; no automatic retry.
- Base URL with or without a trailing slash: the request path is the same `<base URL>/audio/transcriptions`.
- The start window was closed before the text is ready: treated as "not in front" — copied, "copied — paste manually".
- Modifier keys still held when the text is ready (e.g., the user already holds Ctrl for the next shortcut): paste waits up to 1 s; if they are still held, no Ctrl+V is sent, the text stays in the clipboard and "copied — paste manually" is shown (Clarification 2).
- Retry clicked while no pending recording exists (already retried successfully): nothing happens; the tray item is not offered.
- A retry whose engine now returns an empty transcript: treated as "no speech detected"; the pending recording is deleted (same rule as the empty-transcript edge case).
- A new recording starts while a 3 s overlay message is shown: the overlay switches to the recording state at once; the message is not shown again (the toast and tray state still carry it, req FR-25).
- Retry clicked while a new recording is on: the retry runs with the current settings and is delivered after every recording already sent to an engine and before the recording that is on now (Clarifications 3, 4).
- App exit with a pending recording or recordings in flight: all audio is deleted; in-flight results are dropped.
- App crashed with temporary audio on disk: at next start the stale audio is deleted.
- Device disappears and the default device also disappears mid-recording: recording stops, the captured part is processed.
- The clipboard cannot be opened (held by another process) when the text is ready: nothing is pasted; the failure "clipboard unavailable" is reported per FR-028 and the recording's audio becomes the pending recording, so Retry re-runs it (req FR-11: never lose the words).
- Toasts disabled or Focus Assist on: the overlay and tray state still report the failure (req FR-25).
- Sleep while recording: on resume the recording is stopped and what was captured is processed (same rule as a lost device, req FR-27).

## Requirements *(mandatory)*

### Functional Requirements

**Background presence**

- **FR-001** (req FR-01): On start (not first run), the system MUST run in the background with a tray icon whose menu gives access to Settings, History, "Open logs folder", "Retry last failed dictation" (only while a pending recording exists, req FR-11) and Exit — the History entry appears once feature 005 provides the history window — and MUST NOT open or focus any window.
- **FR-002** (req FR-01): The system MUST run as a single instance per user session; launching a second instance MUST NOT start a second instance and MUST bring the running instance's settings window to front.

**Recording**

- **FR-003** (req FR-02): In hold mode, the system MUST record from the selected microphone while the hotkey is held and stop when it is released; releasing any key of the combination counts as the release.
- **FR-004** (req FR-02): In hold mode, a hold shorter than 0.3 s MUST discard the recording without sending it and without a notification.
- **FR-005** (req FR-03): In toggle mode, a hotkey press MUST start recording and the next press MUST stop it.
- **FR-006** (req FR-03, decisions #1): In either mode, a recording that reaches 10 minutes MUST stop, be processed, and the user MUST be notified "maximum length reached".
- **FR-007** (req NFR-02): The microphone MUST be opened only when recording starts and closed when it stops (the Windows "microphone in use" indicator shows only while recording); hotkey press → capture start MUST be ≤ 200 ms at p95 on a wired or USB microphone; Bluetooth headsets may lose the first ~0.5 s (accepted by req NFR-02).
- **FR-008** (req FR-04): While recording, the tray icon MUST be in its recording state and a small overlay MUST be visible that never takes keyboard focus; the window focused at recording start MUST keep focus. After recording stops, the overlay MUST show a processing state while any dictation is being transcribed or post-processed and no recording is on, and MUST hide when nothing is recording or processing and no message is shown (Clarification 5).
- **FR-009** (req FR-04, FR-27): If the microphone is unavailable, no input device exists, or access is denied, the system MUST NOT enter a recording state and MUST notify "microphone unavailable" with the reason.
- **FR-010** (req FR-22): While recording, Esc MUST stop and discard the recording (nothing sent or pasted, indicator off); Esc MUST be intercepted only while recording.

**Hotkey**

- **FR-011** (req FR-05, FR-25): If the saved hotkey cannot be registered at start-up, the system MUST show the tray error state, show a notification, and open settings on the hotkey field; this error state MUST persist until a hotkey is registered. The app MUST never run without a working hotkey silently.
- **FR-012** (req FR-05): When the user changes the hotkey to one that cannot be registered, the change MUST be refused with "hotkey unavailable" and the previous hotkey MUST stay active.
- **FR-013** (req FR-26): The hotkey and recording MUST keep working after sleep/resume, lock/unlock and an Explorer restart without restarting the app; after an Explorer restart the tray icon MUST reappear. If re-registering the hotkey fails, FR-011 applies.
- **FR-014** (req FR-10): The hotkey press and release MUST NOT be delivered to the focused application.

**Speech detection**

- **FR-015** (req FR-12): Before any recording is sent to an engine, the system MUST run voice-activity detection with the bundled Silero model; if no speech is detected, the system MUST send nothing, paste nothing, leave the clipboard unchanged and notify "no speech detected".
- **FR-016** (req FR-12): If the speech-detection model cannot be loaded, the system MUST fall back to an energy threshold and log a warning.

**Transcription via API**

- **FR-017** (req FR-06): For the engine "OpenAI-compatible API", the system MUST send the recording as 16 kHz mono WAV to `<base URL>/audio/transcriptions` with the configured model, API key and language (no language when set to auto-detect) and use the returned text.
- **FR-018** (req FR-24): Transcription requests MUST use a connect timeout (default 5 s; opening the connection) and a transcription timeout (default 30 s; the whole request from its start until the full response is received, connecting and uploading included; Clarification 1); the values are settings (decision #99), derived per job from the job's settings snapshot with `Timeouts::from_settings`, and MUST come from one place shared with the other engines and post-processing.
- **FR-019** (req FR-06, FR-11, FR-24): Every transcription failure MUST paste nothing and report a reason: HTTP 401/403 → "invalid API key"; no network / DNS failure → "network unavailable"; connection refused or connect timeout → "cannot reach <host>"; request timeout → "timeout"; other HTTP status → "server error (HTTP <code>)"; unreadable response → "unexpected response from the server".
- **FR-020** (req NFR-07): A failure of the engine — including malformed or oversized responses and internal errors — MUST be reported as a transcription failure and MUST NOT crash the app or disable the hotkey.
- **FR-021** (req FR-09, integration point): Between transcription and delivery the pipeline MUST pass the text through one post-processing stage; in this feature the stage returns the text unchanged; feature 003 supplies the LLM step and its fallback.

**Delivery**

- **FR-022** (req FR-10): When the final text is ready, the system MUST put it in the clipboard marked as excluded from Windows clipboard history and cloud sync.
- **FR-023** (req FR-10): If auto-paste is on, the system MUST wait until all modifier keys are released (at most 1 s) and then send Ctrl+V to the window that had focus when recording started — only if that window is still in front and its process is not elevated. If modifiers are still held after 1 s, no Ctrl+V is sent and FR-025's "copied — paste manually" branch applies (Clarification 2).
- **FR-024** (req FR-10): If auto-paste is off, the text MUST be only in the clipboard and the user MUST be notified that it was copied.
- **FR-025** (req FR-10): If the start window is no longer in front (including closed) or runs elevated, the system MUST leave the text in the clipboard and notify "copied — paste manually".

**Failures and retry**

- **FR-026** (req FR-11, NFR-06): On a transcription failure the system MUST keep the recording's audio as the pending recording; at most one pending recording exists, and only a later failed recording replaces it.
- **FR-027** (req FR-11): The user MUST be able to retry the pending recording from the failure notification and from the tray menu. A retry MUST use the settings current at the moment Retry is clicked (Clarification 4). A successful retry MUST deliver per FR-022–FR-025 to the original start window (pasted only if it is in front again) and then delete the pending audio; a failed retry MUST report the reason again and keep the audio.
- **FR-028** (req FR-25): Every failure notification MUST also be shown for 3 s in the overlay, and the tray icon MUST show an error state until the next successful dictation or until the user opens the tray menu; the hotkey-registration error (FR-011) persists until a hotkey is registered.

**Ordering**

- **FR-029** (req FR-23): Pressing the hotkey while a previous recording is still being transcribed or post-processed MUST start a new recording; results of recordings sent to an engine MUST be delivered (pasted, copied or reported as failed) in recording order, each to its own start window; a failed earlier recording MUST NOT block later ones. Every stopped recording enters the delivery order at stop, before speech detection, so a later short recording cannot overtake an earlier long one still being checked; one found to contain no speech leaves the order without blocking later ones. A retry enters the delivery order when Retry is clicked: after every recording already sent to an engine, before any recording that stops later (Clarification 3).

**Microphone selection**

- **FR-030** (req FR-27): If the selected microphone is missing, the system MUST record from the Windows default input device and notify once per device change, naming the device, until the selected microphone returns.
- **FR-031** (req FR-27): If the input device disappears during recording, the system MUST stop and process what was captured.

**Privacy of audio**

- **FR-032** (req NFR-06): Each recording's audio MUST be kept only in memory or a temporary file until its result is delivered or discarded; audio MUST be deleted on exit, and stale temporary audio MUST be deleted at the next start after a crash. Audio MUST never be written to history or logs.

**Observability (integration point)**

- **FR-033** (req FR-20, NFR-01, NFR-02, integration point): For each dictation the pipeline MUST expose to the log facility the engine name, the timings hotkey press → first audio frame, stop → text, and text → paste, and the failure reason if any as its stable code (plus the HTTP status where there is one) — never the response body, transcript text, audio or API keys. Feature 006 owns the log file and rotation.

**Messages**

- **FR-034** (req FR-15, dependency): Every user-visible message of this feature MUST be identified by a stable message key and have English and Russian texts; the language selection itself belongs to feature 004.

### Key Entities

- **Recording**: audio captured between start and stop; sequence number (recording order), start window, start time, duration, input device used, how it ended (released, toggled, Esc, max length, device lost).
- **Start window**: the window that had keyboard focus when recording started; identity and whether its process is elevated; the paste target.
- **Dictation job**: a recording on its way through speech detection → transcription → post-processing → delivery; outcome is delivered, copied-only, no speech, discarded, or failed with a reason.
- **Pending recording**: the audio and start window of the latest failed dictation; at most one; replaced only by a later failed recording; deleted after a successful retry or on exit.
- **Failure reason**: a stable code with a message key (invalid API key, network unavailable, cannot reach host, timeout, server error with HTTP code, unexpected response, clipboard unavailable, microphone unavailable, hotkey unavailable).
- **Hotkey binding**: key combination and mode (hold or toggle); registration state (active or failed).
- **Indicator state**: tray state (idle, recording, error, hotkey error) and overlay state (hidden, recording, processing, message for 3 s).
- **Dictation settings (read only here)**: engine, base URL, model, language, API key reference, selected microphone, hotkey, mode, auto-paste. Owned and edited by feature 004.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Over the fixed 20-phrase sample set (5–15 s each), stop → paste p90 ≤ 3 s via OpenAI `whisper-1` and, separately, via Groq `whisper-large-v3-turbo` on a ≥ 20 Mbit/s connection, measured from the log timings (req NFR-01).
- **SC-002**: Hotkey press → capture start p95 ≤ 200 ms on a wired or USB microphone; the Windows microphone-in-use indicator is shown only while recording (req NFR-02).
- **SC-003**: In 100 % of failure cases in the test matrix (each reason of FR-019, no speech, microphone unavailable), nothing is pasted, the user sees the reason in the overlay and the tray state, and transcription failures can be retried with the kept audio (req FR-11, FR-12, FR-25).
- **SC-004**: Auto-paste places the text in the field of each of Notepad, Chrome and Edge text fields, VS Code, Telegram Desktop and Microsoft Word on Windows 11 x64, per the owner's release checklist (req NFR-08).
- **SC-005**: Fault injection of every engine failure mode in the test matrix produces zero crashes; the hotkey works after each (req NFR-07).
- **SC-006**: After sleep/resume, lock/unlock and an Explorer restart, the first hotkey press starts recording in 3 of 3 attempts each, in the owner's manual check (req FR-26).
- **SC-007**: Two overlapping dictations are delivered in recording order to their own windows in 100 % of automated runs, including when the first one fails (req FR-23).
- **SC-008**: After exit, and after the next start following a crash, no recording audio remains on disk (req NFR-06).

## Assumptions

- Settings (engine, base URL, model, key, language, microphone, hotkey, mode, auto-paste) are created and edited by feature 004; this feature reads them and applies changes without restart. Until 004 lands, they can be supplied by the defaults of FR-21 plus a hand-edited settings file.
- The API key is read from Windows Credential Manager through one credential-store interface (NFR-04); its editing UI belongs to feature 004.
- The overlay and settings windows are created on demand and destroyed when closed (NFR-03); the overlay is display-only — it has no buttons, because it must never take focus. Retry is offered in the notification and the tray menu.
- Toast notifications need an application identity created by the installer (requirements §8); the installer belongs to feature 006. When toasts are suppressed, the overlay and tray state still report (FR-025, FR-028).
- The 0.3 s minimum hold is a working assumption of the requirements (§10).
- Global hotkey registration uses the OS hotkey registration rather than a low-level keyboard hook where possible (working assumption, requirements §10).
- Pasting into elevated windows is impossible from a non-elevated app (working assumption, requirements §10).
- A 10-minute recording as 16 kHz mono 16-bit WAV is about 19.2 MB, under OpenAI's 25 MB upload limit (requirements §8); a provider with a lower limit answers 413, which is reported as "server error (HTTP 413)" with the audio kept (FR-019, FR-026).
- The clipboard's previous content is not restored after delivery: the text stays in the clipboard so the user can paste it again or manually (req FR-10).
- Recordings that never complete (hold < 0.3 s, Esc, microphone unavailable) are reported immediately and never enter the delivery order; a stopped recording enters it at stop and, if speech detection finds no speech, leaves it with its "no speech detected" notice without blocking later ones (FR-029).
- The 20-phrase sample set and the speech / silence / cough / keyboard-noise fixtures for speech detection are committed to the repository with licences compatible with MIT (NFR-12).
- Out of scope here: the local engines (feature 002), LLM post-processing (003), settings window, first run, i18n framework and autostart (004), history (005), the log file, crash files, installer and release (006).
