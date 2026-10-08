# Requirements

<!-- The product's requirements document: what must be true, stated so it can be tested.
     Built and roasted by /teamwright:requirements; the spec tool and the tasks are derived from it.
     Write only what the owner said or approved. A guess is marked "working assumption" in section 10;
     an unknown goes to docs/open-questions.md. IDs (FR-NN, NFR-NN) are never reused. -->

Version: 4 · Status: approved · Approved by the owner: 2026-10-02 (v4: spec gaps closed, decisions #7) · Roast: round 2 NOT_READY (on v2), remaining High closed by the owner (`docs/requirements.reviews/`)
Source: owner's brief in the /teamwright:init call (2026-10-02, Russian), interview rounds 1–3 (2026-10-02), roast round 1–2 answers (2026-10-02), spec findings accepted by the owner (2026-10-02, `(v4)`)

Pointers: `(brief)` — the owner's brief; `(iN)` — interview round N; `(r1#N)`, `(r2#N)` — roast round 1/2 finding N, accepted by the owner.

## 1. Problem and users

**Product (working name):** Voicen — a desktop voice-to-text tool: record speech from the microphone, transcribe it, put the text into the clipboard and paste it into the active input field (brief).

**Users:** anyone on Windows who prefers dictating to typing — writing messages, prompts for AI assistants, notes, documents. Distributed as public open-source software (i1).

**Jobs To Be Done:**
- When I am working in any application and want to enter text, I want to press a hotkey, speak and have the text appear in the field I was typing in, so I can enter text faster than typing without switching windows (brief, i1).
- When I care about privacy or have no internet, I want transcription to run on my own machine, so my voice never leaves the computer (brief, i2).
- When I want the best quality or speed, I want to plug in a cloud API of my choice with my own key, so I can pick the provider and pay only for what I use (brief, i2).

**Today without the product:** the user types by hand, or uses the built-in Windows voice typing (Win+H), or records audio and transcribes it in a separate service, then copies the text manually.

## 2. First release: goal and success

**The one flow that must work end to end:** the app runs in the background → the user focuses an input field in any application → presses the global hotkey (hold or toggle) → speaks → stops → the audio is transcribed by the configured engine (OpenAI-compatible API, built-in local whisper.cpp, or a local OpenAI-compatible server) → optionally post-processed by an LLM → the text is copied to the clipboard and pasted into the field that was focused.

**Success (all three criteria, i3, r1#21):**
1. **Speed:** NFR-01 is met — p90 stop→paste ≤ 3 s via the API engine and ≤ 10 s via the built-in local `small` model, over the fixed 20-phrase sample set, without LLM post-processing.
2. **Daily use:** the owner uses it daily for 2 weeks without a crash; checked by the crash files and the log (FR-20) being free of crash records for that period.
3. **Public release:** a Windows installer published in GitHub Releases installs and completes the flow on a clean Windows 11 (Windows Sandbox on the owner's PC, r1#19).

**First run (r1#4):** on first launch the settings window opens on the Engine tab with engine = none; the user picks an engine (enters a key or downloads a model) and can dictate immediately with the defaults (FR-21).

Target date: when ready (decisions #82).

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
| FR-09 | When LLM post-processing is enabled, the system shall send the transcript and the user's editable prompt to the configured OpenAI-compatible chat endpoint and use the returned text instead of the raw transcript. | Given post-processing on with prompt "fix punctuation", raw "привет как дела" → pasted "Привет, как дела?"; failure: if the LLM call fails or exceeds 15 s (FR-24), then the system shall paste the raw transcript and notify that post-processing was skipped. | Must | 1 |
| FR-10 | When the final text is ready, the system shall put it in the clipboard (marked as excluded from Windows clipboard history and cloud sync, r1#28) and, if auto-paste is on, wait until all modifier keys are released (≤ 1 s) and send Ctrl+V to the window that had focus when recording started (r1#17). The hotkey press and release are not delivered to the focused application (r2#7). | Given Notepad was focused at start and auto-paste is on, then the text appears in Notepad and is also in the clipboard, and Win+V clipboard history does not list it (r2#8); Given auto-paste is off, then the text is only in the clipboard and a notification says so; failure: if the start window is no longer in front, or its process runs elevated, then the system shall leave the text in the clipboard and notify "copied — paste manually". | Must | 1 |
| FR-11 | If transcription fails (no network, timeout, HTTP error, engine error), then the system shall paste nothing, notify the user with the reason, and keep the audio of the last failed recording so the user can retry from the notification or the tray. | Given network off and the API engine, then a notification "network unavailable — Retry" appears; when the network returns and the user clicks Retry, then the text is delivered per FR-10 — pasted only if the start window is in front again, otherwise copied with "copied — paste manually" (r1#7); failure of the retry shows the reason again and keeps the audio. | Must | 1 |
| FR-12 | Before sending a recording to any engine, the system shall run voice-activity detection; if no speech is detected, then the system shall send nothing, paste nothing and notify "no speech detected" (r1#2). VAD uses the Silero model bundled in the installer for all engines (r2#3). | Given 3 s of silence, then no request is made, nothing is pasted, the clipboard is unchanged; Given a 1 s cough or keyboard noise only, the same; Given 3 s of speech, then the recording is sent; failure: if the VAD model cannot be loaded, then the system shall fall back to an energy threshold and log a warning (r2#3). | Must | 1 |
| FR-13 | The system shall provide a settings window with: engine (none / API / built-in local / local server), base URL, model, API key, local model selection, download and deletion, speech language (auto-detect or fixed), microphone, hotkey and mode, auto-paste on/off, LLM post-processing (on/off, endpoint, model, key, prompt), history on/off and size (1–100), start with Windows, UI language. The local-server engine has its own base URL, optional model and optional key (v4). | Given the user changes any setting and saves, then it takes effect without restarting the app and persists across restarts; failure: if a required field is invalid (empty base URL, malformed URL), then save is refused with the field highlighted. | Must | 1 |
| FR-14 | When the user clicks "Test connection" for the API or local-server engine, the system shall send a short sample to its transcription endpoint (using the values in the form, without saving) and report success or the error; the post-processing endpoint has no test (v4). | Given a valid key, then "OK" with the latency; failure: wrong base URL → "cannot reach <host>"; wrong key → "invalid API key". | Should | 1 |
| FR-15 | The system shall provide all user-visible text (windows, tray, overlay, notifications, error messages) in English and Russian, selectable in settings, defaulting to the OS language (r1#29). Text the user writes or names from the system (the prompt, including the built-in starter prompt, model and device names) is shown as is (v4). | Given Windows display language Russian, then the UI and notifications are Russian on first start; failure: for an unsupported OS language, the UI falls back to English. | Must | 1 |
| FR-16 | When history is enabled (default on, N = 20), the system shall keep the last N final transcripts (text, time, engine) locally, unencrypted, let the user copy any of them again, and clear them (r1#16). | Given N = 20 and 25 transcripts, then the 20 latest are shown; when "Clear" is clicked, then the list is empty and the stored data is deleted; failure: when history is turned off, then stored entries are deleted and nothing more is written to disk. | Should | 1 |
| FR-17 | When the engine is "local server", the system shall use the same OpenAI-compatible transcription client as FR-06 with a user-given URL and an optional key; the URL is a base URL or the full `…/audio/transcriptions` endpoint, as in FR-06 (#96). | Given a speaches/faster-whisper server at http://localhost:8000/v1, then transcription works without a key; Given URL = http://10.10.10.110:8000/v1/audio/transcriptions, it works the same way; failure: if the server is not running, then notify "cannot reach localhost:8000" and keep the audio (FR-11). | Must | 1 |
| FR-18 | The system shall report its version and commit in the About dialog and in the log line written at start (FR-20). | Given a build from commit abc123, About shows `<version> (abc123)` and the log's first line of the session contains `abc123`. No failure branch: static information (r1#22). | Must | 1 |
| FR-19 | When "start with Windows" is on (default off), the system shall start minimized to tray at user logon. | Given the option on and a reboot, the tray icon is present after logon; when off, it is not. | Should | 1 |
| FR-20 | The system shall write a rotating local log in `%LOCALAPPDATA%\Voicen\logs` (kept 7 days, ≤ 10 MB total) with version, commit, engine, timings (hotkey press → first audio frame, r2#11; stop → text → paste) and errors — never transcript text, audio or API keys. Crash records: an unhandled panic or a native fault (unhandled-exception filter) writes a crash file; a session marker written at start and removed on clean exit, if found at the next start, writes a crash record "previous session ended abnormally" (r2#4). Crash files are excluded from log rotation and kept 30 days, at most 20 files (r2#1). | Given one dictation, then the log has a line with the engine and the three timings and no transcript text; Given a forced panic in a test build, then a crash file appears and the next start logs "previous session crashed"; Given the process is killed, then the next start writes a crash record; Given a crash file 20 days old, then it is still present after rotation; failure: if the log directory is not writable, then the app keeps working and shows a one-time notification. | Must | 1 |
| FR-21 | When the app starts for the first time, the system shall open settings on the Engine tab with engine = none and apply the defaults: hotkey Ctrl+Alt+Space, mode hold, auto-paste on, speech language auto, history on with N = 20, start with Windows off, UI language from the OS (r1#4); microphone = Windows default input; API engine base URL `https://api.openai.com/v1` and model `whisper-1`; local server `http://localhost:8000/v1`; post-processing off with a built-in editable starter prompt (v4). | Given a fresh install, then settings open with these values; failure: if the hotkey is pressed while engine = none, then the system shall not record, shall notify "choose a transcription engine" and open settings. | Must | 1 |
| FR-22 | When the user presses Esc while recording, the system shall stop recording and discard it; Esc is intercepted only while recording (r1#18). | Given recording is on, when Esc is pressed, then nothing is sent or pasted and the indicator goes off; Given no recording, Esc reaches the focused application as usual. | Must | 1 |
| FR-23 | When the hotkey is pressed while a previous result is still being transcribed or post-processed, the system shall start a new recording; results shall be delivered in recording order, each to its own start window per FR-10 (r1#5). | Given dictation A is processing and the user dictates B in another window, then A is delivered to A's window and B to B's window, A first; failure: if A fails, B is still delivered and A follows FR-11. | Must | 1 |
| FR-24 | The system shall apply timeouts: connect 5 s; transcription via API 30 s; via local server 60 s; post-processing 15 s (r1#6); built-in local engine 120 s per transcription (v4). | Given the API does not answer in 30 s, then FR-11 applies with reason "timeout"; Given the LLM does not answer in 15 s, then FR-09's failure branch applies. | Must | 1 |
| FR-25 | Every failure notification shall also be shown for 3 s in the overlay, and the tray icon shall switch to an error state until the next successful dictation or until the user opens the tray menu (r1#8); a hotkey-registration error (FR-05) persists until a hotkey is registered (r2#9). | Given Focus Assist is on and the API key is invalid, then the overlay shows "invalid API key" for 3 s and the tray icon shows the error state. | Must | 1 |
| FR-26 | The hotkey and recording shall keep working after sleep/resume, lock/unlock and an Explorer restart without restarting the app (r1#9). | Given the PC sleeps and resumes, when the hotkey is pressed, then recording starts; failure: if re-registering the hotkey fails, then FR-05's startup branch applies. Checked manually by the owner. | Must | 1 |
| FR-27 | If the selected microphone is missing, then the system shall use the Windows default input device and notify once per device change, until the selected microphone returns (r2#12); if the device disappears mid-recording, then the system shall stop and process what was captured (r1#10). | Given the selected USB mic is unplugged, when the hotkey is pressed, then recording uses the default device and a one-time notification names it; Given the mic is unplugged at second 5 of a recording, then 5 s are processed; failure: if no input device exists at all, then FR-04's failure branch applies. | Must | 1 |
| FR-28 | The system shall let the user delete a downloaded local model, and the uninstaller shall ask "remove settings, history, models, logs and saved keys?" (default yes); all app data lives in `%LOCALAPPDATA%\Voicen` (r1#15), including the WebView2 data folder (v4). Uninstall always removes the start-with-Windows entry; an upgrade asks nothing and removes nothing (v4). | Given `small` is downloaded, when the user deletes it, then the file is gone and the space is freed; if it was selected, the engine becomes "none"; Given uninstall with "yes", then the folder and the Credential Manager entries are removed; with "no", they stay. | Should | 1 |
| FR-29 | When the user saves a base URL using `http://` on a non-local host (local = loopback only: `localhost` in any case, `127.0.0.0/8`, `::1`; v4), the system shall show a warning that the API key travels unencrypted, and still allow saving (r1#27). | Given `http://example.com/v1`, then a warning shows; Given `http://localhost:8000/v1` or `http://127.0.0.1`, then no warning. | Could | 1 |

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

## 6. Constraints

- **Platform:** Windows 10/11 x64 only in release 1 (i1).
- **Distribution:** public open-source on GitHub under MIT (r1#20); installer in GitHub Releases; no code signing in release 1 (decisions #83).
- **Development host:** Linux (this machine). Windows-specific behaviour (global hotkey, input simulation, clipboard, tray) is built and tested on the GitHub Actions Windows runner; manual verification on a real Windows machine by the owner.
- **Install policy (i2):** packages, toolchains, Docker images and MCP servers may be installed on the dev host and in CI, each only with the owner's explicit consent. CI: GitHub Actions, including the Windows runner.
- **Host now has:** Node 20, pnpm, Python 3.12, uv, Docker, make, gcc, gh. No Rust, Go, .NET, CMake, Wine.
- **Costs:** the user brings their own API keys; the project pays for nothing but free CI minutes.
- **Team:** the owner + AI agents; manual Windows checks by the owner on their PC (r1#26).
- **Deadline:** when ready (decisions #82). If the date turns out tight, cut first: FR-16, FR-14, FR-19, NFR-10, then the Russian UI of FR-15 (roast round 1, scope).

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

## 9. Technical decisions

<!-- Proposed by the requirements-reviewer during the roast, confirmed by the owner. -->

| Decision | Choice | Driven by | Alternative |
|---|---|---|---|
| Stack and areas | **Tauri 2**, areas `core` (Rust: `windows` crate for hotkey, SendInput, clipboard, Credential Manager, autostart; `cpal` capture; `whisper-rs` in-process whisper.cpp with VAD; `reqwest` OpenAI-compatible client) and `ui` (TypeScript + Svelte via Vite, pnpm); NSIS per-user installer via the Tauri bundler | NFR-09, NFR-03, FR-07, FR-12, NFR-04, NFR-11, FR-13, FR-15, §6 | .NET 8 WPF + Whisper.net (one area; UI cannot run on the Linux host, no UI verification adapter); Electron rejected (NFR-09 size, NFR-03 RAM) |
| What "deployed" means | Per task: the `windows-latest` CI job builds the commit's NSIS installer, installs it silently per-user, launches the app and checks that the start log line carries the commit (FR-18, FR-20). Per release: a `v*` tag publishes that installer to GitHub Releases | §2 success 3, NFR-09, FR-18 | A portable zip per commit, installer only at release time |
| Where verification runs | Linux host: `core` platform-independent logic (engine clients against a mock OpenAI-compatible server, model download and checksum, retry and pending state, VAD gate, post-processing fallback, settings, history, ordering) with Windows code behind `cfg` and trait fakes; `ui` with vitest and Playwright. GitHub Actions `windows-latest`: full `cargo test` incl. Windows APIs (clipboard, Credential Manager, hotkey conflict, SendInput into a test window), whisper.cpp transcribing a bundled WAV with a cached `tiny` model, installer smoke test; recordings come from an injectable WAV audio source. Owner's Windows PC: real microphone and hotkeys, NFR-08 checklist, NFR-01 benchmark, clean-Win11 install in Windows Sandbox | §6, NFR-01, NFR-02, NFR-08, FR-02–FR-12, FR-26 | A self-hosted runner on the owner's Windows 11 PC (real mic in CI; needs the PC online) |
| Installs needing consent | Host: Rust via a `rust` Docker image (no host install), with cmake and clang in that image; `@playwright/test` with its Chromium. CI: the Rust toolchain action on `windows-latest` (MSVC and CMake preinstalled). Each install is still asked before it runs (r2#13) | §6 install policy, Stack row | rustup + cmake/clang installed on the host |
| UI verification | **playwright** for the web-UI screens (settings, first run, history, overlay) in Chromium against a mocked Tauri IPC layer, on the Linux host and in CI. Tray, global hotkey, toasts and auto-paste have no kit adapter: tasks touching them carry a reviewed `verify_exception` with the owner's manual check on Windows, plus Windows-runner integration tests as evidence | FR-13, FR-15, FR-16, FR-21, FR-04, FR-10, §6 | `none` with a recorded deferral (all UI checks manual); tauri-driver on the Windows runner (no kit adapter) |

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
