# Requirements

<!-- The product's requirements document: what must be true, stated so it can be tested.
     Built and roasted by /teamwright:requirements; the spec tool and the tasks are derived from it.
     Write only what the owner said or approved. A guess is marked "working assumption" in section 10;
     an unknown goes to docs/open-questions.md. IDs (FR-NN, NFR-NN) are never reused. -->

Version: 5 · Status: draft (v5: release 2 — call mode added; release 1 lines unchanged from v4, approved by the owner 2026-10-02) · Roast of v5: not yet run (`/teamwright:requirements`)
Source: owner's brief in the /teamwright:init call (2026-10-02, Russian), interview rounds 1–3 (2026-10-02), roast round 1–2 answers (2026-10-02), spec findings accepted by the owner (2026-10-02, `(v4)`); release 2: the owner's call-mode package v4, `docs/briefs/call-mode-r1/` (2026-10-21, Russian: `tz.md`, `kontseptsiya.md`, `prozharka-1.md`, `adr/`)

Pointers: `(brief)` — the owner's brief; `(iN)` — interview round N; `(r1#N)`, `(r2#N)` — roast round 1/2 finding N, accepted by the owner. Release 2: `(tz ВФ-NN)` / `(tz ВН-NN)` — functional / non-functional requirement NN of the package's `tz.md`; `(tz §N)`, `(tz ВВ-N)`, `(tz П-N)` — its section N, owner answer ВВ-N, roast answer П-N; `(ADR-NNN)` — `adr/ADR-NNN-*.md` of the package (owner-accepted parts are decisions #159–#166, proposed parts are OQ-41…OQ-49); `(roast-pkg А-N)`, `(roast-pkg Б-N)` — finding А-N (requirements) or Б-N (approach) of `prozharka-1.md`; `(concept)` — `kontseptsiya.md`; `proposal` — a design proposal of the package, refined in the roast and the spec; `CA-N` — a call-mode working assumption (§10).

## 1. Problem and users

**Product (working name):** Voicen — a desktop voice-to-text tool: record speech from the microphone, transcribe it, put the text into the clipboard and paste it into the active input field (brief).

**Users:** anyone on Windows who prefers dictating to typing — writing messages, prompts for AI assistants, notes, documents. Distributed as public open-source software (i1).

**Jobs To Be Done:**
- When I am working in any application and want to enter text, I want to press a hotkey, speak and have the text appear in the field I was typing in, so I can enter text faster than typing without switching windows (brief, i1).
- When I care about privacy or have no internet, I want transcription to run on my own machine, so my voice never leaves the computer (brief, i2).
- When I want the best quality or speed, I want to plug in a cloud API of my choice with my own key, so I can pick the provider and pay only for what I use (brief, i2).

**Today without the product:** the user types by hand, or uses the built-in Windows voice typing (Win+H), or records audio and transcribes it in a separate service, then copies the text manually.

### Release 2 — call mode: problem and users

**Problem:** the owner has work calls (clients, team) and job interviews; the conversation is not kept, agreements stay in memory, and a question in a foreign language is sometimes hard to catch by ear (concept).

**Users:** first the owner; as part of public open-source Voicen the feature is available to every Voicen user (tz ВВ-1, ADR-001).

**Jobs To Be Done:**
- When I am on a call in any application (Telegram, Zoom, Meet, Teams, a browser), I want it recorded on my PC without a bot joining, so nothing agreed is lost (concept, ADR-002).
- When the call ends, I want a transcript that shows who said what and when, ready to read and to hand to agents (concept, tz §1).
- During the call, I want live subtitles of both sides on top of the screen, so I can catch what I missed (concept).
- After a work call or an interview, I want a summary by template with links to the moments in the call (tz ВВ-3).

**Today without the feature:** memory and hand notes, or a meeting bot that works only on some platforms and that the other party sees (ADR-002).

## 2. First release: goal and success

**The one flow that must work end to end:** the app runs in the background → the user focuses an input field in any application → presses the global hotkey (hold or toggle) → speaks → stops → the audio is transcribed by the configured engine (OpenAI-compatible API, built-in local whisper.cpp, or a local OpenAI-compatible server) → optionally post-processed by an LLM → the text is copied to the clipboard and pasted into the field that was focused.

**Success (all three criteria, i3, r1#21):**
1. **Speed:** NFR-01 is met — p90 stop→paste ≤ 3 s via the API engine and ≤ 10 s via the built-in local `small` model, over the fixed 20-phrase sample set, without LLM post-processing.
2. **Daily use:** the owner uses it daily for 2 weeks without a crash; checked by the crash files and the log (FR-20) being free of crash records for that period.
3. **Public release:** a Windows installer published in GitHub Releases installs and completes the flow on a clean Windows 11 (Windows Sandbox on the owner's PC, r1#19).

**First run (r1#4):** on first launch the settings window opens on the Engine tab with engine = none; the user picks an engine (enters a key or downloads a model) and can dictate immediately with the defaults (FR-21).

Target date: when ready (decisions #82).

### Release 2 — call mode: goal and success

**Naming:** the package `docs/briefs/call-mode-r1/` calls this release "call-mode release 1". In this document it is **release 2**; release 1 stays the dictation release above. The package's own "release 2" (translation in subtitles) is a later release here (OQ-50).

**Goal (tz §1):** on the owner's Windows PC, a call in any application is recorded as two tracks (the user's microphone and the other party's audio), optionally accompanied by live subtitles, and after stop turned into a transcript with speakers, ready to read and for agents.

**The flow:** the user turns on call mode (tray, hotkey) with "Record" and/or "Subtitles" → Voicen captures the microphone and the system audio as two tracks → with Subtitles on, phrases appear in a subtitle window labelled "Me" / "Other party" → the user stops → the tracks are transcribed again in full, the other party's track is diarized, both are merged by time → the call appears in the "Calls" section as `transcript.json` / `transcript.md` next to the audio.

**Success (all five criteria are mandatory, tz §1):**
1. **Real use:** the owner records at least 5 real calls of 30–90 min (work calls and interviews) in Telegram Desktop, Google Meet in a browser, and Zoom or Teams; none is lost or damaged. Checked by the "Calls" section and the log.
2. **Speakers:** the role "Me / other party" is right for ≥ 95 % of speech time, checked on the test set and on 2 real 10-min calls labelled by hand by the owner (tz П-6, roast-pkg А13); with 2–3 remote participants, DER on the system track ≤ 20 % on an open corpus of real recordings (working assumption CA-1).
3. **Subtitles:** end of phrase → shown in the window, p90 ≤ 4 s via the API engine and ≤ 6 s via the built-in `small` model on the owner's PC (working assumption CA-2; NFR-13).
4. **Final transcript:** for a one-hour call, ready ≤ 5 min via the API and ≤ 30 min locally (`small`, owner's PC) (working assumption CA-3; NFR-13).
5. **Reliability:** 2 weeks of daily use without a crash; an engine failure neither crashes the app nor loses the recording (NFR-15).

**Delivery (tz §11, decisions #171):** four usable increments in order 1a "Record and read" → 1b "Subtitles" → 1c "Speaker diarization" → 1d "Local and quality" (§3). Until 1d the default call engine is the API engine; from 1d the local one (tz ВВ-2).

**Start:** call-mode requirements and spec start now, in parallel with the publication of release 1 (T-026, T-028) (decisions #174; was: the package's condition П-2).

Target date: not set by the owner.

## 3. Scope

| Activity | Steps | Release 1 | Priority |
|---|---|---|---|
| Install and start | install from GitHub Releases; run in background (tray); optional start with Windows | yes | Must (start with Windows: Should) |
| Configure | choose engine and its settings; enter API key; download a local model; choose hotkey and mode; choose speech language; choose microphone; UI language | yes | Must |
| Record | press hotkey; hold (push-to-talk) or toggle; see that recording is on; cancel; detect speech (VAD) | yes | Must |
| Transcribe | send to OpenAI-compatible API; built-in whisper.cpp; local OpenAI-compatible server | yes | Must |
| Post-process | optional LLM step over an OpenAI-compatible chat API with an editable prompt | yes | Must (owner asked for it in release 1, i3) |
| Deliver text | copy to clipboard; auto-paste with Ctrl+V into the focused field; auto-paste can be turned off | yes | Must |
| Handle failures | notify with the reason (toast + overlay + tray state); keep the audio; retry; survive sleep/lock and microphone changes | yes | Must |
| Review history | list of the last N transcripts (text only), copy again, disable, clear; on by default, N = 20 (r1#16) | yes | Should |
| Diagnose | rotating local log, crash file, "Open logs folder" (r1#3) | yes | Must |
| Other providers | Deepgram, ElevenLabs Scribe, Yandex SpeechKit native APIs | no | Won't (release 1) |
| Other platforms | macOS, Linux | no | Won't (release 1) |
| Automatic fallback | switching to a reserve engine on failure | no | Could (later) |
| GPU acceleration | CUDA / Vulkan for local engine | no | Could (later) |

**Not doing (no-gos) in release 1:**
- macOS and Linux builds — Windows only (i1).
- Provider-specific APIs other than OpenAI-compatible (i2): the OpenAI-compatible client covers OpenAI, Groq and compatible local servers.
- Storing audio in history (i3): history keeps text only.
- Automatic fallback to another engine (i3: notification + retry was chosen, not fallback).
- Code signing, auto-update, licensing (i1: public open-source, not a commercial product). No signing in release 1, SmartScreen warning accepted (decisions #83).
- Real-time streaming transcription while speaking.

### Release 2 — call mode: scope

| Activity | Steps | Increment | Priority |
|---|---|---|---|
| Control call mode | two independent toggles "Record" and "Subtitles"; tray menu; call-mode hotkey; one session at a time; exit/update during a recording (FR-30…FR-33, FR-37, FR-38) | 1a (Subtitles toggle: 1b) | Must |
| See and stay honest | recording indicator with timer and track levels; reminder to tell the other party; "better with headphones" hint (FR-34, FR-35, FR-46) | 1a | Must |
| Dictate during a call | dictation keeps working; its speech is "Me" in the call (FR-36) | 1a | Must |
| Capture | mic + system audio (WASAPI loopback) as two tracks; drift compensation; output/microphone device changes; VAD gate against invented text (FR-39…FR-42, FR-45) | 1a | Must |
| Echo without headphones | duplicate filter (FR-44); echo suppression (FR-43) | 1d | Should (filter), Could (suppression) (tz П-3) |
| Record safely | incremental Ogg Opus; recovery after a crash; 4 h limit; low disk; sleep; subtitles-only writes nothing (FR-47…FR-52) | 1a | Must (sleep: Should) |
| Live subtitles | non-activating subtitle window; VAD phrases through the subtitle engine; labels by track; errors in the window; phrase language (FR-53…FR-59) | 1b | Must (click-through, font, transparency: Should) |
| Final transcript | full second pass after stop; merge by time; JSON + Markdown; failure and "Transcribe again"; cloud-provider warning (FR-60, FR-63…FR-65, FR-67) | 1a | Must |
| Diarization of the other party | "Other party 1…N" on the system track; merge two speakers (FR-62, FR-74) | 1c | Must (merge: Should) |
| Engines for calls | subtitle engine and final engine chosen apart from dictation (FR-66); local engine by default (tz ВВ-2) | 1b (choice), 1d (local default) | Should |
| Summaries | "Summary" button, templates "work call" and "interview", `summary.md` (FR-68…FR-70) | 1d | Should (tz ВВ-3) |
| Calls section | list, view with search, rename speaker and call, copy/export MD and JSON, open folder, delete (FR-71…FR-77) | 1a | Must |
| Calls section extras | playback from a line (FR-78); quick final from subtitles (FR-61); auto-delete after N days (FR-79) | 1d | Should / Could / Could |
| Settings | "Calls" tab (FR-80) | 1a, each item with its increment | Must (items: own priority) |
| Translation in subtitles | translate lines whose language differs from the primary language | no — a later release (decisions #166) | Won't (release 2) |

**Increments (tz §11; decisions #171):**

| Increment | Requirements | Done when |
|---|---|---|
| 1a — Record and read | FR-30…FR-42, FR-45…FR-52, FR-60, FR-63…FR-65, FR-67, FR-71…FR-77, FR-80 (partly); NFR-15…NFR-19, NFR-21, NFR-22. Final through the existing API engine; "Me / Other party" by track, no diarization | the owner recorded and read a real one-to-one call |
| 1b — Subtitles | FR-53…FR-59, FR-66; NFR-13 (subtitles) | subtitles on a real call within success criterion 3 |
| 1c — Diarization | FR-62, FR-74; success criterion 2 | DER ≤ 20 % on the corpus (CA-1); the owner's labelling (tz П-6) checked |
| 1d — Local and quality | built-in whisper.cpp (T-017), Silero in the installer (T-102), local default engine; FR-43, FR-44, FR-61, FR-68…FR-70, FR-78, FR-79; NFR-13 (local), NFR-14, NFR-20 | every success criterion of §2 (release 2) holds |

**Not doing (no-gos) in release 2 (concept, tz §9):**
- Translation in subtitles — a later release (the package's "release 2", decisions #166).
- Spoken simultaneous translation, voice cloning.
- Recognising participants by name (voice profiles) and a consent journal — together in a later release (ADR-010, decisions #165).
- An AI assistant with hints during the call; e-mails and tracker tasks by an agent; integrations (calendar, mail, trackers, ecosystem-bots).
- A meeting bot, an own meeting room or dialler; a virtual audio cable (ADR-002).
- A "secret recording" mode: recording is always visible (ADR-010).
- Partial ("typing as you speak") subtitles and streaming models (ADR-005, proposed).
- macOS, Linux, mobile.

## 4. Functional requirements

| ID | Requirement (EARS) | Acceptance: examples, including a failing one | Priority | Release |
|---|---|---|---|---|
| FR-01 | When the app starts, the system shall run in the background with a tray icon giving access to settings, history, "Open logs folder" and exit. | Given the app is launched (not first run), then a tray icon appears and no window takes focus; failure: if a second instance is launched, then the system shall not start a second instance and shall bring the running one's settings to front. | Must | 1 |
| FR-02 | When the user presses the configured global hotkey in hold mode, the system shall record from the selected microphone while the hotkey is held and stop when it is released. | Given mode = hold, hotkey = Ctrl+Alt+Space, when the user holds it 4 s in Notepad and releases, then ~4 s of audio is captured and processing starts; failure: if the hotkey is held < 0.3 s, then the system shall discard the recording without sending it. | Must | 1 |
| FR-03 | When the user presses the configured global hotkey in toggle mode, the system shall start recording, and stop when the hotkey is pressed again. | Given mode = toggle, press → speak 10 s → press, then 10 s of audio is processed; failure: if the recording reaches 10 min, then the system shall stop it, process what was recorded and notify "maximum length reached" (r1#12). | Must | 1 |
| FR-04 | While recording, the system shall show a visible indicator: tray icon in recording state and a small overlay that never takes keyboard focus (r1#18). | Given Notepad is focused and recording starts, then the overlay is visible and Notepad keeps focus; failure: if the microphone is unavailable or access is denied, then the system shall not show a recording state and shall notify "microphone unavailable" with the reason. | Must | 1 |
| FR-05 | When the configured hotkey cannot be registered (taken by another application or the OS), the system shall report the conflict and keep or require a working hotkey. | In settings: the user picks a taken hotkey → "hotkey unavailable", the old one stays active; at startup (r1#9): the saved hotkey cannot be registered → tray error state + notification + settings open on the hotkey field; failure: the app never runs without a working hotkey silently. | Must | 1 |
| FR-06 | When recording stops and the engine is "OpenAI-compatible API", the system shall send the audio as 16 kHz mono WAV (r1#12) to `<base URL>/audio/transcriptions` with the configured model, API key and language, and receive the text; a URL that already ends with `/audio/transcriptions` is used as is (#96). | Given base URL = https://api.openai.com/v1, model = whisper-1, valid key, then the transcript text is returned; Given base URL of Groq, it works the same way; Given URL = https://api.openai.com/v1/audio/transcriptions, then the request goes to that URL with nothing appended; failure: if the API returns 401/403, then the system shall notify "invalid API key" and paste nothing. | Must | 1 |
| FR-07 | When recording stops and the engine is "built-in local", the system shall transcribe the audio on the user's machine with whisper.cpp and the selected downloaded model, without network access. | Given model `small` is downloaded and the network is off, then the transcript is produced and pasted; failure: if no model is downloaded, then the system shall notify "no local model" with a link to the settings, and paste nothing. | Must | 1 |
| FR-08 | When the user selects a local model in settings, the system shall download it with progress, verify its SHA-256 against the value pinned in the app, and make it available offline. Offered models (r1#14): multilingual tiny, base, small (recommended), medium (q5_0), large-v3-turbo (q5_0). | Given the user picks `base`, then a progress bar shows and the model becomes selectable when done; failure: if the download is interrupted or the checksum does not match, then the system shall delete the partial file and offer to retry. | Must | 1 |
| FR-09 | When LLM post-processing is enabled, the system shall send the transcript and the user's editable prompt to the configured OpenAI-compatible chat endpoint and use the returned text instead of the raw transcript. | Given post-processing on with prompt "fix punctuation", raw "привет как дела" → pasted "Привет, как дела?"; failure: if the LLM call fails or exceeds the post-processing timeout (FR-24, default 15 s), then the system shall paste the raw transcript and notify that post-processing was skipped. | Must | 1 |
| FR-10 | When the final text is ready, the system shall put it in the clipboard (marked as excluded from Windows clipboard history and cloud sync, r1#28) and, if auto-paste is on, wait until all modifier keys are released (≤ 1 s) and send Ctrl+V to the window that had focus when recording started (r1#17). The hotkey press and release are not delivered to the focused application (r2#7). | Given Notepad was focused at start and auto-paste is on, then the text appears in Notepad and is also in the clipboard, and Win+V clipboard history does not list it (r2#8); Given auto-paste is off, then the text is only in the clipboard and a notification says so; failure: if the start window is no longer in front, or its process runs elevated, then the system shall leave the text in the clipboard and notify "copied — paste manually". | Must | 1 |
| FR-11 | If transcription fails (no network, timeout, HTTP error, engine error), then the system shall paste nothing, notify the user with the reason, and keep the audio of the last failed recording so the user can retry from the notification or the tray. | Given network off and the API engine, then a notification "network unavailable — Retry" appears; when the network returns and the user clicks Retry, then the text is delivered per FR-10 — pasted only if the start window is in front again, otherwise copied with "copied — paste manually" (r1#7); failure of the retry shows the reason again and keeps the audio. | Must | 1 |
| FR-12 | Before sending a recording to any engine, the system shall run voice-activity detection; if no speech is detected, then the system shall send nothing, paste nothing and notify "no speech detected" (r1#2). VAD uses the Silero model bundled in the installer for all engines (r2#3). | Given 3 s of silence, then no request is made, nothing is pasted, the clipboard is unchanged; Given a 1 s cough or keyboard noise only, the same; Given 3 s of speech, then the recording is sent; failure: if the VAD model cannot be loaded, then the system shall fall back to an energy threshold and log a warning (r2#3). | Must | 1 |
| FR-13 | The system shall provide a settings window with: engine (none / API / built-in local / local server), base URL, model, API key, local model selection, download and deletion, speech language (auto-detect or fixed), microphone, hotkey and mode, auto-paste on/off, LLM post-processing (on/off, endpoint, model, key, prompt), history on/off and size (1–100), the five request timeouts of FR-24 (seconds), start with Windows, UI language. The local-server engine has its own base URL, optional model and optional key (v4). | Given the user changes any setting and saves, then it takes effect without restarting the app and persists across restarts; failure: if a required field is invalid (empty base URL, malformed URL), then save is refused with the field highlighted. | Must | 1 |
| FR-14 | When the user clicks "Test connection" for the API or local-server engine, the system shall send a short sample to its transcription endpoint (using the values in the form, without saving) and report success or the error; the post-processing endpoint has no test (v4). | Given a valid key, then "OK" with the latency; failure: wrong base URL → "cannot reach <host>"; wrong key → "invalid API key". | Should | 1 |
| FR-15 | The system shall provide all user-visible text (windows, tray, overlay, notifications, error messages) in English and Russian, selectable in settings, defaulting to the OS language (r1#29). Text the user writes or names from the system (the prompt, including the built-in starter prompt, model and device names) is shown as is (v4). | Given Windows display language Russian, then the UI and notifications are Russian on first start; failure: for an unsupported OS language, the UI falls back to English. | Must | 1 |
| FR-16 | When history is enabled (default on, N = 20), the system shall keep the last N final transcripts (text, time, engine) locally, unencrypted, let the user copy any of them again, and clear them (r1#16). | Given N = 20 and 25 transcripts, then the 20 latest are shown; when "Clear" is clicked, then the list is empty and the stored data is deleted; failure: when history is turned off, then stored entries are deleted and nothing more is written to disk. | Should | 1 |
| FR-17 | When the engine is "local server", the system shall use the same OpenAI-compatible transcription client as FR-06 with a user-given URL and an optional key; the URL is a base URL or the full `…/audio/transcriptions` endpoint, as in FR-06 (#96). | Given a speaches/faster-whisper server at http://localhost:8000/v1, then transcription works without a key; Given URL = http://localhost:8000/v1/audio/transcriptions, it works the same way; failure: if the server is not running, then notify "cannot reach localhost:8000" and keep the audio (FR-11). | Must | 1 |
| FR-18 | The system shall report its version and commit in the About dialog and in the log line written at start (FR-20). | Given a build from commit abc123, About shows `<version> (abc123)` and the log's first line of the session contains `abc123`. No failure branch: static information (r1#22). | Must | 1 |
| FR-19 | When "start with Windows" is on (default off), the system shall start minimized to tray at user logon. | Given the option on and a reboot, the tray icon is present after logon; when off, it is not. | Should | 1 |
| FR-20 | The system shall write a rotating local log in `%LOCALAPPDATA%\Voicen\logs` (kept 7 days, ≤ 10 MB total) with version, commit, engine, timings (hotkey press → first audio frame, r2#11; stop → text → paste) and errors — never transcript text, audio or API keys. Crash records: an unhandled panic or a native fault (unhandled-exception filter) writes a crash file; a session marker written at start and removed on clean exit, if found at the next start, writes a crash record "previous session ended abnormally" (r2#4). Crash files are excluded from log rotation and kept 30 days, at most 20 files (r2#1). | Given one dictation, then the log has a line with the engine and the three timings and no transcript text; Given a forced panic in a test build, then a crash file appears and the next start logs "previous session crashed"; Given the process is killed, then the next start writes a crash record; Given a crash file 20 days old, then it is still present after rotation; failure: if the log directory is not writable, then the app keeps working and shows a one-time notification. | Must | 1 |
| FR-21 | When the app starts for the first time, the system shall open settings on the Engine tab with engine = none and apply the defaults: hotkey Ctrl+Alt+Space, mode hold, auto-paste on, speech language auto, history on with N = 20, start with Windows off, UI language from the OS (r1#4); microphone = Windows default input; API engine base URL `https://api.openai.com/v1` and model `whisper-1`; local server `http://localhost:8000/v1`; post-processing off with a built-in editable starter prompt (v4). | Given a fresh install, then settings open with these values; failure: if the hotkey is pressed while engine = none, then the system shall not record, shall notify "choose a transcription engine" and open settings. | Must | 1 |
| FR-22 | When the user presses Esc while recording, the system shall stop recording and discard it; Esc is intercepted only while recording (r1#18). | Given recording is on, when Esc is pressed, then nothing is sent or pasted and the indicator goes off; Given no recording, Esc reaches the focused application as usual. | Must | 1 |
| FR-23 | When the hotkey is pressed while a previous result is still being transcribed or post-processed, the system shall start a new recording; results shall be delivered in recording order, each to its own start window per FR-10 (r1#5). | Given dictation A is processing and the user dictates B in another window, then A is delivered to A's window and B to B's window, A first; failure: if A fails, B is still delivered and A follows FR-11. | Must | 1 |
| FR-24 | The system shall apply timeouts to dictation requests, each a setting in whole seconds (FR-13) with these defaults and bounds (decisions #97, #99): connect 5 s (1–60); transcription via API 30 s (5–600); via local server 60 s (5–1800); post-processing 15 s (5–300, r1#6); built-in local engine 120 s per transcription (10–1800, v4). A value outside its bounds is refused on save. Model downloads keep their own fixed limits and are not settings. | Given the API does not answer within the configured API timeout (default 30 s), then FR-11 applies with reason "timeout"; Given the LLM does not answer within the configured post-processing timeout (default 15 s), then FR-09's failure branch applies; Given the user changes a timeout and saves, then the next job uses it without a restart. | Must | 1 |
| FR-25 | Every failure notification shall also be shown for 3 s in the overlay, and the tray icon shall switch to an error state until the next successful dictation or until the user opens the tray menu (r1#8); a hotkey-registration error (FR-05) persists until a hotkey is registered (r2#9). | Given Focus Assist is on and the API key is invalid, then the overlay shows "invalid API key" for 3 s and the tray icon shows the error state. | Must | 1 |
| FR-26 | The hotkey and recording shall keep working after sleep/resume, lock/unlock and an Explorer restart without restarting the app (r1#9). | Given the PC sleeps and resumes, when the hotkey is pressed, then recording starts; failure: if re-registering the hotkey fails, then FR-05's startup branch applies. Checked manually by the owner. | Must | 1 |
| FR-27 | If the selected microphone is missing, then the system shall use the Windows default input device and notify once per device change, until the selected microphone returns (r2#12); if the device disappears mid-recording, then the system shall stop and process what was captured (r1#10). | Given the selected USB mic is unplugged, when the hotkey is pressed, then recording uses the default device and a one-time notification names it; Given the mic is unplugged at second 5 of a recording, then 5 s are processed; failure: if no input device exists at all, then FR-04's failure branch applies. | Must | 1 |
| FR-28 | The system shall let the user delete a downloaded local model, and the uninstaller shall ask "remove settings, history, models, logs and saved keys?" (default yes); all app data lives in `%LOCALAPPDATA%\Voicen` (r1#15), including the WebView2 data folder (v4). Uninstall always removes the start-with-Windows entry; an upgrade asks nothing and removes nothing (v4). | Given `small` is downloaded, when the user deletes it, then the file is gone and the space is freed; if it was selected, the engine becomes "none"; Given uninstall with "yes", then the folder and the Credential Manager entries are removed; with "no", they stay. | Should | 1 |
| FR-29 | When the user saves a base URL using `http://` on a non-local host (local = loopback only: `localhost` in any case, `127.0.0.0/8`, `::1`; v4), the system shall show a warning that the API key travels unencrypted, and still allow saving (r1#27). | Given `http://example.com/v1`, then a warning shows; Given `http://localhost:8000/v1` or `http://127.0.0.1`, then no warning. | Could | 1 |

### Release 2 — call mode: functional requirements

Release column: `2 / <increment>` (§3). The ВФ → FR map is in §10.

| ID | Requirement (EARS) | Acceptance: examples, including a failing one | Priority | Release |
|---|---|---|---|---|
| FR-30 | The system shall have a call mode with two independent toggles "Record" and "Subtitles"; call mode is active while at least one is on (tz ВФ-01, decisions #167). | Given both on, then both tracks are recorded and the subtitle window is visible; Given both off, then call mode is inactive and the microphone and the system-audio capture are closed. | Must | 2 / 1a (Subtitles: 1b) |
| FR-31 | When the user changes one toggle while call mode is active, the system shall apply it without stopping the other (tz ВФ-02). | Given a recording has run 10 min, when Subtitles is turned on, then the window appears and the recorded file has no gap; when Subtitles is turned off, then the window closes and the recording continues. | Must | 2 / 1a |
| FR-32 | The system shall let the user control call mode from the tray menu: "Call" with the two toggles as check items, and "Stop" (tz ВФ-03, proposal). | Given the tray menu, when "Record" is checked, then the recording indicator appears within ≤ 1 s. | Must | 2 / 1a |
| FR-33 | When the user presses the call-mode hotkey, the system shall start call mode with the last used toggle combination (first use: Record + Subtitles), and stop call mode on the next press (tz ВФ-04, proposal). | Given first use, press → Record + Subtitles; press again → stopped; failure: if the hotkey is taken by another program, then the system shall notify with the reason, as for the dictation hotkey (FR-05). | Must | 2 / 1a |
| FR-34 | While recording, the system shall show a recording indicator with a timer above other windows and a recording state in the tray (tz ВФ-05). | Given a browser in full screen, then "● Recording 00:12" is visible above it; the indicator can be moved and its position is remembered (OQ-52). | Must | 2 / 1a |
| FR-35 | When a recording starts, the system shall show in the indicator a reminder to tell the other party about the recording and the hint "better with headphones"; the reminder can be turned off in settings (tz ВФ-06, ADR-010, roast-pkg А12). | Given any recording start, then the reminder is visible for 10 s; Given the reminder is turned off, then it is not shown. | Must | 2 / 1a |
| FR-36 | While call mode is active, dictation (FR-02…FR-11) shall work as usual; the dictated speech is part of the mic track and appears in the call transcript as "Me" (tz ВФ-07, ВВ-4). | Given a recording runs, when the user dictates a phrase into a chat, then the text is pasted and the call transcript holds the phrase as "Me". | Must | 2 / 1a |
| FR-37 | When the user turns call mode on while it is already active, the system shall not start a second session and shall apply the toggles to the current one (tz ВФ-08, roast-pkg А21). | Given a recording runs, when "Record" is chosen again in the tray, then there is still one session; failure: a repeated activation never opens a second capture or a second call folder. | Must | 2 / 1a |
| FR-38 | When the user exits the app or an update starts while recording, the system shall ask for confirmation and then stop the recording cleanly with saving (tz ВФ-09, roast-pkg А20). | Given a recording, exit → question → "Yes" → the call is saved and transcribed at the next start. The "No" answer and the installer's own close are not specified by the package (roast). | Must | 2 / 1a |
| FR-39 | When Record or Subtitles is on, the system shall capture two tracks at once and never mix them: "mic" — the microphone selected in Voicen settings; "system" — the system audio of the default output device (WASAPI loopback) (tz ВФ-10, ADR-002). | Given a call in Telegram Desktop, then the other party's voice is in the system track and the user's own in the mic track; failure: audio that the call app sends to a non-default output device is not captured, and FR-46 warns. | Must | 2 / 1a |
| FR-40 | The system shall align both tracks to one clock and compensate the drift between the two devices' clocks (tz ВФ-11, ADR-002, roast-pkg Б3). | With fake sources drifting 50 ppm, the timestamps of both tracks differ by ≤ 100 ms at minute 60 (value: working assumption CA-4); on the owner's PC, clicks heard by both the speaker and the mic every 10 min line up. | Must | 2 / 1a |
| FR-41 | When the default output device changes (a headset plugged in or removed, including a Bluetooth switch to its headset profile), the system shall continue capturing system audio from the new device with a gap of ≤ 2 s and record the event in `meta.json` and the log (tz ВФ-12, roast-pkg Б11). | Given Bluetooth headphones are connected mid-call, then the recording continues and `meta.json` and the log hold the device change; failure: the gap is filled with silence so that later timestamps do not shift (ADR-002, proposed). | Must | 2 / 1a |
| FR-42 | If the microphone disappears or changes during call mode, then the system shall behave as in dictation (FR-27): continue from the Windows default input and notify (tz ВФ-13). | Given the selected USB microphone is unplugged mid-call, then the recording continues from the built-in microphone and the notice is shown. | Must | 2 / 1a |
| FR-43 | The system shall suppress the other party's echo in the mic track when the user talks without headphones; on by default, can be turned off (tz ВФ-14, ADR-008, decisions #172). | Given a call over loudspeakers, then in ≥ 90 % of cases the final transcript does not repeat the other party's lines as "Me" (value: working assumption CA-5). | Could | 2 / 1d |
| FR-44 | The system shall find "Me" lines of the final transcript that match a line of the other party in time and text (echo), hide them and let the user show them (tz ВФ-14а, ADR-008, decisions #172). | Given the "no headphones" test, then duplicates are hidden and "Show hidden" brings them back. | Should | 2 / 1d |
| FR-45 | When the system track carries no speech (silence, music), the system shall put no invented text into the subtitles or the transcript (VAD gate as in FR-12) (tz ВФ-15, roast-pkg А17). | Given 2 min of silence and 2 min of music without vocals, then 0 lines; a song with vocals may give text (known limitation). Depends on the Silero detector (OQ-51). | Must | 2 / 1a |
| FR-46 | While call mode is active, the system shall show the level of both tracks (in the indicator or the subtitle window); if the system track stays silent for more than 60 s while the mic track carries speech, then the system shall warn "Can't hear the other party — check the output device" (tz ВФ-16, roast-pkg А25). | Given the call audio is routed to a non-default device, then the warning appears after 60 s. | Must | 2 / 1a |
| FR-47 | While Record is on, the system shall write both tracks to disk incrementally (Ogg Opus, flushed at least every 10 s), so that a crash loses ≤ 10 s (tz ВФ-20, ADR-009). | Given the process crashes at minute 20, then at the next start the call is recovered with ≥ 19 min 50 s of audio. | Must | 2 / 1a |
| FR-48 | When the recording stops, the system shall close the track files (`mic.opus`, `system.opus`) and start the final transcription (tz ВФ-21). | Given stop, then the call folder holds two readable files and the call list shows "Transcribing…"; failure: if the final pass fails, then FR-65 applies. | Must | 2 / 1a |
| FR-49 | When an unfinished recording is found at start, the system shall recover the call, mark it "recovered after a crash" and offer to transcribe it (tz ВФ-22, ADR-009). | As FR-47: after a forced crash, the next start lists the call as recovered with the offer, and its files are readable. | Must | 2 / 1a |
| FR-50 | The system shall support calls of up to 4 h; at 4 h it shall stop with saving and notify "maximum length reached"; when free disk space is below what 30 min of recording needs, it shall warn, and below 100 MB stop and keep what was recorded (tz ВФ-23, roast-pkg А19; limits: working assumption CA-6). | Given an artificially small disk, then a warning, then a clean stop, and the files are readable; Given a 4 h recording from a fake source, then the automatic stop and the notice. | Must | 2 / 1a |
| FR-51 | When Record is off and only Subtitles is on, the system shall write no call audio or text to disk; audio is held in memory only while the current phrase is recognised (tz ВФ-24, ADR-009, decisions #164). | Given a 10-min subtitles-only session, then no new file with call audio or text appears in Voicen's data folders (logs carry no text). | Must | 2 / 1a |
| FR-52 | When the PC goes to sleep while recording, the system shall end the recording as a stop and notify on wake; on screen lock the recording continues (tz ВФ-25, roast-pkg А9). | Given sleep at minute 5, then after wake the call is saved and being transcribed; Given a 5-min lock, then the recording is continuous. | Should | 2 / 1a |
| FR-53 | When Subtitles is on, the system shall show the subtitle window: above all windows, never taking focus, movable, resizable, with position and size remembered (tz ВФ-30, ADR-007). | Given the window over Google Meet in a full-screen browser, then typing into Meet's chat is not interrupted. | Must | 2 / 1b |
| FR-54 | The system shall split each track into phrases by VAD (at most 8–10 s; long speech is cut) and recognise each phrase with the subtitle engine — recognition only: API, built-in or local server (tz ВФ-31, ADR-005, FR-66). | Given a 4 s phrase, then it appears in the window within success criterion 3 (NFR-13); failure: FR-58. | Must | 2 / 1b |
| FR-55 | The system shall label subtitle lines by track: "Me" — mic, "Other party" — system audio (tz ВФ-32, ADR-003, ADR-005). | Given a two-person dialogue, then the labels match who spoke. Two people speaking into the user's own headset both get "Me" (accepted limitation, ADR-003). | Must | 2 / 1b |
| FR-56 | The subtitle window shall show the last N lines (N = 2–4, a setting), ordered by phrase start time (tz ВФ-33). | Given overlapping speech, then both lines are visible in the right order. | Must | 2 / 1b |
| FR-57 | The user shall be able to turn on click-through for the subtitle window and set its font size and transparency (tz ВФ-34). | Given click-through on, then a click on the window reaches the window under it. | Should | 2 / 1b |
| FR-58 | If recognising a subtitle phrase fails (network, timeout), then the system shall show a "…" mark with the reason in the window, without stopping the recording or the next phrases (tz ВФ-35). | Given the network is off for 30 s, then marks appear, the recording is continuous, and subtitles resume when the network returns. | Must | 2 / 1b |
| FR-59 | When the engine returns a phrase's language, the system shall store it with the line (otherwise `lang = null`); settings hold a "primary language" that release 2 only stores (translation uses it later) (tz ВФ-36, ADR-012, roast-pkg А7). | Given an engine that returns the language (`verbose_json`) and primary language Russian, an English phrase gets `lang = en`; failure: with an engine that returns no language, `lang = null` and nothing else changes. | Must | 2 / 1b |
| FR-60 | When the recording stops, the system shall recognise both tracks again in full, with phrase times, using the final engine; live subtitles do not enter the final transcript (tz ВФ-40, ADR-006). | On the test set, WER of the final ≤ WER of the subtitles and ≤ 15 % Russian / ≤ 10 % English on the API engine (values: working assumption CA-7); every line has a start and an end; failure: FR-65. | Must | 2 / 1a |
| FR-61 | When subtitles ran for the whole call, the user shall be able to choose "quick final from subtitles" without a second recognition pass (tz ВФ-40а, roast-pkg Б5). | Given it is on, then the final is ready right after diarization. | Could | 2 / 1d |
| FR-62 | The system shall label speakers: every line of the mic track is "Me"; lines of the system track are "Other party 1…N" by diarization (tz ВФ-41, ADR-003, ADR-004). | Given a call with two remote participants, then "Other party 1" and "Other party 2" are separated per success criterion 2; failure: if diarization fails, then FR-65 applies. Until 1c the system track is one "Other party". | Must | 2 / 1c |
| FR-63 | The system shall merge the lines of both tracks into one transcript by time, keeping overlaps (two lines at the same time are both kept) (tz ВФ-42). | Given the user interrupts the other party, then both lines are in the transcript in start order. | Must | 2 / 1a |
| FR-64 | The system shall save the transcript as `transcript.json` (schema in §7, ADR-009) and `transcript.md` (time, speaker, text) (tz ВФ-43). | The files open, and the JSON validates against the schema. | Must | 2 / 1a |
| FR-65 | If the final transcription or the diarization fails, then the system shall keep the audio, show the state "failed" with the reason and offer "Transcribe again" (tz ВФ-44). | Given an invalid API key, then "failed: key"; after the key is fixed, "Transcribe again" produces the transcript. | Must | 2 / 1a |
| FR-66 | The user shall be able to choose, apart from the dictation engine, a subtitle engine and a final engine (recognition + diarization) for calls (tz ВФ-45, ВВ-2, roast-pkg А6). | Given dictation = API, subtitles = API, final = built-in + local diarization, then each step uses its own engine (OQ-53). | Should | 2 / 1b |
| FR-67 | When a cloud subtitle or final engine is selected, the system shall show, in settings and once at the first recording, "the other party's audio goes to provider <name>" (tz ВФ-45а, roast-pkg А23). | Given OpenAI is chosen for the final, then the warning is shown in settings and at the first recording. | Must | 2 / 1a |
| FR-68 | When the transcript is ready, the user shall be able to get a summary with the "Summary" button, made by the existing post-processing (FR-09) from a template: "work call" (decisions, tasks with owners and due dates, open questions, next steps) or "interview" (questions and answers, strong and weak points of the answers, what to prepare); every decision, task and question cites the time of its line `[mm:ss]` (tz ВФ-46, ВВ-3, roast-pkg А16). | Given a 30-min call and the "work call" template, then the sections are filled; a spot check of 5 citations: each leads to the line where it was said. | Should | 2 / 1d |
| FR-69 | The system shall save the summary in the call folder (`summary.md`) and show it in the call view; the templates are editable in settings (tz ВФ-47). | Given an edited template, then the next summary follows it. | Should | 2 / 1d |
| FR-70 | If post-processing is not configured or fails, or the transcript is longer than the model's context, then the system shall show the reason (for a long transcript: summarise in parts and merge them), and the transcript stays available (tz ВФ-48, roast-pkg А24). | Given no LLM key, then "Summary unavailable: post-processing is not configured"; Given a 2-h transcript and a small context, then a summary made in parts. | Should | 2 / 1d |
| FR-71 | The system shall show the list of calls: title (default "Call DD.MM HH:MM"), date and time, duration, state (recording, transcribing, ready, failed, recovered) (tz ВФ-50, concept). | Given 3 calls, then 3 rows, newest first. | Must | 2 / 1a |
| FR-72 | The user shall be able to open a call and read its transcript — time, speaker (colour and label), text — and search the text (tz ВФ-51). | Given a search for "contract", then the matching lines are highlighted. | Must | 2 / 1a |
| FR-73 | The user shall be able to rename a speaker within a call ("Other party 1" → "Colleague"); the name applies to all their lines in that call and to exports (tz ВФ-52). | Given a rename, then `transcript.md` and `transcript.json` show "Colleague". | Must | 2 / 1a |
| FR-74 | The user shall be able to merge two speakers when diarization split one person (tz ВФ-52а, roast-pkg А22). | Given "Other party 2" merged into "Other party 1", then all their lines carry one name, in exports too. | Should | 2 / 1c |
| FR-75 | The user shall be able to rename a call (tz ВФ-53). | The package gives no example; drafted: given a rename, then the list and `transcript.json` `title` show the new title (roast). | Must | 2 / 1a |
| FR-76 | The user shall be able to copy the transcript (Markdown), export Markdown and JSON to a chosen place, and open the call folder (tz ВФ-54). | Given "Copy", then the clipboard holds a readable transcript with speaker names. | Must | 2 / 1a |
| FR-77 | The user shall be able to delete a call: after confirmation the whole call folder (audio and text) is deleted (tz ВФ-55, NFR-17). | Given a deleted call, then its folder no longer exists. | Must | 2 / 1a |
| FR-78 | The user shall be able to play the call audio from a chosen line (both tracks together) (tz ВФ-56). | Given a click on a line, then playback starts at its start. | Should | 2 / 1d |
| FR-79 | The user shall be able to turn on automatic deletion of calls older than N days (default off: calls are kept until the user deletes them) (tz ВФ-57, ВВ-5). | Given N = 30, then calls older than 30 days are deleted at start. | Could | 2 / 1d |
| FR-80 | The system shall provide a "Calls" settings tab with: the call-mode hotkey; subtitle engine and final engine (FR-66, Should); output device to capture (default: the system default; Could); quick final from subtitles (FR-61, Could); primary language; echo suppression on/off (FR-43, Could) and hiding echo duplicates (FR-44, Should); subtitle window: number of lines, font, transparency; the reminder on/off; storage folder (default `%LOCALAPPDATA%\Voicen\calls`); auto-delete (FR-79, Could) (tz ВФ-60). | The package gives no example; drafted after FR-13: given a changed setting is saved, then it takes effect without a restart and persists across restarts (roast). | Must (items: own priority) | 2 / 1a (items with their increment) |

## 5. Non-functional requirements

| ID | Characteristic | Requirement (measurable) | How it is checked | Priority | Release |
|---|---|---|---|---|---|
| NFR-01 | Performance | Over a fixed set of 20 phrases of 5–15 s (committed to the repo), p90 stop → paste ≤ 3 s via the API engine (OpenAI `whisper-1` and Groq `whisper-large-v3-turbo`, both measured, on a ≥ 20 Mbit/s connection), and ≤ 10 s via the built-in local `small` model on the reference machine (the owner's Windows PC, decisions #84); without LLM post-processing (r1#1). Each listed API provider must meet p90 ≤ 3 s on its own; the local figure is measured warm (model loaded), the cold first dictation is logged but not counted (r2#6). | Benchmark run from the log timings (FR-20) on the owner's PC per release. | Must | 1 |
| NFR-02 | Performance | The microphone is opened only on hotkey press (the Windows "microphone in use" indicator shows only while recording); hotkey press → capture start p95 ≤ 200 ms on a wired or USB microphone; Bluetooth headsets may lose the first ~0.5 s (accepted) (r1#11). | Automated timing test on the Windows runner with the injected audio source; manual check on the owner's PC. | Must | 1 |
| NFR-03 | Resource use | Idle in tray with no local model loaded and no window open: CPU < 1 %, RAM ≤ 150 MB, counted as the sum of the private working set of the app process and its WebView2 child processes; the overlay and settings webviews are created on demand and destroyed when closed (r2#5). With the built-in engine selected, the model stays loaded after use and is unloaded after 10 min idle (r1#13). RAM while a model is loaded is not bounded; settings show each model's size (v4). | Measured on the reference machine; unit test of the unload timer. | Should | 1 |
| NFR-04 | Security | API keys are stored in Windows Credential Manager (r1#23), never in plain-text config, logs or crash reports. | Unit test on storage; log redaction test; secret scanner in CI. | Must | 1 |
| NFR-05 | Privacy | With the built-in local engine and post-processing off, no audio or text leaves the machine (no network calls except model download and an explicit "Test connection"). No telemetry. | Test with network mocked/blocked; code review of network calls. | Must | 1 |
| NFR-06 | Privacy | Each recording's audio is kept in memory or a temp file until its result is delivered; a failed one becomes the pending recording, and only a newer failed one replaces it (at most one pending) (r2#2). Audio is deleted on exit, and stale temp audio is deleted at the next start after a crash (r1#24). History stores text only, and only when enabled. | Integration test on the temp dir; manual check. | Must | 1 |
| NFR-07 | Reliability | No crash in 2 weeks of daily use by the owner (no crash file per FR-20 in that period); a failure in any engine never crashes the app. An OS shutdown/logoff or the installer closing the app is a clean exit, not a crash (v4). | Crash files in `%LOCALAPPDATA%\Voicen\logs`; fault-injection tests per engine. | Must | 1 |
| NFR-08 | Compatibility | Windows 11 x64 checked per release; Windows 10 22H2 best-effort, not checked per release (r1#25). Auto-paste works in: Notepad, Chrome and Edge text fields, VS Code, Telegram Desktop, Microsoft Word. | Manual checklist on the owner's PC per release (r1#19); automated paste test into a test window on the CI Windows runner. | Must | 1 |
| NFR-09 | Portability / install | Installer (.exe) from GitHub Releases installs per-user without admin rights; installed size ≤ 100 MB excluding local models. | CI build artifact and silent-install smoke test on the Windows runner; install on a clean Windows 11 in Windows Sandbox on the owner's PC per release (r1#19). | Must | 1 |
| NFR-10 | Usability | First run: from launching the installer to the first pasted transcript ≤ 3 minutes with an API key at hand. | Manual run-through in Windows Sandbox. | Should | 1 |
| NFR-11 | Maintainability | Engines are behind one interface; adding a new provider does not change recording or delivery code. | Architecture review; tests per engine with a fake. | Should | 1 |
| NFR-12 | Legal | The project is MIT-licensed (r1#20); every bundled component's license (whisper.cpp MIT, models' licenses, crates and npm packages) is compatible and listed. | License check in CI. | Must | 1 |

### Release 2 — call mode: non-functional requirements

| ID | Characteristic | Requirement (measurable) | How it is checked | Priority | Release |
|---|---|---|---|---|---|
| NFR-13 | Performance | Subtitles: end of phrase → shown, p90 ≤ 4 s via the API engine and ≤ 6 s via the built-in `small` model on the owner's PC (success criterion 3, CA-2). Final transcript of a one-hour call ≤ 5 min via the API and ≤ 30 min locally (`small`, owner's PC) (success criterion 4, CA-3) (tz ВН-01, roast-pkg А18). | Log timings on the test set and on the owner's PC. | Must | 2 / 1b (subtitles), 1d (local) |
| NFR-14 | Resource use | Recording without subtitles: CPU ≤ 5 % on the reference machine (the owner's PC, decisions #84), memory growth ≤ 50 MB per hour; with subtitles on the built-in engine the model stays loaded as in NFR-03 (tz ВН-02; values: CA-8). | Measured on the reference machine. | Should | 2 / 1d |
| NFR-15 | Reliability | A failure of capture, an engine or diarization never crashes the app and never loses what was recorded (FR-47, FR-65); 2 weeks of daily use without a crash (success criterion 5) (tz ВН-03). | Fault-injection tests; crash files (FR-20). | Must | 2 / 1a |
| NFR-16 | Privacy | With built-in engines, local diarization and post-processing off or local, no call audio or text leaves the machine. Logs and crash reports never contain call text or audio (as FR-20, NFR-04) (tz ВН-04, roast-pkg А8). | Test with the network blocked; log-cleanliness test. | Must | 2 / 1a |
| NFR-17 | Privacy | Subtitles-only writes nothing to disk (FR-51); deleting a call deletes all of it (FR-77) (tz ВН-05). | Integration test over the data folder. | Must | 2 / 1a |
| NFR-18 | Size | Audio is Opus, mono 16 kHz; both tracks together ≤ 40 MB per hour (tz ВН-06; value: CA-9). | Test on a 60-min recording. | Should | 2 / 1a |
| NFR-19 | Compatibility | Full matrix once before the release is final: Telegram Desktop, Google Meet in Chrome and Edge, Zoom, Microsoft Teams; USB headset, Bluetooth headset (with the switch to its headset profile), a laptop's built-in speakers and microphone. Per delivery: the owner's short set (their real apps and headset) (tz ВН-07, roast-pkg Б8). | Owner's manual checklist. | Must | 2 / 1a |
| NFR-20 | Maintainability | Capture, recognition, diarization and storage sit behind core interfaces; a new diarization engine or audio source is added without changing the rest of the chain (tz ВН-08, ADR-011 proposed). | Architecture review; tests with fakes. | Should | 2 / 1d |
| NFR-21 | Legal | Every new component (WASAPI binding, Opus, diarization models, echo suppression) is compatible with Voicen's MIT licence and listed; model licences are checked one by one (tz ВН-09). | License check in CI (`make licenses`). | Must | 2 / 1a |
| NFR-22 | Testability | Both tracks can be replaced by WAV files (as the injected source of dictation tests). The chain capture → subtitles → final (segmentation, sync, merge, storage, recovery) is checked on the Linux host with fakes; native engines (whisper.cpp, sherpa-onnx, echo suppression) in Windows CI (decisions #12); real loopback and microphone capture only on the owner's PC — the `windows-latest` runner has no audio devices (a virtual audio driver in CI: OQ-62) (tz ВН-10, roast-pkg Б7). | Core tests, Windows CI, owner's manual check. | Must | 2 / 1a |

## 6. Constraints

- **Platform:** Windows 10/11 x64 only in release 1 (i1).
- **Distribution:** public open-source on GitHub under MIT (r1#20); installer in GitHub Releases; no code signing in release 1 (decisions #83).
- **Development host:** Linux (this machine). Windows-specific behaviour (global hotkey, input simulation, clipboard, tray) is built and tested on the GitHub Actions Windows runner; manual verification on a real Windows machine by the owner.
- **Install policy (i2):** packages, toolchains, Docker images and MCP servers may be installed on the dev host and in CI, each only with the owner's explicit consent. CI: GitHub Actions, including the Windows runner.
- **Host now has:** Node 20, pnpm, Python 3.12, uv, Docker, make, gcc, gh. No Rust, Go, .NET, CMake, Wine.
- **Costs:** the user brings their own API keys; the project pays for nothing but free CI minutes.
- **Team:** the owner + AI agents; manual Windows checks by the owner on their PC (r1#26).
- **Deadline:** when ready (decisions #82). If the date turns out tight, cut first: FR-16, FR-14, FR-19, NFR-10, then the Russian UI of FR-15 (roast round 1, scope).

### Release 2 — call mode: constraints

- **Platform and verification:** Windows 10/11 x64 only, developed on the Linux host, Windows behaviour proven in Windows CI and on the owner's PC, as for release 1 (tz §7).
- **No backend of ours:** the user's own API keys or local engines; an own server of the owner plugs in as an OpenAI-compatible local or network server (tz §7, ВВ-2).
- **Real call recordings are never committed to git** (biometrics, confidentiality); test data are open corpora (outside git, fetched by checksum) and synthetic dialogues with stock synthetic voices — never clones of real people (tz §7, §8; decisions #173).
- **No audio devices on the CI Windows runner:** real capture is checked only by the owner (NFR-22).
- **New dependencies** (WASAPI binding, Opus encoder, sherpa-onnx, echo suppression) each need the owner's consent (tz §6; §6 install policy above; OQ-63).
- **Dictation stays as is:** call mode reuses the engines, VAD, post-processing, storage, log and installer of release 1 (ADR-001); the call engines are chosen apart from the dictation engine (FR-66).
- **Built on unfinished release-1 parts:** the built-in engine (T-017) and the Silero detector in the installer (T-102) are not shipped yet; increment 1d depends on them (tz §6, OQ-51).
- **Deadline:** not set; the increments 1a → 1d fix the order (decisions #171).

## 7. Glossary and data

| Term | Meaning |
|---|---|
| Recording | Audio captured from the selected microphone between hotkey start and stop. |
| Engine | The transcription backend: OpenAI-compatible API, built-in local (whisper.cpp), or local server (OpenAI-compatible URL on the user's machine). |
| OpenAI-compatible API | An HTTP API implementing `POST /v1/audio/transcriptions` (and `/v1/chat/completions` for post-processing) as OpenAI does: OpenAI, Groq, speaches, etc. |
| Local model | A whisper.cpp ggml model file (tiny, base, small, medium q5_0, large-v3-turbo q5_0; about 75–550 MB, r2#10) downloaded to the user's machine. |
| Post-processing | Optional LLM step that rewrites the transcript using the user's prompt. |
| Push-to-talk (hold) | Recording while the hotkey is held. |
| Toggle | First press starts recording, second press stops. |
| Auto-paste | Simulated Ctrl+V into the window that was focused at recording start. |
| Transcript | Text returned by the engine (raw), or after post-processing (final). |
| History entry | Final text, timestamp, engine name; no audio. |
| VAD | Voice-activity detection: decides whether a recording contains speech before it is sent (FR-12). |
| Start window | The window that had keyboard focus when recording started; the paste target (FR-10). |

**Storage:** all app data (settings, history, models, logs) in `%LOCALAPPDATA%\Voicen`; keys in Windows Credential Manager (FR-28, NFR-04).

**Main entities:** `Settings` (engine, base URL, model, language, mic, hotkey, mode, auto-paste, post-processing config, history config, UI language, autostart); `Secret` (API keys, in the OS credential store); `LocalModel` (name, size, path, checksum, state); `HistoryEntry`; `PendingRecording` (audio of the last failed attempt).

### Release 2 — call mode: glossary and data

| Term | Meaning |
|---|---|
| Call mode | The mode with the two toggles "Record" and "Subtitles"; active while at least one is on (FR-30). |
| Call | One call-mode session with Record on: its two tracks, transcript and metadata in one call folder. |
| Track | One of two never-mixed audio streams: `mic` (the selected microphone, the user = "Me") and `system` (loopback of the default output device = the other party). |
| Loopback | Capture of what the default output device plays (WASAPI loopback). |
| Drift | The rate difference between the clocks of the two audio devices (±50 ppm ≈ 180 ms per hour), compensated per FR-40. |
| Subtitle engine | Recognition-only engine for live phrases: API, built-in or local server (FR-54, FR-66). |
| Final engine | Engine of the final pass: recognition with phrase times plus diarization (FR-60, FR-62, FR-66). |
| Final pass | Recognition of both whole tracks after stop; the source of the final transcript (FR-60). |
| Diarization | Splitting the system track into "Other party 1…N"; the mic track is always "Me" (FR-62). |
| Segment (line) | One recognised phrase: start, end, track, speaker, language, text. |
| Speaker | `me` or `s1…sN`, with a display name the user can change per call (FR-73). |
| Echo duplicate | A "Me" line that repeats a line of the other party picked up by the microphone without headphones (FR-44). |
| Primary language | A setting stored for the later translation feature (FR-59). |
| Summary | Template-based LLM result of a call, `summary.md` (FR-68). |
| DER / WER | Diarization error rate / word error rate, the accuracy measures of success criterion 2 and FR-60. |

**Storage (ADR-009, proposed; OQ-47):** call folders under `%LOCALAPPDATA%\Voicen\calls\<YYYY-MM-DD_HHMMSS>\`; the root can be changed in settings (FR-80). Subtitles-only writes nothing (FR-51, decisions #164). Logs hold no call text or audio, only durations, sizes, device events and error codes (NFR-16).

**Stored call layout:**

| File | When | Content |
|---|---|---|
| `mic.opus`, `system.opus` | written incrementally while recording (flushed ≤ every 10 s), closed at stop | Ogg Opus, mono 16 kHz, about 16–24 kbit/s (FR-47, NFR-18) |
| `meta.json` | from the start | state `recording` → `ready` / `failed` / `recovered`; start, end, duration, mode flags, devices and device changes, engines, app version, speaker names |
| `transcript.json` | after the final pass | schema version 1 (below) (FR-64) |
| `transcript.md` | after the final pass | readable: time, speaker, text (FR-64) |
| `summary.md` | on "Summary" | the summary (FR-69) |

`transcript.json`, schema 1 (ADR-009): `{ "schema": 1, "call_id", "title", "started_at" (ISO-8601), "duration_ms", "speakers": [{"id": "me" | "s1"…, "name"}], "segments": [{"start_ms", "end_ms", "track": "mic" | "system", "speaker", "lang" (or null), "text"}], "engines": {"transcription", "diarization"} }`. The schema is versioned; it is the entry point for agents and future integrations.

**Main entities (tz §4):** `Call` (id, title, start, end, duration, state `recording` / `transcribing` / `ready` / `failed` / `recovered`, mode flags, devices with changes, engines, app version, speaker names); `Track` (`mic` / `system`, file, rate, start offset from `Call.start`); `Segment` (start_ms, end_ms, track, speaker_id `me` / `s1…sN`, lang, text, confidence if given, source `final`); `Speaker` (id, display name, default "Me" / "Other party N"); `CallSettings` (hotkey, last toggle combination, engines, primary language, echo, subtitle window, reminder, folder, auto-delete).

## 8. Integrations and external dependencies

| Dependency | Use | Account / cost | Notes |
|---|---|---|---|
| OpenAI-compatible transcription API (OpenAI, Groq, …) | FR-06 | user's own key, paid per use | Rate limits per provider; max upload size (OpenAI: 25 MB). |
| OpenAI-compatible chat API | FR-09 | user's own key | Can be the same or a different provider than transcription. |
| Local OpenAI-compatible server (speaches, faster-whisper-server, …) | FR-17 | none | Run by the user. |
| whisper.cpp | FR-07 | none, MIT | Bundled library/binary. |
| Whisper ggml models (Hugging Face `ggerganov/whisper.cpp`) | FR-08 | none | Downloaded on demand; about 75–550 MB (r2#10). |
| Silero VAD model (ggml) | FR-12 | none, MIT | Bundled in the installer (< 1 MB) (r2#3). |
| Windows APIs | hotkey (RegisterHotKey), input simulation (SendInput), clipboard, Credential Manager, autostart, toast notifications | none | Toasts for an unpackaged app need an AppUserModelID / Start-menu shortcut created by the installer (r1#8). |
| GitHub (repo, Actions, Releases) | CI, distribution | free for public repos | |

### Release 2 — call mode: integrations and external dependencies

| Dependency | Use | Account / cost | Notes |
|---|---|---|---|
| Windows WASAPI loopback (crate `wasapi` or `windows`) | FR-39, FR-41 | none | Captures only the default output device (FR-46 warns otherwise); a new dependency needs consent (OQ-63). |
| Opus encoder + Ogg container | FR-47, NFR-18 | none | New native/crate dependency; licence per NFR-21 (OQ-63). |
| sherpa-onnx (pyannote-3.0 segmentation + speaker-embedding models, clustering) | FR-62 local diarization, default (decisions #162) | none | Models downloaded on demand like whisper models; binding and model licences open (OQ-61). |
| OpenAI `gpt-4o-transcribe-diarize` (Transcription API) | FR-62 cloud final engine | user's own key, paid per use | Speakers guaranteed only for a system track that fits one upload chunk; longer → local diarization (decisions #162). Limits and price unverified (OQ-60). Never used for subtitles. |
| OpenAI-compatible transcription API / local server / built-in whisper.cpp (release 1) | FR-54, FR-60 | as in release 1 | Phrase times and language need a richer response than today's client (OQ-54). Built-in engine: T-017 (OQ-51). |
| OpenAI-compatible chat API (release 1 post-processing) | FR-68…FR-70 | user's own key | Summaries; cloud LLM sends call text out (NFR-16). |
| WebRTC echo suppression (e.g. `webrtc-audio-processing`) | FR-43 (Could) | none | Windows build and BSD licence unverified (OQ-46). |
| Open corpora of real recordings (AMI Meeting Corpus, VoxConverse; a Russian set to be found) | success criterion 2, FR-60 tests | none | Outside git, fetched in CI by checksum; licences unverified (OQ-59). |

## 9. Technical decisions

<!-- Proposed by the requirements-reviewer during the roast, confirmed by the owner. -->

| Decision | Choice | Driven by | Alternative |
|---|---|---|---|
| Stack and areas | **Tauri 2**, areas `core` (Rust: `windows` crate for hotkey, SendInput, clipboard, Credential Manager, autostart; `cpal` capture; `whisper-rs` in-process whisper.cpp with VAD; `reqwest` OpenAI-compatible client) and `ui` (TypeScript + Svelte via Vite, pnpm); NSIS per-user installer via the Tauri bundler | NFR-09, NFR-03, FR-07, FR-12, NFR-04, NFR-11, FR-13, FR-15, §6 | .NET 8 WPF + Whisper.net (one area; UI cannot run on the Linux host, no UI verification adapter); Electron rejected (NFR-09 size, NFR-03 RAM) |
| What "deployed" means | Per task: the `windows-latest` CI job builds the commit's NSIS installer, installs it silently per-user, launches the app and checks that the start log line carries the commit (FR-18, FR-20). Per release: a `v*` tag publishes that installer to GitHub Releases | §2 success 3, NFR-09, FR-18 | A portable zip per commit, installer only at release time |
| Where verification runs | Linux host: `core` platform-independent logic (engine clients against a mock OpenAI-compatible server, model download and checksum, retry and pending state, VAD gate, post-processing fallback, settings, history, ordering) with Windows code behind `cfg` and trait fakes; `ui` with vitest and Playwright. GitHub Actions `windows-latest`: full `cargo test` incl. Windows APIs (clipboard, Credential Manager, hotkey conflict, SendInput into a test window), whisper.cpp transcribing a bundled WAV with a cached `tiny` model, installer smoke test; recordings come from an injectable WAV audio source. Owner's Windows PC: real microphone and hotkeys, NFR-08 checklist, NFR-01 benchmark, clean-Win11 install in Windows Sandbox | §6, NFR-01, NFR-02, NFR-08, FR-02–FR-12, FR-26 | A self-hosted runner on the owner's Windows 11 PC (real mic in CI; needs the PC online) |
| Installs needing consent | Host: Rust via a `rust` Docker image (no host install), with cmake and clang in that image; `@playwright/test` with its Chromium. CI: the Rust toolchain action on `windows-latest` (MSVC and CMake preinstalled). Each install is still asked before it runs (r2#13) | §6 install policy, Stack row | rustup + cmake/clang installed on the host |
| UI verification | **playwright** for the web-UI screens (settings, first run, history, overlay) in Chromium against a mocked Tauri IPC layer, on the Linux host and in CI. Tray, global hotkey, toasts and auto-paste have no kit adapter: tasks touching them carry a reviewed `verify_exception` with the owner's manual check on Windows, plus Windows-runner integration tests as evidence | FR-13, FR-15, FR-16, FR-21, FR-04, FR-10, §6 | `none` with a recorded deferral (all UI checks manual); tauri-driver on the Windows runner (no kit adapter) |

### Release 2 — call mode: technical decisions (owner decisions)

Only the owner-accepted ADRs and ADR parts of the package (all decided by the owner on 2026-10-21). Their proposed parts are not decisions: they are working assumptions in §10 and questions OQ-41…OQ-49.

| Decision | Choice | Driven by | Alternative |
|---|---|---|---|
| Where call mode lives (ADR-001, tz ВВ-1; decisions #159) | **Inside Voicen**, same public MIT repository; shared parts stay in `voicen-core` behind interfaces; closed niche shells may later be built on the same core | §1 users, ADR-011 | A separate closed app on `voicen-core` (second installer and CI, no showcase) |
| What is captured (ADR-002, owner part; decisions #160) | **Speakers and microphone as two separate tracks**; how (WASAPI crate, QPC timestamps, adaptive resampling) is proposed (OQ-41) | FR-39, FR-40, FR-62 | Virtual audio cable; a meeting bot; one mixed track |
| Speakers (ADR-003, owner part; decisions #161) | **Diarization is mandatory**: "Me" and "Other party 1…N"; the track-based method is proposed (OQ-42) | FR-55, FR-62 | — |
| Diarization and final engines (ADR-004, tz ВВ-2, П-5; decisions #162) | A core `Diarizer` trait and a separate final-recognition trait with phrase times; two implementations: **local sherpa-onnx (default)** and **cloud OpenAI `gpt-4o-transcribe-diarize`** as its own call engine; cloud diarization only for a system track that fits one chunk until a spike, otherwise local; the owner's own server later if local quality falls short; subtitles never use the diarizing engine | FR-60, FR-62, FR-66, NFR-16 | The owner's own WhisperX/pyannote server (breaks "no backend of ours") |
| Live subtitles in this release (ADR-005, owner part; decisions #163) | **Subtitles are in release 2**; the VAD-phrase method is proposed (OQ-43) | FR-53…FR-59 | — |
| Subtitles-only stores nothing (ADR-009, owner part; decisions #164) | With Record off, **nothing is written to disk**; phrase audio lives in memory until its text arrives | FR-51, NFR-17 | — |
| Consent of the other party (ADR-010, tz ВВ-6; decisions #165) | Recording is always visible (indicator, tray); **a reminder** at each start, can be turned off; the docs say the user is responsible for consent and give a sample opening phrase; no secret mode; a consent journal comes with voice profiles in a later release | FR-34, FR-35 | — |
| Language (ADR-012, owner part; decisions #166) | **Translation comes in a later release, by the "primary language"**; storing `lang` per phrase now is proposed (OQ-49) | FR-59 | — |

## 10. Assumptions, risks, open questions

| Assumption or risk | Status | What it affects |
|---|---|---|
| Users bring their own API keys; no hosted backend of ours. | confirmed (i2) | FR-06, FR-09, NFR-05 |
| Minimum hold of 0.3 s to count as a recording. | working assumption | FR-02 |
| Whisper hallucinates text on silence/noise; a VAD gate before sending prevents pasting garbage. | confirmed (r1#2) | FR-12 |
| Global hotkey via RegisterHotKey rather than a low-level keyboard hook where possible (lower antivirus heuristics, survives hook timeouts). | working assumption | FR-05, FR-26, OQ-02 |
| API keys in Windows Credential Manager. | confirmed (r1#23) | NFR-04 |
| No code signing; SmartScreen warning accepted in release 1. | decided (decisions #83) | NFR-09 |
| Auto-paste into elevated (admin) windows is impossible from a non-elevated app; text stays in clipboard. | working assumption | FR-10 |
| Pre-mortem: auto-paste unreliable in some apps (Electron, games, RDP) → users lose trust. | risk | FR-10, NFR-08 |
| Pre-mortem: local engine too slow on CPU for `medium`/`large` → users think the app is broken. | risk; mitigated by showing a recommended model and progress | FR-07, NFR-01 |
| Pre-mortem: global hotkey collides with other apps or is swallowed by antivirus/keyboard hooks. | risk | FR-05 |
| Pre-mortem: Windows behaviour cannot be tested on the Linux dev host → regressions reach users. | risk; mitigated by Windows CI runner and owner's manual check | NFR-08, section 9 |
| Pre-mortem: hotkey or recording silently stops after sleep/resume or a headset change, with no way to diagnose. | risk; mitigated by FR-20, FR-26, FR-27 | NFR-07 |
| Pre-mortem: Ctrl+Alt+V instead of Ctrl+V lands in the target because modifiers are still held. | risk; mitigated by FR-10 (wait for modifier release) | FR-10 |
| Pre-mortem: unsigned installer flagged by SmartScreen/antivirus (keyboard hook + input simulation looks like a keylogger). | risk | NFR-09, OQ-02 |

Open questions: none; OQ-01 → decisions #82, OQ-02 → #83, OQ-03 → #84, OQ-04 answered (10 min, r1#12).

### Release 2 — call mode: assumptions, risks, open questions

**Working assumptions (the package's (Р) values; checked in the spec, the spikes and by measurement):**

| ID | Working assumption | What it affects |
|---|---|---|
| CA-1 | DER ≤ 20 % on the system track with 2–3 remote participants, on an open corpus of real recordings (tz §1 crit. 2, П-4) | success criterion 2, FR-62 |
| CA-2 | Subtitle latency p90 ≤ 4 s (API) / ≤ 6 s (built-in `small`, owner's PC) (tz §1 crit. 3) | NFR-13, FR-54 |
| CA-3 | Final transcript of a one-hour call ≤ 5 min (API) / ≤ 30 min (local `small`, owner's PC) (tz §1 crit. 4) | NFR-13, FR-60 |
| CA-4 | Track misalignment ≤ 100 ms after 60 min with drift compensation (tz ВФ-11) | FR-40 |
| CA-5 | Echo suppression keeps ≥ 90 % of the other party's lines from repeating as "Me" (tz ВФ-14) | FR-43 |
| CA-6 | 4 h maximum call; warn below the space for 30 min of recording; stop below 100 MB (tz ВФ-23) | FR-50 |
| CA-7 | WER of the final ≤ 15 % Russian / ≤ 10 % English on the API engine (tz ВФ-40) | FR-60 |
| CA-8 | Recording without subtitles: CPU ≤ 5 %, memory growth ≤ 50 MB/h (tz ВН-02) | NFR-14 |
| CA-9 | Both Opus tracks ≤ 40 MB per hour (tz ВН-06) | NFR-18 |
| CA-10 | The proposed parts of ADR-002, 003, 005, 006, 007, 008, 009, 011, 012 hold as written until the owner confirms or the spec refines them (OQ-41…OQ-49) | FR-39…FR-64, NFR-20 |

**Risks (tz §9, roast-pkg):**

| Risk | Status | What it affects |
|---|---|---|
| Echo without headphones: the other party's voice enters the mic → duplicate lines, wrong "Me". | risk; mitigated by FR-44, FR-43 and the headphones hint (FR-35) | FR-62, FR-63 |
| The call app plays to a non-default output device → no other-party audio. | risk; mitigated by FR-46 and the output-device setting (FR-80, Could) | FR-39 |
| Overlapping speech and many participants → DER above target. | risk; the target is set for 2–3 participants; voice profiles later | success criterion 2 |
| API subtitle latency on a poor network. | risk; mitigated by the built-in engine and error marks (FR-58) | NFR-13 |
| API cost: two tracks × an hour, subtitles plus final. | risk; only VAD speech segments are recognised; quick final (FR-61); a cost estimate in settings was named as Could but is not a requirement (roast) | FR-54, FR-60 |
| Local diarization quality on real voices; the clustering threshold is calibrated on synthetic data. | risk; threshold in settings for debugging; calibration on the owner's own set | FR-62 |
| Laws on recording conversations (consent). | risk for the user; mitigated by FR-35 and the docs (decisions #165) | FR-35 |
| Subtitle window: "non-activating" and "draggable" at once may fail (it steals focus or will not move). | risk; spike first, fallback: topmost window that returns focus (ADR-007, OQ-45) | FR-53 |
| Clock drift between microphone and output device. | risk; mitigated by FR-40 | FR-40, FR-43 |
| A Bluetooth headset switches to its headset profile when its mic opens → frequent output-device changes. | risk; mitigated by FR-41, FR-46, NFR-19 | FR-41 |
| Cloud diarization of long calls: speaker labels do not match across chunks. | risk; cloud diarization only for one chunk until a spike (decisions #162, OQ-60) | FR-62 |
| The built-in engine (T-017) and Silero in the installer (T-102) are not ready → "local by default" delays the release. | risk; mitigated by delivering 1a–1c on the API engine (decisions #171; OQ-51) | 1d |
| No audio devices in CI → capture is not checked automatically. | risk; fake sources plus the owner's manual check (NFR-22, OQ-62) | FR-39…FR-42 |
| Full-screen exclusive apps and games may cover the subtitle window. | accepted limitation (ADR-007, proposed) | FR-53 |
| Two people speaking into the user's own headset both get "Me". | accepted limitation (ADR-003) | FR-55, FR-62 |

**ID map (package → this document):**

| Package | Here | Package | Here | Package | Here | Package | Here |
|---|---|---|---|---|---|---|---|
| ВФ-01 | FR-30 | ВФ-14 | FR-43 | ВФ-33 | FR-56 | ВФ-47 | FR-69 |
| ВФ-02 | FR-31 | ВФ-14а | FR-44 | ВФ-34 | FR-57 | ВФ-48 | FR-70 |
| ВФ-03 | FR-32 | ВФ-15 | FR-45 | ВФ-35 | FR-58 | ВФ-50 | FR-71 |
| ВФ-04 | FR-33 | ВФ-16 | FR-46 | ВФ-36 | FR-59 | ВФ-51 | FR-72 |
| ВФ-05 | FR-34 | ВФ-20 | FR-47 | ВФ-40 | FR-60 | ВФ-52 | FR-73 |
| ВФ-06 | FR-35 | ВФ-21 | FR-48 | ВФ-40а | FR-61 | ВФ-52а | FR-74 |
| ВФ-07 | FR-36 | ВФ-22 | FR-49 | ВФ-41 | FR-62 | ВФ-53 | FR-75 |
| ВФ-08 | FR-37 | ВФ-23 | FR-50 | ВФ-42 | FR-63 | ВФ-54 | FR-76 |
| ВФ-09 | FR-38 | ВФ-24 | FR-51 | ВФ-43 | FR-64 | ВФ-55 | FR-77 |
| ВФ-10 | FR-39 | ВФ-25 | FR-52 | ВФ-44 | FR-65 | ВФ-56 | FR-78 |
| ВФ-11 | FR-40 | ВФ-30 | FR-53 | ВФ-45 | FR-66 | ВФ-57 | FR-79 |
| ВФ-12 | FR-41 | ВФ-31 | FR-54 | ВФ-45а | FR-67 | ВФ-60 | FR-80 |
| ВФ-13 | FR-42 | ВФ-32 | FR-55 | ВФ-46 | FR-68 |  |  |
| ВН-01 | NFR-13 | ВН-04 | NFR-16 | ВН-07 | NFR-19 | ВН-10 | NFR-22 |
| ВН-02 | NFR-14 | ВН-05 | NFR-17 | ВН-08 | NFR-20 |  |  |
| ВН-03 | NFR-15 | ВН-06 | NFR-18 | ВН-09 | NFR-21 |  |  |

Open questions (release 2): OQ-41…OQ-49 (proposed ADRs), OQ-50…OQ-58 (package vs repository), OQ-59…OQ-63 (unresolved items of the package) in `docs/open-questions.md`.
