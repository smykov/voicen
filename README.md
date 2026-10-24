# Voicen

Dictation into any text field on Windows. Press a hotkey, speak, and the text is pasted into the field you were typing in — a chat, an AI prompt, a document, a code editor.

**Status: in development, no release yet.** Version 0.1.0; the dictation loop through an API engine works and is verified on Windows CI. The first public build will be published in GitHub Releases.

## How it works

1. Voicen runs in the tray.
2. You focus any input field and press the global hotkey — hold to talk, or press once to start and once to stop.
3. The recording is transcribed by the engine you chose.
4. Optionally, an LLM cleans up the text with a prompt you can edit.
5. The text goes to the clipboard and is pasted into the field with Ctrl+V. If pasting is not possible, it stays in the clipboard and you get a notification.

If something fails — no network, a wrong key, the microphone disappeared — you see the reason, and the recording is kept so you can retry.

## Engines

| Engine | Where speech is processed | When to use it |
|---|---|---|
| OpenAI-compatible API (OpenAI, Groq and others) | the provider you choose, with your own key | best quality and speed |
| Built-in whisper.cpp | on your computer, model downloaded from the app | privacy, no internet |
| Local OpenAI-compatible server (speaches, faster-whisper and others) | a server you run, e.g. `http://localhost:8000/v1` | your own GPU box |

Keys are stored in Windows Credential Manager. Audio is not saved to history — only the last transcripts as text, and history can be turned off.

## Recording calls

Call mode is in development and is not in a release yet.

**Consent is your responsibility.** Before you record a call, tell the other party and ask for their consent; the rules differ by country. Voicen cannot notify them for you. A recording is always visible — there is no secret mode — and at each start Voicen reminds you to tell the other party; the reminder is on by default and can be turned off in the Calls settings. A sample opening phrase:

> Just so you know, I'm recording this call to keep notes. Is that OK with you?

**Where the audio goes.** If the transcription engine is not on your computer (an API provider), the other party's audio is sent to that provider.

**Storage.** Calls are stored unencrypted in `%LOCALAPPDATA%\Voicen\calls` or in the folder you choose. Anyone signed in to your Windows account can read them. Uninstalling keeps them unless you answer "yes" to the separate question about recorded calls, or a silent uninstall is run with `/PURGECALLS`. A folder you chose yourself is never deleted by the uninstaller.

**Starting from the command line.** `voicen.exe --call record` starts a recording in the running Voicen. Any program running under your Windows account can do the same; the recording indicator is always shown, so such a recording cannot stay hidden.

### Русский

Режим записи звонков ещё в разработке и пока не вошёл ни в один релиз.

**Согласие собеседника — ваша ответственность.** Перед записью звонка предупредите собеседника и спросите его согласия; правила в разных странах разные. Voicen не может сообщить ему о записи за вас. Запись всегда видна — скрытого режима нет, — и при каждом старте Voicen напоминает предупредить собеседника; напоминание включено по умолчанию, его можно отключить в настройках звонков. Пример фразы в начале разговора:

> Предупреждаю: я записываю этот разговор, чтобы не потерять детали. Вы не против?

**Куда уходит звук.** Если движок распознавания работает не на вашем компьютере (API-провайдер), звук собеседника отправляется этому провайдеру.

**Хранение.** Звонки хранятся без шифрования в `%LOCALAPPDATA%\Voicen\calls` или в выбранной вами папке. Их может прочитать любой, кто вошёл в вашу учётную запись Windows. При удалении программы они сохраняются, если только вы не ответите «да» на отдельный вопрос о записанных звонках или тихое удаление не запущено с ключом `/PURGECALLS`. Папку, которую вы выбрали сами, деинсталлятор не удаляет никогда.

**Запуск из командной строки.** `voicen.exe --call record` начинает запись в уже запущенном Voicen. То же может сделать любая программа, работающая под вашей учётной записью Windows; индикатор записи показывается всегда, поэтому такая запись не останется незаметной.

## Built by an AI agent team

Voicen is written by Claude Code agents working under [teamwright](https://github.com/smykov/teamwright), a process plugin that turns each step of a development team into a checked record. The owner sets requirements and accepts results; agents do the analysis, tests, code and review. All of it stays in the repository:

- `docs/requirements.md` — what must be true, approved by the owner, with the review rounds in `docs/requirements.reviews/`;
- `specs/` — specifications per feature;
- `docs/tasks/` — every task with its analysis, review by a separate agent (`*.reviews`), validation and verification records;
- `docs/decisions.md`, `docs/failures.md`, `docs/process-reviews/` — decisions, what went wrong and how the process changed.

Repository started on 2026-10-02; about 1,300 commits and 80 tasks in the first ten days.

## Architecture

A Tauri 2 desktop app:

- **`crates/voicen-core`** (Rust) — the platform-independent pipeline: voice activity detection → engine → optional post-processing → delivery decision; settings, history, local models, messages in English and Russian.
- **`src-tauri`** (Rust) — everything Windows-specific: tray, global hotkey (`RegisterHotKey`), audio capture (cpal), clipboard, `SendInput` paste, Credential Manager, notifications.
- **`src`** (Svelte, TypeScript) — settings, first run, history and the recording overlay, talking to Rust only through Tauri IPC.

Details: [`docs/architecture.md`](docs/architecture.md).

## Build from source

On Windows 10/11 with Rust (stable, MSVC toolchain), Node.js, pnpm and the [Tauri prerequisites](https://tauri.app/start/prerequisites/):

```powershell
pnpm install
pnpm tauri dev      # run in development mode
pnpm tauri build    # build the installer
```

Checks run with `make check` (Rust and front-end tests, linters, repository checks); CI runs them on Linux and builds and tests the app on Windows.

## Earlier version

[voicen-python](https://github.com/smykov/voicen-python) — the first prototype: a tray app in Python that sends recordings to a Whisper API, built in one evening.

## License

MIT — see [LICENSE](LICENSE). Third-party licenses: [`THIRD-PARTY-NOTICES.txt`](THIRD-PARTY-NOTICES.txt).
