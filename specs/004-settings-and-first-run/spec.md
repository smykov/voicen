# Feature Specification: Settings and First Run

**Feature Branch**: `004-settings-and-first-run`

**Created**: 2026-10-02

**Status**: Approved (clarifications confirmed by the owner 2026-10-02)

**Input**: User description: "Settings and first run: a settings window with every option (engine none/API/built-in local/local server, base URL, model, API key, local model selection, speech language, microphone, hotkey and mode, auto-paste, LLM post-processing fields, history on/off and size, start with Windows, UI language) that applies without restart and persists; field validation; Test connection; API keys stored only in Windows Credential Manager; the first launch opens settings with engine none and the documented defaults; an http:// warning for non-local hosts; English and Russian UI defaulting to the OS language; start with Windows."

**Requirement ids in scope** (`docs/requirements.md` v3): FR-13, FR-14, FR-15, FR-19, FR-21, FR-29; NFR-04, NFR-10.

**Boundaries with other features**: this feature owns the settings window, the settings model (fields, defaults, validation, persistence, live apply), key storage, first run, the UI-language mechanism and the message catalog, start with Windows, Test connection and the http warning. It hosts but does not own: hotkey registration and its conflict handling (001, req FR-05), the transcription client used by Test connection (001, req FR-06/FR-17), local model download/deletion controls (002, req FR-08/FR-28), post-processing behaviour and its default prompt (003, req FR-09), history behaviour when it is turned off or shrunk (005, req FR-16). The texts those features show are localized through this feature's catalog (req FR-15).

## Clarifications

### Session 2026-10-02

- Q: When a save includes a hotkey that cannot be registered, is the rest of the save applied? → A: No. A save is all-or-nothing: the whole save is refused, the hotkey field is highlighted with "hotkey unavailable", the previous hotkey stays active and nothing else changes (confirmed by the owner 2026-10-02)
- Q: Which fields are required for each engine? → A: API engine: base URL, model and API key. Local server: base URL only; model and key optional (aligned with 002, whose local-server request omits the model when the field is empty). Built-in local: a downloaded model selected. Engine none: nothing. Fields of engines that are not selected are kept but not validated (confirmed by the owner 2026-10-02)
- Q: What does "Test connection" test, and with which values? → A: The transcription endpoint of the selected API or local-server engine, using the values currently in the form (unsaved ones included, the stored key when the key field is untouched), by sending a bundled ~1 s speech clip; it never saves anything. The post-processing endpoint is not tested in release 1 (confirmed by the owner 2026-10-02)
- Q: What happens when the settings file exists but cannot be read (corrupt, unknown format)? → A: The unreadable file is kept as a timestamped backup next to it, the app starts with the defaults as on a first run (settings open on the Engine tab, engine = none), notifies "settings could not be read and were reset", and leaves saved keys in the credential store untouched (confirmed by the owner 2026-10-02)
- Q: Are the API engine's base URL and model pre-filled on first run? → A: Yes: base URL `https://api.openai.com/v1` and model `whisper-1` for the API engine, base URL `http://localhost:8000/v1` for the local server; so a user with an OpenAI key only pastes the key (supports NFR-10) (confirmed by the owner 2026-10-02)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - First run: choose an engine and dictate (Priority: P1)

A new user installs Voicen and launches it. Instead of a silent tray icon, the settings window opens on the Engine tab with engine = none and all other settings at their documented defaults. The user picks "OpenAI-compatible API", pastes their key (base URL and model are pre-filled), saves, and can dictate immediately with the default hotkey Ctrl+Alt+Space (hold).

**Why this priority**: without it a new user cannot dictate at all; it is the entry point of release 1's one flow and of the NFR-10 three-minute goal.

**Independent Test**: start the app with an empty data folder → the settings window shows the Engine tab with engine = none and the defaults; pick API, enter a key, save → the saved settings file holds the choice and no key; the credential store holds the key; pressing the hotkey records.

**Acceptance Scenarios**:

