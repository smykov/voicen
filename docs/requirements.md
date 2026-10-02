# Requirements

<!-- The product's requirements document: what must be true, stated so it can be tested.
     Built and roasted by /teamwright:requirements; the spec tool and the tasks are derived from it.
     Write only what the owner said or approved. A guess is marked "working assumption" in section 10;
     an unknown goes to docs/open-questions.md. IDs (FR-NN, NFR-NN) are never reused. -->

Version: 1 · Status: draft · Approved by the owner: — · Roast: — (`docs/requirements.reviews/`)
Source: owner's brief in the /teamwright:init call (2026-10-02, Russian), interview rounds 1–3 (2026-10-02)

Pointers: `(brief)` — the owner's brief; `(iN)` — interview round N.

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

**Success (both criteria, i3):**
1. **Speed:** for a phrase up to 15 s, the text is pasted ≤ 3 s after the user stops recording via the API engine, and ≤ 10 s via the built-in local engine on the `small` model (reference machine: see OQ-03). Without LLM post-processing.
2. **Daily use:** the owner uses it daily for 2 weeks without a crash.
3. **Public release:** a Windows installer published in GitHub Releases installs and completes the flow on a clean Windows 11.

Target date: see OQ-01.

## 3. Scope

| Activity | Steps | Release 1 | Priority |
|---|---|---|---|
| Install and start | install from GitHub Releases; run in background (tray); optional start with Windows | yes | Must (start with Windows: Should) |
| Configure | choose engine and its settings; enter API key; download a local model; choose hotkey and mode; choose speech language; choose microphone; UI language | yes | Must |
| Record | press hotkey; hold (push-to-talk) or toggle; see that recording is on; cancel | yes | Must |
| Transcribe | send to OpenAI-compatible API; built-in whisper.cpp; local OpenAI-compatible server | yes | Must |
| Post-process | optional LLM step over an OpenAI-compatible chat API with an editable prompt | yes | Must (owner asked for it in release 1, i3) |
| Deliver text | copy to clipboard; auto-paste with Ctrl+V into the focused field; auto-paste can be turned off | yes | Must |
| Handle failures | notify with the reason; keep the audio; retry | yes | Must |
| Review history | list of the last N transcripts (text only), copy again, disable, clear | yes | Should |
| Other providers | Deepgram, ElevenLabs Scribe, Yandex SpeechKit native APIs | no | Won't (release 1) |
| Other platforms | macOS, Linux | no | Won't (release 1) |
| Automatic fallback | switching to a reserve engine on failure | no | Could (later) |
| GPU acceleration | CUDA / Vulkan for local engine | no | Could (later) |

**Not doing (no-gos) in release 1:**
- macOS and Linux builds — Windows only (i1).
- Provider-specific APIs other than OpenAI-compatible (i2): the OpenAI-compatible client covers OpenAI, Groq and compatible local servers.
- Storing audio in history (i3): history keeps text only.
- Automatic fallback to another engine (i3: notification + retry was chosen, not fallback).
- Code signing, auto-update, licensing (i1: public open-source, not a commercial product). See OQ-02 for SmartScreen.
- Real-time streaming transcription while speaking.

## 4. Functional requirements

