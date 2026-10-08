# Feature Specification: Local Transcription

**Feature Branch**: `002-local-transcription`

**Created**: 2026-10-02

**Status**: Approved (clarifications confirmed by the owner 2026-10-02)

**Input**: User description: "Local transcription: a built-in whisper.cpp engine that transcribes offline with a downloaded model (multilingual tiny, base, small recommended, medium q5_0, large-v3-turbo q5_0), with download progress, pinned SHA-256 verification, retry on failure, deletion of a downloaded model, the model kept loaded and unloaded after 10 minutes idle; and a local-server engine that reuses the OpenAI-compatible transcription client with a user-given URL and an optional key."

**Requirement ids in scope** (`docs/requirements.md` v3): FR-07, FR-08, FR-17, FR-24 (local-server timeout), FR-28 (model deletion only; the uninstaller part belongs to `006-diagnostics-and-release`), NFR-01 (local part, warm model), NFR-03, NFR-05.

**Depends on**: `001-dictation-via-api` — the engine interface, the OpenAI-compatible transcription client (FR-06), the dictation pipeline (VAD gate FR-12, delivery FR-10, failure handling and pending recording FR-11, notifications FR-25, log timings FR-20, the connect timeout of FR-24). `004-settings-and-first-run` — the settings window that hosts the engine choice, model list and local-server fields (FR-13), the speech-language setting, key storage UI (NFR-04), the `http://` warning (FR-29).

## Clarifications

### Session 2026-10-02

- Q: When is the selected built-in model loaded into memory if it is not loaded yet? → A: Loading starts when the hotkey starts a recording with the built-in engine selected, in parallel with the recording; it is never loaded at app start. (confirmed by the owner 2026-10-02)
- Q: What counts as "idle" for the 10-minute unload, and what else unloads the model? → A: The 10 minutes count from the latest end of a recording, a transcription or a model load; a recording or transcription in progress holds the countdown. The model is also unloaded at once when the engine changes away from built-in, when another model is selected, and before the loaded model is deleted. (confirmed by the owner 2026-10-02)
- Q: Can the user cancel a model download that is in progress? → A: Yes; cancelling stops the download, deletes the partial file and returns the model to "not downloaded" without a retry prompt. (confirmed by the owner 2026-10-02)
- Q: Which model name does the local-server engine send to the server? → A: The local-server engine has its own optional "model" field; when it is set it is sent as the request's model, when it is empty the request carries no model. (confirmed by the owner 2026-10-02)
- Q: When does a download that stops receiving data count as interrupted? → A: After 30 s without receiving any data (the connect timeout of 5 s per FR-24 applies to opening the connection); it then follows the interrupted-download branch of FR-08. (confirmed by the owner 2026-10-02)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Download a local model (Priority: P1)

The user opens the engine settings, sees the five offered models with their sizes (`small` marked as recommended), picks one, watches a progress bar while it downloads, and the model becomes selectable once its checksum has been verified. If the download breaks or the file is corrupt, the partial file is removed and the user is offered a retry.

**Why this priority**: Nothing local works without a verified model on disk; it is the precondition of the built-in engine and the main privacy job ("my voice never leaves the computer").

**Independent Test**: Against a mock download server serving a small fake model file whose SHA-256 is pinned in a test catalog, request the download and observe progress events, the final "downloaded" state, the file on disk, and — for a truncated or altered file — the partial file deleted and a retry offered.

**Acceptance Scenarios**:

1. **Given** no model is downloaded, **When** the user opens the local model list, **Then** exactly the five models tiny, base, small, medium (q5_0) and large-v3-turbo (q5_0) are offered, all multilingual, each with its download size, and `small` is marked as recommended (req FR-08).
2. **Given** the user picks `base`, **When** the download runs, **Then** a progress bar shows the received and total size and percentage, and when the download completes and the SHA-256 matches the pinned value, the model becomes selectable (req FR-08).
3. **Given** a download is in progress, **When** the connection drops (or no data arrives for 30 s), **Then** the partial file is deleted, the model is shown as not downloaded with the reason, and a "Retry" action is offered (req FR-08, failure branch).
4. **Given** a download completed, **When** the computed SHA-256 does not match the pinned value, **Then** the file is deleted, the user is told the file was corrupt, and a "Retry" action is offered; the model is never selectable (req FR-08, failure branch).
5. **Given** a failed download, **When** the user clicks "Retry", **Then** the download starts again from zero and follows scenarios 2–4.
6. **Given** a download is in progress, **When** the user cancels it, **Then** the download stops, the partial file is deleted and the model shows as not downloaded, without a retry prompt.
7. **Given** a model was downloaded, **When** the network is off and the app restarts, **Then** the model is still listed as downloaded and selectable (req FR-08 "available offline").

