# Quickstart: validating Settings and First Run

How to prove the feature works, per verification surface. Contracts: [core-traits.md](./contracts/core-traits.md), [ipc.md](./contracts/ipc.md). Types: [data-model.md](./data-model.md).

## Prerequisites

```sh
pnpm install && make core-image   # once
```

## 1. Linux host — core with fakes

```sh
scripts/tw-run core -- cargo test -p voicen-core settings     # defaults, validation, save transaction, load/reset, live apply
scripts/tw-run core -- cargo test -p voicen-core i18n         # catalog parity (ids + placeholders), MESSAGE_IDS present, language resolution
scripts/tw-run core -- cargo test -p voicen-core connection   # Test connection vs mock OpenAI-compatible server
scripts/tw-run core -- cargo test -p voicen-core secrets      # Secret redaction; settings file and log capture free of a known key
```

Expected: green. Each failure branch has its own test — refused save leaves file, snapshot, fake hotkey registrar, fake autostart and fake credential store unchanged; unreadable file → backup + defaults + `Reset`; write failure → refusal and undo of keys/autostart/hotkey; Test connection → `CannotReach`, `InvalidKey` (401 and 403), `Timeout` (injected short timeouts), `Http{500}`, `UnexpectedResponse`, `Ok{latency}`.

## 2. Linux host — UI with mocked IPC

```sh
pnpm test -- src/lib/settings          # draft/dirty logic, field-error mapping, t() ids exist in the catalog
pnpm e2e -- e2e/settings-first-run.spec.ts e2e/settings-save.spec.ts e2e/settings-language.spec.ts \
            e2e/settings-test-connection.spec.ts e2e/settings-warning.spec.ts
```

Expected scenarios (mock state in `e2e/support/tauriMock.ts`):
- First run (`first_run: true`): Engine tab active, engine none, hotkey `Ctrl+Alt+Space`, hold, auto-paste on, auto language, history on 20, start with Windows off; selecting API shows `https://api.openai.com/v1` and `whisper-1`, key empty.
- Save refused: the mock returns `Refused` → each named field is highlighted with its localized reason; the draft stays.
- Key field: with `keys.transcription_api = true` the field shows "key saved" and no value; untouched save sends `Untouched`; clear sends `Clear`.
- Language: mock emits `settings://changed` with `ui_language: "ru"` → page text switches to Russian without reload.
- Test connection: button disabled while the mock's promise is pending; each result kind shows its message.
- http warning: `Saved` with an `endpoint.insecure` warning → warning shown; none for loopback.
- Close with unsaved edits → discard dialog; "Keep editing" keeps the window.

## 3. Windows CI runner

`cargo test --workspace` on `windows-latest` runs the shell integration tests:
- Credential Manager round trip per slot (write → read → delete → read none) with a throwaway target prefix.
- Autostart: `set(true)` writes `HKCU\…\Run\Voicen` pointing at the test exe with `--autostart`; `set(false)` removes it; reconcile rewrites a stale path.
- OS language read returns a non-empty BCP-47 tag.
- Silent install + first launch: the log contains `settings load outcome=first_run` and `%LOCALAPPDATA%\Voicen\settings.json` exists with `"engine":"none"` and no key-like value.

## 4. Owner's manual checks (Windows 11; Windows Sandbox for clean installs)

| Check | Steps | Pass |
|---|---|---|
| NFR-10 / SC-001 | Fresh Sandbox, start a timer, run the installer, launch, choose API, paste an OpenAI key, Save, dictate into Notepad | first pasted transcript ≤ 3 min |
| FR-15 Russian first start | Sandbox with Russian display language, fresh install | settings, tray menu, overlay, notifications in Russian |
| FR-19 | Turn on, reboot, log on → tray icon, no window; turn off, reboot → no Voicen | as stated |
| FR-13 live apply | change each setting, save, use it without restart; restart, value kept | all |
| FR-14 real endpoints | Test connection against OpenAI and Groq with good and wrong keys | "OK, n ms" / "invalid API key" |
| NFR-04 | Save a known test key, test, fail a dictation with it; search `%LOCALAPPDATA%\Voicen` (settings, logs, crash files) for it | 0 matches |

Known limitation: if the user disables Voicen in Task Manager → Startup, Windows does not start it even with the option on; the app does not override that choice.