1. **Given** a fresh install (no settings file), **When** the app starts, **Then** the settings window opens on the Engine tab with engine = none, hotkey Ctrl+Alt+Space, mode hold, auto-paste on, speech language auto, history on with N = 20, start with Windows off, microphone = Windows default, and UI language derived from the OS language (req FR-21).
2. **Given** the first-run window, **When** the user selects "OpenAI-compatible API", **Then** base URL `https://api.openai.com/v1` and model `whisper-1` are already filled and only the API key is empty.
3. **Given** engine = API with a key entered, **When** the user saves, **Then** the save succeeds, the key is in the credential store, the settings file contains no key, and the next hotkey press records with the API engine without restarting the app (req FR-13, NFR-04).
4. **Given** engine = none (the user closed the first-run window without choosing), **When** the hotkey is pressed, **Then** nothing is recorded (the microphone is not opened), the notification "choose a transcription engine" is shown, and the settings window opens on the Engine tab (req FR-21 failure branch).
5. **Given** the first run happened and settings exist, **When** the app starts again, **Then** no settings window opens (tray only) (req FR-21, FR-01).
6. **Given** a fresh install on a PC with an OpenAI key at hand, **When** the user goes from launching the installer to the first pasted transcript, **Then** it takes ≤ 3 minutes (req NFR-10, owner's manual check).

---

### User Story 2 - Change any setting, validated, applied without restart, persisted (Priority: P1)

A user opens settings from the tray, changes any option on any tab, and saves. Invalid input is refused with the offending field highlighted and a reason; valid changes take effect immediately (next dictation, hotkey re-registered, tray and windows re-labelled) and survive a restart.

**Why this priority**: FR-13 is Must; every other feature's options are reached through this window.

**Independent Test**: for every field, change it, save, observe the effect on the next use without restart, restart, observe the value persisted; for every validation rule, enter an invalid value and observe the refusal and highlight.

**Acceptance Scenarios**:

1. **Given** the settings window, **When** the user changes any field and saves, **Then** the change takes effect without restarting the app (see FR-008 for what "takes effect" means per field) and persists across restarts (req FR-13).
2. **Given** engine = API, **When** the base URL is empty or malformed (e.g. `api.openai.com`, `htp://x`, `https://`), **Then** save is refused, the base URL field is highlighted with the reason, and the previously saved settings stay in force (req FR-13 failure branch).
3. **Given** engine = API and no key stored or entered, **When** the user saves, **Then** save is refused with the API key field highlighted.
4. **Given** a new hotkey that another application holds, **When** the user saves, **Then** the whole save is refused, the hotkey field shows "hotkey unavailable", and the previous hotkey keeps working (req FR-05 via FR-13; Clarification Q1).
5. **Given** a stored API key, **When** the settings window opens, **Then** the key field shows that a key is saved but never shows the key itself; **When** the user leaves it untouched and saves, **Then** the stored key is kept; **When** the user clears it and saves (with an engine that does not require it), **Then** the stored key is deleted (req NFR-04).
6. **Given** the credential store refuses a write, **When** the user saves a new key, **Then** save is refused with the key field highlighted and "cannot store the key in Windows Credential Manager", and the key is not written anywhere else (req NFR-04).
7. **Given** unsaved changes, **When** the user closes the window, **Then** the user is asked whether to discard them; choosing discard keeps the saved settings unchanged.

---

### User Story 3 - English and Russian interface (Priority: P2)

A Russian-speaking user on a Russian Windows sees the whole app in Russian from the first start; any user can switch between English and Russian in settings and the change applies at once to the open windows, the tray menu, the overlay and every later notification.

**Why this priority**: Must (FR-15), but the first slice of the product works in English; listed in requirements §6 as the last thing cut if the date is tight.

**Independent Test**: start with OS language Russian → the settings window is Russian; switch to English and save → the window and tray menu become English without restart; start with an unsupported OS language → English.

**Acceptance Scenarios**:

1. **Given** Windows display language Russian and a fresh install, **When** the app starts, **Then** the settings window, tray menu, overlay and notifications are in Russian (req FR-15, FR-21).
2. **Given** Windows display language English (any region), **When** the app starts the first time, **Then** the UI is English.
3. **Given** an unsupported Windows display language (e.g. German, Ukrainian), **When** the app starts the first time, **Then** the UI falls back to English (req FR-15 failure branch).
4. **Given** UI language English, **When** the user selects Russian and saves, **Then** the open settings window, the tray menu and any notification shown afterwards are Russian, without restart.
5. **Given** the UI language was chosen once, **When** the Windows display language later changes, **Then** the app keeps the language chosen in settings.

---

### User Story 4 - Test connection (Priority: P3)

Before saving, a user who entered a base URL, model and key clicks "Test connection" and sees "OK" with the latency, or the precise reason it failed.

**Why this priority**: Should (FR-14); it shortens setup but the product works without it.

**Independent Test**: against a mock OpenAI-compatible server: valid key → "OK, <n> ms"; wrong key → "invalid API key"; unreachable host → "cannot reach <host>".

**Acceptance Scenarios**:

1. **Given** engine = API with a valid base URL, model and key, **When** the user clicks "Test connection", **Then** "OK" with the measured latency in milliseconds is shown (req FR-14).
2. **Given** a base URL whose host cannot be reached, **When** the user tests, **Then** "cannot reach <host>" is shown (req FR-14 failure branch).
3. **Given** a wrong key (endpoint answers 401 or 403), **When** the user tests, **Then** "invalid API key" is shown (req FR-14 failure branch).
4. **Given** the endpoint does not answer within the timeouts of FR-24 (connect 5 s; 30 s API, 60 s local server), **When** the user tests, **Then** "timeout" is shown.
5. **Given** engine = local server at `http://localhost:8000/v1` without a key, **When** the user tests and the server runs, **Then** "OK" with latency is shown.
6. **Given** a test is in progress, **Then** the button is disabled and shows progress; the test never changes the saved settings or the stored key.

---

### User Story 5 - Start with Windows (Priority: P3)

A user turns "Start with Windows" on; after the next logon Voicen is in the tray without opening any window. Turning it off stops this.

**Why this priority**: Should (FR-19).

**Independent Test**: turn the option on and save → the logon start entry exists for the current user; turn it off and save → it is gone; reboot with it on → the tray icon is present after logon (owner's manual check).

**Acceptance Scenarios**:

1. **Given** "Start with Windows" on and saved, **When** the user logs on after a reboot, **Then** the tray icon is present and no window opens (req FR-19).
2. **Given** the option off and saved, **When** the user logs on, **Then** Voicen does not start (req FR-19).
3. **Given** the option is on and settings exist and are readable, **When** the app is started at logon, **Then** it starts in the tray without opening any window. The only cases that open the settings window at a logon start are the ones that open it at any start: no settings file (FR-005), an unreadable settings file (FR-010) and the startup hotkey-failure branch of req FR-05 (owned by 001).
4. **Given** the logon start entry cannot be written, **When** the user saves with the option on, **Then** save is refused with the option highlighted and the reason, and nothing else changes.

---

### User Story 6 - Warning for unencrypted remote endpoints (Priority: P4)

A user who enters an `http://` base URL on a non-local host is warned that the API key travels unencrypted; the save still goes through.

**Why this priority**: Could (FR-29).

**Independent Test**: enter `http://example.com/v1` → warning shown, save succeeds; enter `http://localhost:8000/v1` or `http://127.0.0.1` → no warning.

**Acceptance Scenarios**:

1. **Given** base URL `http://example.com/v1`, **When** the user saves, **Then** a warning that the API key travels unencrypted is shown and the settings are saved (req FR-29). The warning applies only to URLs in use: the selected engine's base URL and the post-processing endpoint while post-processing is on.
2. **Given** base URL `http://localhost:8000/v1`, `http://127.0.0.1:8000/v1` or `http://[::1]:8000/v1`, **When** the user saves, **Then** no warning is shown (req FR-29).
3. **Given** base URL `https://example.com/v1`, **Then** no warning is shown.
4. The same rule applies to the post-processing endpoint and to the local-server base URL; `http://LOCALHOST` is loopback too.

---

### Edge Cases

- **Settings file unreadable** (corrupt JSON, unknown schema version): backed up as `settings.json.bad-<timestamp>`, defaults applied as on a first run, settings open on the Engine tab, notification "settings could not be read and were reset"; stored keys untouched (Clarification Q4). If the file cannot be moved aside or read for an I/O reason: defaults in memory only, file never written or moved, saves refused with a notice until restart, credential store untouched (FR-010).
- **Settings file cannot be written** (disk full, permission): save is refused with "cannot save settings: <reason>", the in-memory settings, hotkey, autostart and keys stay as they were before the save.
- **Save interrupted** (crash or power loss during save): the previous settings file stays intact; a partially written file is never read as the settings (write to a temporary file, then replace).
- **Credential Manager unavailable or write refused**: save refused, key field highlighted, the key is never stored in the settings file or anywhere else (req NFR-04).
- **A stored key is missing at dictation time** (deleted outside the app): the engine reports "invalid API key"/missing key per 001; the settings window shows the key field as empty.
- **Hotkey taken by another application at save**: whole save refused, old hotkey stays active (req FR-05, Clarification Q1).
- **Hotkey without a modifier, or Esc as the key**: refused at validation — a hotkey needs at least one of Ctrl, Alt, Shift, Win plus one non-modifier key; Esc is reserved for cancel (req FR-22).
- **Selected microphone not connected when settings open**: it stays selected and is shown as "(not connected)"; runtime fallback is 001's (req FR-27).
- **Engine switched while a dictation is being processed**: the dictation in progress finishes with the settings it started with; the new settings apply from the next recording.
- **Settings changed while a Test connection is running**: the test result refers to the values at the moment of the click.
- **Settings window closed while a Test connection is running**: the test result is discarded; nothing is saved and no notification is shown.
- **Built-in local selected with no downloaded model**: save refused with the local model field highlighted ("download a model first") (Clarification Q2).
- **Base URL with a trailing slash or surrounding spaces**: trimmed before validation and storage (`https://api.openai.com/v1/` → `https://api.openai.com/v1`).
- **Base URL with a scheme other than http/https, or without a host**: refused as malformed.
- **History size outside 1–100 or not a whole number**: refused with the field highlighted.
- **Second "Settings" open request while the window is open** (tray click, FR-21 failure branch, FR-05): the existing window comes to front on the requested tab; no second window.
- **First run launched by the autostart entry**: cannot happen in practice (start with Windows is off by default); if no settings file exists the first-run window opens regardless of how the app was launched.
- **OS language cannot be read**: English is used (same as unsupported language).
- **A message key missing from the Russian catalog**: impossible by construction — the build/test fails when the catalogs differ (FR-012).
- **Two windows with unsaved edits**: only one settings window exists; the overlay is read-only.

## Requirements *(mandatory)*

### Functional Requirements

**Settings window and model**

- **FR-001** (req FR-13): The system MUST provide a single settings window with these settings, grouped into tabs Engine, Recording, Output, Post-processing, History, General:
  - Engine tab: engine (none / OpenAI-compatible API / built-in local / local server); for API and local server: base URL, model, API key; for built-in local: the local model selection (download and deletion controls hosted from 002); speech language (auto-detect or a fixed language); Test connection.
  - Recording tab: microphone (Windows default or a named input device), hotkey, mode (hold / toggle).
  - Output tab: auto-paste on/off.
  - Post-processing tab: on/off, endpoint (base URL), model, API key, prompt.
  - History tab: on/off, size N.
  - General tab: start with Windows, UI language (English / Russian).
- **FR-002** (req FR-13): The settings window MUST open from the tray (owned by 001), on first run (FR-005), when the hotkey is pressed with engine = none (FR-007), and on the FR-05 startup branch (owned by 001) on a requested tab/field, when a second instance is launched (001 FR-002), and from the "no local model" notification action (002 FR-011) on the Engine tab; a request while it is open brings the existing window to front on that tab.
- **FR-003** (req FR-13): Changes MUST be applied only by an explicit Save. A save is all-or-nothing: either every changed setting is validated, applied and persisted, or nothing changes and the reasons are shown on the fields concerned (Clarification Q1).
- **FR-004** (req FR-13): Save MUST be refused, with the offending fields highlighted and a reason per field, when:
  - engine = API and the base URL is empty, malformed (not an absolute `http`/`https` URL with a host), or carries userinfo (`user:pass@`, `url.credentials`), the model is empty, or no API key is stored or entered;
  - engine = local server and the base URL is empty, malformed or carries userinfo (`url.credentials`) (model and key are optional, 002 FR-017);
  - the speech language is neither auto nor a code of the supported list (`language.unsupported`, any engine);
  - engine = built-in local and no downloaded model is selected;
  - post-processing is on and its endpoint is empty or malformed, its model is empty, or its prompt is empty;
  - the hotkey has no modifier (Ctrl, Alt, Shift, Win), has no non-modifier key, or uses Esc;
  - the hotkey cannot be registered ("hotkey unavailable", req FR-05);
  - history size N is not a whole number from 1 to 100;
  - start with Windows cannot be applied, the key cannot be stored, or the settings cannot be written.
  Fields of engines (and of post-processing) that are not selected are kept as entered but not validated (Clarification Q2).
- **FR-005** (req FR-21): When the app starts and no settings file exists, the system MUST create the settings with these defaults, persist them, and open the settings window on the Engine tab: engine none; hotkey Ctrl+Alt+Space; mode hold; auto-paste on; speech language auto; history on, N = 20; start with Windows off; UI language from the OS (FR-011); microphone Windows default; post-processing off with empty endpoint, model and key and the prompt = 003's built-in starter prompt (003 FR-011); API engine base URL `https://api.openai.com/v1` and model `whisper-1`; local server base URL `http://localhost:8000/v1` with an empty model (Clarification Q5). Defaults are defined in exactly one place.
- **FR-006** (req FR-21, FR-01): When settings exist, the app MUST start without opening the settings window (tray only).
- **FR-007** (req FR-21 failure branch): While engine = none, a hotkey press MUST NOT open the microphone or record; the system MUST notify "choose a transcription engine" and open the settings window on the Engine tab.
- **FR-008** (req FR-13): After a successful save every setting MUST take effect without restarting the app:
  - engine, base URLs, models, keys, speech language, microphone, auto-paste, post-processing: from the next recording (a dictation already in progress keeps the settings it started with);
  - hotkey and mode: the new hotkey is active and the old one released before Save reports success;
  - history on/off and size: immediately (what happens to stored entries is 005's, req FR-16);
  - start with Windows: the logon entry is created or removed before Save reports success;
  - UI language: immediately in the open windows, the tray menu, the overlay and every later notification.
- **FR-009** (req FR-13): Saved settings MUST persist across app restarts and be the settings loaded at the next start; a save MUST never leave a partially written settings file (the previous file stays valid until the new one is complete).
- **FR-010** (req FR-13, FR-21; Clarification Q4): When the settings file exists but cannot be read, the system MUST keep it as a timestamped backup, start with the defaults of FR-005 (opening settings on the Engine tab), notify "settings could not be read and were reset", and leave stored keys untouched. If the file cannot be moved aside, or a read fails with an I/O error other than not-found (locked, no permission), the system MUST run on the defaults in memory, MUST NOT write or move the file, MUST refuse saves with a notice until restart, and MUST NOT touch the credential store (decisions #19).

**Language**

- **FR-011** (req FR-15, FR-21): On first run the UI language MUST be Russian when the Windows display language is Russian (any `ru` variant) and English otherwise, including when the OS language cannot be read. Once saved, the UI language changes only through settings.
- **FR-012** (req FR-15): Every user-visible text — settings window, tray menu, overlay, notifications, error messages, including those of features 001–006 — MUST come from one message catalog with an English and a Russian text for every message; a message present in one language and missing in the other, or with different placeholders, MUST fail the gate. User-entered or user-editable content (the post-processing prompt, including 003's starter prompt; model and device names; transcripts) is not a catalog message and is shown as is. Speech-language names are rendered from their language codes in the current UI language by the platform's locale data, not stored in the catalog.
- **FR-013** (req FR-15): Changing the UI language MUST apply to all open windows, the tray menu and the next notification without restart (FR-008).

**Keys**

- **FR-014** (req NFR-04): API keys (transcription API, local server, post-processing) MUST be stored only in Windows Credential Manager, one entry per key slot, and MUST NOT appear in the settings file, the UI state sent to windows, logs, crash files, error messages or notifications.
- **FR-015** (req NFR-04): The settings window MUST never display a stored key; it shows whether a key is stored, and offers to replace or clear it. An untouched key field keeps the stored key on save.
- **FR-016** (req NFR-04): If the credential store cannot store, read or delete a key, the system MUST refuse the save (or report the failure) without falling back to any other storage.

**Test connection**

- **FR-017** (req FR-14): For engine = API or local server, the settings window MUST offer "Test connection", which sends a bundled ~1 s speech sample through the same transcription client as dictation (req FR-06, FR-17) using the values currently in the form (the stored key when the key field is untouched), and reports: "OK" with latency in ms; "cannot reach <host>" (DNS failure, connection refused, connect timeout); "invalid API key" (HTTP 401/403); "timeout" (FR-24 limits exceeded); otherwise "HTTP <status>" or "unexpected response". It MUST NOT change saved settings or stored keys, and MUST be disabled while a test runs (Clarification Q3).
- **FR-018** (req FR-14, NFR-05): Test connection MUST NOT apply voice-activity detection to the sample and is the only network call the settings window makes on its own (besides model downloads owned by 002).

**Start with Windows**

- **FR-019** (req FR-19): When "start with Windows" is on (default off), the system MUST register a per-user logon start of the installed app that starts it minimized to the tray without opening a window; when off, the registration MUST be removed. A failure to write or remove it refuses the save (FR-004).

**Insecure endpoint warning**

- **FR-020** (req FR-29): When a saved base URL that is in use (the selected engine's base URL; the post-processing endpoint while post-processing is on) uses `http://` on a host that is not a loopback host (`localhost` in any letter case, any `127.x.x.x`, `::1`), the system MUST show a warning that the API key travels unencrypted and still save. `https://` URLs and loopback hosts get no warning.

**Logging**

- **FR-021** (req FR-20, NFR-04): The system MUST log settings loads and saves (which fields changed, by name only, and the outcome) and first-run/reset events. It MUST NOT log any field value: never keys, never the prompt text, and not base URLs either (a URL may carry credentials).

### Key Entities

- **Settings**: everything the user configures except keys: engine choice; per-engine configuration (API: base URL, model; local server: base URL, model; built-in local: selected model id); speech language (auto or a language code); microphone (Windows default or a device id with its last known name); hotkey (modifiers + key) and mode (hold/toggle); auto-paste; post-processing (on, endpoint, model, prompt); history (on, N); start with Windows; UI language (en/ru); schema version. Persisted as one file in the app data folder.
- **Key slot (Secret)**: one API key per slot — transcription API, local server, post-processing — held only in the OS credential store; the settings model knows only whether each slot holds a key.
- **Message catalog**: message id → English text and Russian text, with named placeholders (e.g. `{host}`); the single source of all user-visible text.
- **Save outcome**: success with optional warnings (insecure endpoint), or refusal with a list of field errors (field id + message id).
- **Connection test result**: OK + latency, or an error kind (unreachable host, invalid key, timeout, HTTP status, unexpected response) with the host.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: From launching the installer to the first pasted transcript takes ≤ 3 minutes on a clean Windows 11 with an API key at hand (req NFR-10; owner's run-through in Windows Sandbox).
- **SC-002**: 100% of the settings listed in FR-001 take effect on their next use without a restart and keep their value after a restart (one automated check per setting).
- **SC-003**: Every invalid-input case listed in FR-004 is refused with the right field highlighted (one automated check per rule), and a refused save changes nothing.
- **SC-004**: A known test key value is found 0 times in the settings file, the log files and the crash files after saving it, testing a connection with it and failing a request with it (req NFR-04).
- **SC-005**: 100% of user-visible messages exist in both English and Russian with the same placeholders (gate check), and a first start on a Russian Windows shows Russian everywhere the owner looks (manual check).
- **SC-006**: Test connection reports a result for every case of FR-017 within the FR-24 limits (≤ 35 s for the API engine, ≤ 65 s for a local server, worst case).
- **SC-007**: With start with Windows on, the tray icon is present after logon in 100% of the owner's reboot checks; with it off, the app is absent.

## Assumptions

- Settings are applied only by an explicit Save (FR-13 says "changes any setting and saves"); there is no auto-save.
- "Local host" for FR-29 means loopback only; private LAN addresses (e.g. `192.168.x.x`) get the warning because keys do travel unencrypted on the LAN.
- History size N is limited to 1–100 (requirements give only the default 20); 005 may narrow it.
- Post-processing is off by default (requirements give no default; it needs an endpoint). Its endpoint/model/prompt defaults are 003's.
- The speech-language list is the set of languages the Whisper models support that have an ISO 639-1 code, plus "auto-detect": Whisper's `jw` (ISO: `jv`), Hawaiian (`haw`) and Cantonese (`yue`) are excluded, so Javanese is not offered (decisions #27, #28).
- The first-run defaults are persisted immediately, so closing the first-run window without choosing an engine leaves engine = none and the next start is tray-only.
- The UI language is stored explicitly (en/ru) on first run, not as "follow the OS".
- The local-server model has no default because local servers name models differently; it is optional, and when empty the request carries no model (002 Clarification, FR-017).
- The overlay and settings webviews are created on demand and destroyed when closed (req NFR-03); the settings window keeps no state of its own beyond unsaved edits.

## Dependencies

- 001-dictation-via-api: hotkey registration with "keep old on failure" semantics (req FR-05), tray menu entry "Settings" (req FR-01), the pipeline start point where the engine = none gate sits, the OpenAI-compatible transcription client and FR-24 timeouts (req FR-06, FR-17, FR-24), the notification mechanism.
- 002-local-transcription: the list of downloaded models and the download/deletion controls hosted on the Engine tab (req FR-08, FR-28).
- 003-llm-post-processing: post-processing defaults and behaviour (req FR-09).
- 005-transcript-history: behaviour when history is turned off or N shrinks (req FR-16).
- 006-diagnostics-and-release: the log (req FR-20) used by FR-021; the installer/uninstaller removing the autostart entry and keys (req FR-28).

## Verification (where each requirement is checked)

| Req | Linux host — core with fakes | Linux host — UI, Playwright with mocked IPC | Windows CI runner | Owner's manual check |
|---|---|---|---|---|
| FR-13 | validation rules, defaults, all-or-nothing save, atomic persistence, live apply to subscribers | every field rendered, edited, saved; refusal highlights; discard prompt | settings file in `%LOCALAPPDATA%\Voicen`, hotkey re-registration on save | each field takes effect on a real PC |
| FR-14 | result mapping against a mock OpenAI-compatible server | messages rendered, button disabled while running | — | test against real OpenAI/Groq |
| FR-15 | language resolution from an OS tag, catalog parity, placeholder parity | UI in en and ru, live switch | OS language read returns a tag | Russian Windows first start; tray and toasts in Russian |
| FR-19 | save applies/removes via a fake autostart | toggle saved | logon entry written/removed for the current user | reboot with on/off |
| FR-21 | first-run detection, defaults, engine-none gate | first-run window on Engine tab with defaults | silent install + first launch logs "first run" | fresh install in Windows Sandbox |
| FR-29 | loopback classification | warning shown/not shown | — | — |
| NFR-04 | settings file and logs free of keys with a fake credential store; redacted debug output | key never in UI state; masked field | Credential Manager write/read/delete round trip | — |
| NFR-10 | — | — | — | timed run-through in Windows Sandbox |