---

### User Story 2 - Dictate offline with the built-in engine (Priority: P1)

With the engine set to "built-in local" and a downloaded model selected, the user dictates as usual; the audio is transcribed on their machine without any network access and the text is delivered like any other transcript. The model stays in memory after use so the next dictation is fast, and leaves memory after 10 minutes without use.

**Why this priority**: This is the offline, private dictation the owner asked for and stage 2's done criterion ("offline dictation with a downloaded `small` model").

**Independent Test**: In the core, with a fake speech model behind the engine interface and a network fake that records every attempted request, run a recording through the pipeline with engine = built-in: the text is produced, zero network requests are made, the model loads once, stays loaded for the second dictation, and is unloaded after a fake clock advances 10 minutes. On the Windows CI runner, the real whisper.cpp engine transcribes a bundled WAV with the cached `tiny` model.

**Acceptance Scenarios**:

1. **Given** model `small` is downloaded and selected and the network is off, **When** the user dictates a phrase, **Then** the transcript is produced on the machine and delivered per FR-10 (req FR-07).
2. **Given** engine = built-in local and no model is downloaded (or the selected model's file is missing), **When** the user finishes a recording, **Then** nothing is pasted, a notification "no local model" with a link that opens the engine settings is shown, and no transcription is attempted (req FR-07, failure branch).
3. **Given** engine = built-in local and post-processing off, **When** the user dictates any number of times, **Then** the app makes no network request at all (req NFR-05).
4. **Given** the model is not loaded, **When** the user starts a recording, **Then** loading starts in parallel with the recording, and the first ("cold") dictation is logged as cold (req NFR-01, NFR-03).
5. **Given** a dictation just finished, **When** the user dictates again within 10 minutes, **Then** the model is not reloaded (the dictation is logged as warm) (req NFR-03, NFR-01).
6. **Given** the model is loaded, **When** 10 minutes pass with no recording or transcription, **Then** the model is unloaded and its memory released (req NFR-03).
7. **Given** the selected model's file exists but cannot be loaded or transcription fails inside the engine, **When** a recording finishes, **Then** nothing is pasted, the user is notified with the reason, the audio is kept for retry per FR-11, and the app keeps running (req FR-07, FR-11, NFR-07).
8. **Given** the speech language is set to a fixed language, **When** the built-in engine transcribes, **Then** it transcribes in that language; with "auto-detect", it detects the language (req FR-13 speech language, FR-07).

---

### User Story 3 - Use a local OpenAI-compatible server (Priority: P2)

A user who already runs a local transcription server (speaches, faster-whisper-server, whisper.cpp server, …) chooses the "local server" engine, enters its URL (for example `http://localhost:8000/v1`), optionally a model name and a key, and dictates; the audio is sent to that server the same way as to a cloud API.

**Why this priority**: Must in release 1, but serves a smaller audience than the built-in engine; it reuses the client built in 001, so it is mostly configuration and failure handling.

**Independent Test**: In the core, point the local-server engine at a mock OpenAI-compatible server: a transcription request arrives at `<URL>/audio/transcriptions` without an `Authorization` header when no key is set, the text comes back; with the mock stopped, the failure reason is "cannot reach <host:port>" and the audio is kept; with the mock not answering, the request fails with reason "timeout" after 60 s (fake or shortened clock).

**Acceptance Scenarios**:

1. **Given** a speaches/faster-whisper server at `http://localhost:8000/v1` and no key, **When** the user dictates with engine = local server, **Then** the transcript is returned and delivered per FR-10, and the request carried no key (req FR-17).
2. **Given** a key is entered for the local server, **When** a request is made, **Then** the key is sent as a bearer token, read from the OS credential store, and never written to the settings file or logs (req FR-17, NFR-04).
3. **Given** the server is not running, **When** the user finishes a recording, **Then** the notification says "cannot reach localhost:8000", nothing is pasted, and the audio is kept for retry (req FR-17 failure branch, FR-11).
4. **Given** the server accepts the connection but does not answer within 60 s, **When** the timeout elapses, **Then** FR-11 applies with reason "timeout" (req FR-24).
5. **Given** the server cannot be connected to within 5 s, **When** the connect timeout elapses, **Then** FR-11 applies with reason "cannot reach <host:port>" (req FR-24 connect 5 s, FR-17).
6. **Given** the server returns 401/403 because a key is required, **When** a recording is transcribed, **Then** the notification says "invalid API key" and nothing is pasted (req FR-17 via the shared client of FR-06).

---

### User Story 4 - Delete a downloaded model (Priority: P3)

The user frees disk space by deleting a model they no longer need. If it was the selected model of the built-in engine, the engine becomes "none" so the next hotkey press asks the user to choose an engine.

**Why this priority**: Should in release 1 (FR-28); useful for large models but not on the critical dictation path.

**Independent Test**: In the core with a temporary models directory: delete a downloaded model; the file is gone, the model shows as not downloaded, and when it was the selected one the engine setting is "none" and persisted.

**Acceptance Scenarios**:

1. **Given** `small` is downloaded, **When** the user deletes it, **Then** the file is removed from disk, the space is freed, and `small` is shown as not downloaded (req FR-28).
2. **Given** `small` is downloaded and selected for the built-in engine, **When** the user deletes it, **Then** the model is unloaded if loaded, the file is removed, and the engine becomes "none" (req FR-28).
3. **Given** a transcription with `small` is running, **When** the user deletes `small`, **Then** deletion is refused with "model is in use — try again when the transcription finishes", and nothing changes (edge case).
4. **Given** the file cannot be deleted (locked or permission denied), **When** the user deletes it, **Then** the user is told the reason, the model remains listed as downloaded and stays usable (edge case).

---

### Edge Cases

- **No model selected but one is downloaded**: engine = built-in requires a selected model; the settings UI offers only downloaded models for selection. If the selected model is not downloaded (e.g. its file was removed outside the app), FR-07's failure branch applies ("no local model" with a link to the settings).
- **Model file changed on disk outside the app**: the checksum is verified at download; at load time a file of the wrong size is treated as missing (FR-07 failure branch); a file that loads but fails inside the engine follows User Story 2 scenario 7.
- **Not enough disk space**: checked before the download starts against the catalog size; if insufficient, the download does not start and the user is told how much space is needed. A disk-full error during the download follows the interrupted branch (partial file deleted, retry offered).
- **App exits or crashes during a download**: at the next start, any leftover partial download file is deleted and the model is shown as not downloaded.
- **Two downloads requested**: one download runs at a time; other models' download buttons are disabled while one is running.
- **Engine switched or another model selected while loaded**: the loaded model is unloaded immediately (Clarification 2).
- **Hotkey pressed while the model is still loading**: recording proceeds; transcription waits for loading to finish; loading failure follows User Story 2 scenario 7.
- **VAD finds no speech** (FR-12, owned by 001): the built-in engine is not called; the idle countdown, cancelled when the recording started, restarts when the recording ends.
- **Long recording on a slow CPU with a large model**: the built-in transcription is bounded by 120 s (requirements v4 FR-24); when it elapses, FR-11 applies with reason "timeout" and the audio is kept for retry.
- **Multiple recordings in flight** (FR-23, owned by 001): the built-in engine transcribes them one at a time in recording order with the single loaded model.
- **Local server on a non-local host over `http://`**: FR-29's warning (owned by 004) applies; with `localhost`/`127.0.0.1` no warning.
- **Local server URL invalid or empty**: save is refused with the field highlighted (FR-13, owned by 004).
- **Local server returns a non-2xx other than 401/403, or a malformed body**: FR-11 applies with reason "server error <status>" or "invalid response", audio kept.
- **Model download source unreachable** (no network, DNS failure, HTTP 404/5xx): follows the interrupted branch of FR-08 with the reason (e.g. "cannot reach huggingface.co"); partial file (if any) deleted, retry offered.
- **Retry of a failed built-in transcription** (FR-11) after the model was deleted: FR-07's failure branch applies ("no local model"), the audio stays pending.

## Requirements *(mandatory)*

### Functional Requirements

**Model catalog and download**

- **FR-001** (req FR-08): The system MUST offer exactly five whisper models for download — multilingual tiny, base, small, medium (q5_0), large-v3-turbo (q5_0) — each with its display name, download size and a SHA-256 value pinned in the application; `small` MUST be marked as recommended.
- **FR-002** (req FR-08): When the user starts a download, the system MUST report progress (bytes received, total bytes, percentage) to the settings window at least once per second while data arrives.
- **FR-003** (req FR-08): After the download completes, the system MUST compute the file's SHA-256 and compare it with the pinned value; only a matching file becomes a downloaded, selectable model. Until then the file MUST NOT be visible under the model's final name.
- **FR-004** (req FR-08 failure branch): If the download is interrupted (connection error, HTTP error, disk error, 30 s without data, or 5 s connect timeout per req FR-24) or the checksum does not match, the system MUST delete the partial or corrupt file, show the model as not downloaded with the reason, and offer "Retry", which restarts the download from zero.
- **FR-005** (req FR-08; Clarification 3): The user MUST be able to cancel a running download; cancelling deletes the partial file and shows the model as not downloaded, without a retry prompt.
- **FR-006** (req FR-08): The system MUST run at most one model download at a time.
- **FR-007** (req FR-08 failure branch, edge case): Before starting a download, the system MUST check free disk space against the model's size and refuse with "not enough disk space (<size> needed)" if insufficient.
- **FR-008** (req FR-08 "available offline"): Downloaded models MUST be stored under `%LOCALAPPDATA%\Voicen\models` and MUST be listed as downloaded at every start without network access; leftover partial files from an interrupted session MUST be deleted at start.

**Built-in local engine**

- **FR-009** (req FR-07): When recording stops and the engine is "built-in local", the system MUST transcribe the audio on the user's machine with whisper.cpp and the selected downloaded model, honouring the speech-language setting (auto-detect or fixed), and pass the text to the normal delivery path (FR-10, owned by 001).
- **FR-010** (req FR-07, NFR-05): The built-in engine MUST make no network request; with the built-in engine and post-processing off, the app MUST make no network request other than a model download or an explicit "Test connection" started by the user.
- **FR-011** (req FR-07 failure branch): If the engine is "built-in local" and no model is downloaded or the selected model is not downloaded, the system MUST paste nothing, attempt no transcription, notify "no local model" with an action that opens the engine settings, and keep the audio as the pending recording per FR-11 so it can be retried once a model is downloaded.
- **FR-012** (req FR-07, FR-11, NFR-07): If loading the model or transcribing fails inside the engine, the system MUST paste nothing, notify the reason, keep the audio as the pending recording per FR-11, and keep running.
- **FR-013** (req NFR-03, NFR-01): The selected model MUST stay loaded after a transcription; it MUST be unloaded after 10 minutes without use, counted from the latest end of a recording, a transcription or a model load; while a recording or transcription is in progress the countdown is held.
- **FR-014** (req NFR-03): The model MUST be unloaded at once when the engine changes away from "built-in local", when another model is selected, and before the loaded model is deleted. It MUST NOT be loaded at app start.
- **FR-015** (req NFR-01): If the model is not loaded when a recording with the built-in engine starts, loading MUST start in parallel with the recording; each dictation's log line (FR-20, owned by 001) MUST mark the built-in engine's dictation as `cold` (model loaded for it) or `warm` (model already loaded), and log the load time for a cold one.
- **FR-016** (req FR-23, owned by 001): The built-in engine MUST process one transcription at a time, in the order received.

**Local server engine**

- **FR-017** (req FR-17): When the engine is "local server", the system MUST transcribe through the same OpenAI-compatible transcription client as the API engine (FR-06, built in 001), with the user-given base URL, an optional model name (sent only when set) and an optional key (sent as a bearer token only when set).
- **FR-018** (req FR-17, NFR-04): The local-server key MUST be stored only in the OS credential store, separately from the API engine's key, and never in settings files or logs.
- **FR-019a** (req FR-24 v4): Built-in transcription MUST stop waiting after 120 s; on timeout FR-11 applies with reason "timeout" and the audio stays pending.
- **FR-019** (req FR-24): Local-server transcription MUST use the configured connect timeout and total request timeout (defaults 5 s and 60 s, decision #99); on timeout FR-11 applies with reason "timeout".
- **FR-020** (req FR-17 failure branch, FR-11): If the server cannot be reached, the system MUST paste nothing, notify "cannot reach <host:port>" (e.g. "cannot reach localhost:8000") and keep the audio for retry.

**Model deletion**

- **FR-021** (req FR-28): The user MUST be able to delete any downloaded model; the file is removed from disk and the model shows as not downloaded.
- **FR-022** (req FR-28): If the deleted model was selected for the built-in engine, the system MUST unload it if loaded and set the engine to "none", persisted.
- **FR-023** (req FR-28, NFR-07; edge case): Deleting a model that is currently transcribing MUST be refused with "model is in use"; a failure to remove the file MUST be reported and the model left downloaded and usable.

**Privacy and logging**

- **FR-024** (req NFR-05, FR-20): Logs MUST record model download start, completion, failure reason, deletion, load (with duration) and unload events with the model name only — never audio, transcript text, keys or the local-server key.
- **FR-025** (req FR-15): Every user-visible text this feature adds (model list, progress, download and deletion errors, "no local model", local-server errors) MUST be provided in English and Russian through the app's localisation (owned by 004).

### Key Entities

- **LocalModel** (catalog entry, from requirements §7): id (tiny, base, small, medium-q5_0, large-v3-turbo-q5_0), display name, download URL, size in bytes, pinned SHA-256, recommended flag. Fixed in the application.
- **LocalModelState** (per model, derived at start from disk): not downloaded / downloading (bytes received, total) / downloaded (path) / failed (reason, retry offered). Persisted only as the file on disk; "failed" and "downloading" live in memory.
- **LoadedModel**: at most one; the model id, load time, and the idle countdown deadline.
- **LocalServerConfig** (part of Settings, owned by 004): base URL, optional model name, reference to an optional key in the credential store.
- **EngineChoice** (part of Settings): none / API / built-in local (with selected model id) / local server.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001** (req NFR-01): Over the fixed set of 20 phrases of 5–15 s, p90 stop → paste with the built-in `small` model, warm, without post-processing, is ≤ 10 s on the reference machine (OQ-03), measured from the log timings.
- **SC-002** (req NFR-03): Idle in tray with no model loaded and no window open, CPU < 1 % and RAM ≤ 150 MB (app process plus its WebView2 child processes); 10 minutes after the last local transcription the model's memory is released.
- **SC-003** (req NFR-05): With the built-in engine and post-processing off, a dictation session produces zero outbound network connections (checked with a request-recording network fake in core tests and by code review of network call sites).
- **SC-004** (req FR-08): 100 % of corrupted or truncated downloads are rejected (never selectable) and leave no file behind; a retry after a transient failure completes the download.
- **SC-005** (req FR-07, stage 2): The user can go from "no model" to an offline dictation pasted into Notepad using only the settings window and the hotkey.
- **SC-006** (req FR-17): Dictation through a local server at `http://localhost:8000/v1` without a key delivers text; a stopped server yields "cannot reach localhost:8000" with the audio kept for retry.

## Assumptions

- The five models are downloaded from the Hugging Face repository `ggerganov/whisper.cpp` (requirements §8), pinned to one repository revision so the pinned SHA-256 values stay valid.
- "Multilingual" means the non-`.en` ggml files; English-only variants are not offered.
- The built-in engine transcription times out after 120 s (requirements v4 FR-24, decisions #7).
- Speech language, engine choice and local-server fields are edited in the settings window owned by 004; this feature provides the core behaviour and the IPC commands that window calls.
- The VAD gate (FR-12), delivery (FR-10), pending recording and retry (FR-11), notifications (FR-25) and the log line with timings (FR-20) are provided by 001; this feature plugs two engines into them.
- The local-server engine's "Test connection" (FR-14, Should) is provided by whichever feature owns FR-14 for API engines; it reuses the same client with the local-server settings.
- whisper.cpp runs on the CPU only (GPU acceleration is out of scope, requirements §3).
