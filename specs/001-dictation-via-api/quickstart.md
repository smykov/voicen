# Quickstart: validating dictation via an OpenAI-compatible API

How to prove the feature works, from the Linux host up to the owner's PC. Contracts: [core-traits](contracts/core-traits.md), [openai-transcription](contracts/openai-transcription.md), [ipc](contracts/ipc.md), [messages](contracts/messages.md). Entities: [data-model.md](data-model.md).

## Prerequisites

```sh
pnpm install && make core-image      # the image needs cmake/clang only if the owner approved Silero on Linux (research R-6)
```

## 1. Linux host — the gate

```sh
make check
```

Expected: green. Target checks in it:

```sh
scripts/tw-run core -- cargo test -p voicen-core openai_client   # every row of the response table, timeouts, no-key, trailing slash, redaction
scripts/tw-run core -- cargo test -p voicen-core pipeline        # US1 + US2 + US5 flows with fakes
scripts/tw-run core -- cargo test -p voicen-core recording       # hold/toggle/0.3 s/10 min/Esc/auto-repeat
scripts/tw-run core -- cargo test -p voicen-core queue           # ordering incl. failure, no speech, retry
scripts/tw-run core -- cargo test -p voicen-core vad             # energy gate on fixtures; fallback warning
pnpm test -- src/lib/overlay/state.test.ts
pnpm e2e -- e2e/overlay.spec.ts
```

Key observable outcomes:

- **The pipeline test with a wiremock 401** produces no `Clipboard` call, a `Notifier` call with `failure.invalid_api_key` and a retry id, the tray `Error` state, an overlay `message` for 3 s, and one pending file in the fake store. The next 200 response plus `retry()` yields one clipboard write and paste, and an empty store.
- **The ordering test**: job A (slow mock) and job B (fast mock) are delivered A then B, each to its own fake start window. With A answering 500, B is still delivered and A becomes the pending recording.
- **The redaction test**: the key `sk-test-SECRET` and the transcript `TRANSCRIPT-MARKER` never appear in any captured `DictationEvent` or error text.

## 2. Windows CI runner

`cargo test --workspace` (with the features `silero` and `test-audio`), then `pnpm tauri build` and the install smoke (`.github/workflows/ci.yml`). Expected:

- **Win32 integration tests** (`src-tauri/tests`):
  - With a test window focused, the injected WAV source and a local mock endpoint, a synthetic hotkey press/release puts the mock transcript into the test window's edit control and onto the clipboard.
  - The clipboard formats `CanIncludeInClipboardHistory = 0` and `CanUploadToCloudClipboard = 0` are present.
  - The test window saw no hotkey character and no menu activation.
- **Hotkey conflict**: with the hotkey pre-registered by a helper process, start-up gives the tray `HotkeyError`. After a simulated resume message, re-registration is attempted.
- **Capture timing**: hotkey → first frame p95 ≤ 200 ms over 20 runs with the injected source (SC-002 proxy).
- **Overlay**: after it is shown, the foreground window is still the test window.
- **Smoke**: the installed app starts and its log carries the commit. After exit, `%LOCALAPPDATA%\Voicen\tmp\audio` is empty.

## 3. Owner's manual check (Windows 11 PC)

Settings until 004 lands: `%LOCALAPPDATA%\Voicen\settings.json` with `{"engine":"api","api_base_url":"https://api.openai.com/v1","api_model":"whisper-1"}`; the key goes in Credential Manager (target `Voicen/transcription-api`, written by 004's `CredentialStore`).

1. Focus Notepad, hold Ctrl+Alt+Space for 4 s and speak, then release. The text appears in Notepad, Win+V does not list it, the overlay showed recording then processing, and Notepad kept focus.
2. Switch to toggle mode in the settings file. Press, speak 10 s, press: the text appears. Press, speak, Esc: nothing. Esc with no recording reaches Notepad.
3. Set a wrong key. Dictate: toast "Invalid API key" with Retry, a 3 s overlay message, the tray error icon, nothing pasted. Fix the key and click Retry: the text is pasted if Notepad is in front, otherwise "Copied — paste manually".
4. Turn on Focus Assist and repeat step 3: the overlay and tray still report the failure.
5. Unplug the selected USB mic and press the hotkey: one toast names the default device. Unplug it mid-recording: the captured part is pasted.
6. Sleep and resume, lock and unlock, restart Explorer: the hotkey works 3/3 each time, and the tray icon returns (SC-006).
7. NFR-08 list (SC-004): Notepad, Chrome, Edge, VS Code, Telegram Desktop, Word.
8. Benchmark (SC-001): the 20-phrase set via OpenAI `whisper-1` and via Groq `whisper-large-v3-turbo`. Compute p90 stop → paste from the `JobFinished` + `Delivered` timings in the log.