| ID | Requirement (EARS) | Acceptance: examples, including a failing one | Priority | Release |
|---|---|---|---|---|
| FR-01 | When the app starts, the system shall run in the background with a tray icon giving access to settings, history and exit. | Given the app is launched, then a tray icon appears and no main window blocks the user; failure: if a second instance is launched, then the system shall not start a second instance and shall bring the running one's settings to front. | Must | 1 |
| FR-02 | When the user presses the configured global hotkey in push-to-talk mode, the system shall record from the selected microphone while the hotkey is held and stop when it is released. | Given mode = hold, hotkey = Ctrl+Alt+Space, when the user holds it 4 s in Notepad and releases, then ~4 s of audio is captured and processing starts; failure: if the hotkey is held < 0.3 s (working assumption), then the system shall discard the recording without sending it. | Must | 1 |
| FR-03 | When the user presses the configured global hotkey in toggle mode, the system shall start recording, and stop when the hotkey is pressed again. | Given mode = toggle, press → speak 10 s → press, then 10 s of audio is processed; failure: if the recording reaches the maximum length (OQ-04), then the system shall stop it and process what was recorded, notifying the user. | Must | 1 |
| FR-04 | While recording, the system shall show a visible indicator (tray icon state and a small overlay) and allow cancelling with Esc. | Given recording is on, then the indicator is visible; when Esc is pressed, then recording stops and nothing is sent or pasted; failure: if the microphone is unavailable or access is denied, then the system shall not show a recording state and shall notify "microphone unavailable" with the reason. | Must | 1 |
| FR-05 | When the configured hotkey is already taken by another application or the OS, the system shall report the conflict in settings and keep the previous hotkey. | Given the user picks a hotkey that cannot be registered, then settings show "hotkey unavailable" and the old one stays active; failure: the app never runs without a working hotkey silently. | Must | 1 |
| FR-06 | When recording stops and the engine is "OpenAI-compatible API", the system shall send the audio to `<base URL>/audio/transcriptions` with the configured model, API key and language, and receive the text. | Given base URL = https://api.openai.com/v1, model = whisper-1, valid key, then the transcript text is returned; Given base URL of Groq or a local server, it works the same way; failure: if the API returns 401/403, then the system shall notify "invalid API key" and paste nothing. | Must | 1 |
| FR-07 | When recording stops and the engine is "built-in local", the system shall transcribe the audio on the user's machine with whisper.cpp and the selected downloaded model, without network access. | Given model `small` is downloaded and the network is off, then the transcript is produced and pasted; failure: if no model is downloaded, then the system shall notify "no local model" with a link to the settings, and paste nothing. | Must | 1 |
| FR-08 | When the user selects a local model size in settings, the system shall download it with progress, verify its checksum and make it available offline. | Given the user picks `base`, then a progress bar shows and the model becomes selectable when done; failure: if the download is interrupted or the checksum does not match, then the system shall delete the partial file and offer to retry. | Must | 1 |
| FR-09 | When LLM post-processing is enabled, the system shall send the transcript and the user's editable prompt to the configured OpenAI-compatible chat endpoint and use the returned text instead of the raw transcript. | Given post-processing on with prompt "fix punctuation", raw "привет как дела" → pasted "Привет, как дела?"; failure: if the LLM call fails or times out, then the system shall paste the raw transcript and notify that post-processing was skipped. | Must | 1 |
| FR-10 | When the final text is ready, the system shall put it in the clipboard and, if auto-paste is on, send Ctrl+V to the window that had focus when recording started. | Given Notepad was focused at start and auto-paste is on, then the text appears in Notepad and is also in the clipboard; Given auto-paste is off, then the text is only in the clipboard and a notification says so; failure: if the focused window changed or cannot receive input (e.g. an elevated window), then the system shall leave the text in the clipboard and notify "copied — paste manually". | Must | 1 |
| FR-11 | If transcription fails (no network, timeout, HTTP error, engine error), then the system shall paste nothing, notify the user with the reason, and keep the audio of the last failed recording so the user can retry from the notification or the tray. | Given network off and the API engine, then a notification "network unavailable — Retry" appears; when the network returns and the user clicks Retry, then the text is pasted/copied; failure of the retry shows the reason again and keeps the audio. | Must | 1 |
| FR-12 | If the transcript is empty (silence), then the system shall paste nothing and notify "no speech detected". | Given 3 s of silence, then nothing is pasted, clipboard is unchanged. | Must | 1 |
| FR-13 | The system shall provide a settings window with: engine (API / built-in local / local server), base URL, model, API key, local model selection and download, speech language (auto-detect or fixed), microphone, hotkey and mode, auto-paste on/off, LLM post-processing (on/off, endpoint, model, key, prompt), history on/off and size, start with Windows, UI language. | Given the user changes any setting and saves, then it takes effect without restarting the app and persists across restarts; failure: if a required field is invalid (empty base URL, malformed URL), then save is refused with the field highlighted. | Must | 1 |
| FR-14 | When the user clicks "Test connection" for an API engine, the system shall send a short sample to the endpoint and report success or the error. | Given a valid key, then "OK" with the latency; failure: wrong base URL → "cannot reach <host>"; wrong key → "invalid API key". | Should | 1 |
| FR-15 | The system shall provide its UI in English and Russian, selectable in settings, defaulting to the OS language. | Given Windows display language Russian, then the UI is Russian on first start; failure: for an unsupported OS language, the UI falls back to English. | Must | 1 |
| FR-16 | When history is enabled, the system shall keep the last N transcripts (text, time, engine), let the user copy any of them again, and clear them. | Given N = 20 and 25 transcripts, then the 20 latest are shown; when "Clear" is clicked, then the list is empty and the stored data is deleted; failure: when history is disabled, then nothing is written to disk. | Should | 1 |
| FR-17 | When the engine is "local server", the system shall use the same OpenAI-compatible transcription client as FR-06 with a user-given URL and an optional key. | Given a faster-whisper/speaches server at http://localhost:8000/v1, then transcription works without a key; failure: if the server is not running, then notify "cannot reach localhost:8000" and keep the audio (FR-11). | Must | 1 |
| FR-18 | The system shall report its version and commit (About dialog and log line at start). | Given build from commit abc123, About shows `<version> (abc123)`. | Must | 1 |
| FR-19 | When "start with Windows" is on, the system shall start minimized to tray at user logon. | Given the option on and a reboot, the tray icon is present after logon; when off, it is not. | Should | 1 |

## 5. Non-functional requirements

| ID | Characteristic | Requirement (measurable) | How it is checked | Priority | Release |
|---|---|---|---|---|---|
| NFR-01 | Performance | Phrase ≤ 15 s: text pasted ≤ 3 s after stop via OpenAI-compatible API (excluding provider outage), ≤ 10 s via built-in local `small` model on the reference machine (OQ-03); LLM post-processing excluded. | Timing log of stop→paste; manual benchmark on the reference machine with a fixed sample set. | Must | 1 |
| NFR-02 | Performance | Hotkey press → recording starts ≤ 200 ms (no lost first word). | Automated test measuring capture start; manual check. | Must | 1 |
| NFR-03 | Resource use | Idle in tray: CPU < 1 %, RAM ≤ 150 MB without a loaded local model. | Measured on the reference machine. | Should | 1 |
| NFR-04 | Security | API keys are stored in Windows Credential Manager (or DPAPI-encrypted), never in plain-text config, logs or crash reports. | Unit test on storage; log redaction test; secret scanner in CI. | Must | 1 |
| NFR-05 | Privacy | With the built-in local engine and post-processing off, no audio or text leaves the machine (no network calls except model download and an explicit "Test connection"). No telemetry. | Test with network mocked/blocked; code review of network calls. | Must | 1 |
| NFR-06 | Privacy | Audio is kept only in memory or a temp file until successful transcription or the next recording; deleted on exit. History stores text only, and only when enabled. | Integration test on temp dir; manual check. | Must | 1 |
| NFR-07 | Reliability | No crash in 2 weeks of daily use by the owner; a failure in any engine never crashes the app. | Owner's 2-week usage log; crash log empty. | Must | 1 |
| NFR-08 | Compatibility | Windows 10 22H2 and Windows 11, x64. Auto-paste works in common targets: Notepad, browsers (Chrome/Edge text fields), VS Code, Telegram Desktop, Microsoft Word. | Manual checklist per release; automated paste test on CI Windows runner where possible. | Must | 1 |
| NFR-09 | Portability / install | Installer (.exe or .msi) from GitHub Releases installs per-user without admin rights; installed size ≤ 100 MB excluding local models. | CI build artifact; install on clean Windows 11 VM. | Must | 1 |
| NFR-10 | Usability | First-run: from installer to first pasted transcript ≤ 3 minutes with an API key at hand. | Manual run-through. | Should | 1 |
| NFR-11 | Maintainability | Engines are behind one interface; adding a new provider does not change recording or delivery code. | Architecture review; tests per engine with a fake. | Should | 1 |
| NFR-12 | Legal | The project's license and every bundled component's license (whisper.cpp MIT, models' licenses) are compatible and listed. | License check in CI. | Must | 1 |

## 6. Constraints

- **Platform:** Windows 10/11 x64 only in release 1 (i1).
- **Distribution:** public open-source on GitHub; installer in GitHub Releases; no code signing in release 1 (working assumption, OQ-02).
- **Development host:** Linux (this machine). Windows-specific behaviour (global hotkey, input simulation, clipboard, tray) is built and tested on the GitHub Actions Windows runner; manual verification on a real Windows machine by the owner.
- **Install policy (i2):** packages, toolchains, Docker images and MCP servers may be installed on the dev host and in CI, each only with the owner's explicit consent. CI: GitHub Actions, including the Windows runner.
- **Host now has:** Node 20, pnpm, Python 3.12, uv, Docker, make, gcc, gh. No Rust, Go, .NET, CMake, Wine.
- **Costs:** the user brings their own API keys; the project pays for nothing but free CI minutes.
- **Deadline / budget / team:** OQ-01.

## 7. Glossary and data

| Term | Meaning |
|---|---|
| Recording | Audio captured from the selected microphone between hotkey start and stop. |
| Engine | The transcription backend: OpenAI-compatible API, built-in local (whisper.cpp), or local server (OpenAI-compatible URL on the user's machine). |
| OpenAI-compatible API | An HTTP API implementing `POST /v1/audio/transcriptions` (and `/v1/chat/completions` for post-processing) as OpenAI does: OpenAI, Groq, speaches, etc. |
| Local model | A whisper.cpp ggml model file (tiny/base/small/medium/large-v3[-turbo]) downloaded to the user's machine. |
| Post-processing | Optional LLM step that rewrites the transcript using the user's prompt. |
| Push-to-talk (hold) | Recording while the hotkey is held. |
| Toggle | First press starts recording, second press stops. |
| Auto-paste | Simulated Ctrl+V into the window that was focused at recording start. |
| Transcript | Text returned by the engine (raw), or after post-processing (final). |
| History entry | Final text, timestamp, engine name; no audio. |

**Main entities:** `Settings` (engine, base URL, model, language, mic, hotkey, mode, auto-paste, post-processing config, history config, UI language, autostart); `Secret` (API keys, in the OS credential store); `LocalModel` (name, size, path, checksum, state); `HistoryEntry`; `PendingRecording` (audio of the last failed attempt).

## 8. Integrations and external dependencies

| Dependency | Use | Account / cost | Notes |
|---|---|---|---|
| OpenAI-compatible transcription API (OpenAI, Groq, …) | FR-06 | user's own key, paid per use | Rate limits per provider; max upload size (OpenAI: 25 MB). |
| OpenAI-compatible chat API | FR-09 | user's own key | Can be the same or a different provider than transcription. |
| Local OpenAI-compatible server (speaches, faster-whisper-server, …) | FR-17 | none | Run by the user. |
| whisper.cpp | FR-07 | none, MIT | Bundled library/binary. |
| Whisper ggml models (Hugging Face `ggerganov/whisper.cpp`) | FR-08 | none | Downloaded on demand; 75 MB – 3 GB. |
| Windows APIs | hotkey, input simulation (SendInput), clipboard, Credential Manager, autostart | none | |
| GitHub (repo, Actions, Releases) | CI, distribution | free for public repos | |

## 9. Technical decisions

<!-- Proposed by the requirements-reviewer during the roast, confirmed by the owner. -->

| Decision | Choice | Driven by | Alternative |
|---|---|---|---|
| Stack and areas | — | — | — |
| What "deployed" means | — | — | — |
| Where verification runs | — | — | — |
| UI verification | — | — | — |

## 10. Assumptions, risks, open questions

| Assumption or risk | Status | What it affects |
|---|---|---|
| Users bring their own API keys; no hosted backend of ours. | confirmed (i2) | FR-06, FR-09, NFR-05 |
| Minimum hold of 0.3 s to count as a recording. | working assumption | FR-02 |
| API keys in Windows Credential Manager. | working assumption | NFR-04 |
| No code signing; SmartScreen warning accepted in release 1. | working assumption → OQ-02 | NFR-09 |
| Auto-paste into elevated (admin) windows is impossible from a non-elevated app; text stays in clipboard. | working assumption | FR-10 |
| Pre-mortem: auto-paste unreliable in some apps (Electron, games, RDP) → users lose trust. | risk | FR-10, NFR-08 |
| Pre-mortem: local engine too slow on CPU for `medium`/`large` → users think the app is broken. | risk; mitigated by showing a recommended model and progress | FR-07, NFR-01 |
| Pre-mortem: global hotkey collides with other apps or is swallowed by antivirus/keyboard hooks. | risk | FR-05 |
| Pre-mortem: Windows behaviour cannot be tested on the Linux dev host → regressions reach users. | risk; mitigated by Windows CI runner and owner's manual check | NFR-08, section 9 |
| Pre-mortem: unsigned installer flagged by SmartScreen/antivirus (keyboard hook + input simulation looks like a keylogger). | risk | NFR-09, OQ-02 |

Open questions: OQ-01, OQ-02, OQ-03, OQ-04 (`docs/open-questions.md`).
