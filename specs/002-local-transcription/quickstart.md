# Quickstart: validating Local Transcription

Interfaces: [contracts/core-traits.md](contracts/core-traits.md), [contracts/ipc.md](contracts/ipc.md). States: [data-model.md](data-model.md).

## Prerequisites

```sh
pnpm install && make core-image      # Linux dev host
```

001's engine trait, OpenAI-compatible client, timeouts module and pipeline must exist; 004's settings window hosts the UI component.

## 1. Linux host — core with fakes (gate)

```sh
make check                                                          # full gate
scripts/tw-run core -- cargo test -p voicen-core local_models       # catalog, store, download, residency
scripts/tw-run core -- cargo test -p voicen-core engines::builtin   # built-in engine with fake SpeechModel
scripts/tw-run core -- cargo test -p voicen-core engines::local_server
```

Expected:
- Catalog: five entries, one recommended (`small`), pinned commit shared.
- Download vs mock server: progress events; final file only after the hash matches; truncated, altered, stalled (short no-data timeout), HTTP 500, refused connection → no file left, `Failed{reason}`; cancel → `NotDownloaded`, no file; second start → `Busy`; low disk → refused.
- Start-up: `*.part` deleted; wrong-size final file → not downloaded.
- Residency (fake clock): not loaded at start; prewarm on recording start; second dictation `Warm`; unload at exactly 10 min idle, not during activity; unload on engine/model change and before delete.
- Built-in engine: no model → `NoLocalModel`, audio pending; load/transcribe error → engine failure; request-recording network fake sees **zero** requests.
- Local server vs mock OpenAI-compatible server: no `Authorization` header without key; bearer with key; model field only when set; stopped server → "cannot reach 127.0.0.1:<port>"; stalled response → "timeout" (injected short value; production value asserted = 60 s).
- Delete: file gone; selected → engine `None` persisted; in use → refused; remove error → still downloaded.

## 2. Linux host — UI with mocked IPC

```sh
pnpm test -- src/lib/local-models
pnpm e2e -- e2e/local-models.spec.ts
```

Expected: five rows with sizes and the recommended badge; Download shows a progress bar driven by mocked `local-model://progress`; mocked `failed` state shows the reason and Retry; Cancel returns the row to Download; other Download buttons disabled while one runs; Delete asks for confirmation and, with `engineReset: true`, the engine shows "none".

## 3. Windows CI runner

`cargo test --workspace` (in `.github/workflows/ci.yml`):
- `voicen-whisper` transcribes the bundled WAV with the cached `tiny` model (fixed language and auto-detect) → non-empty text with the expected word.
- On a cache miss, `tiny` is fetched with the app's downloader and its pinned hash is verified against Hugging Face.
- Shell: `local-server` key read from Credential Manager; delete of a model file after load/unload succeeds.

## 4. Owner's manual check (Windows 11 PC, installer from CI)

1. Settings → Engine → built-in local → download `small`; watch progress; select it.
2. Turn off the network; dictate into Notepad → text pasted (req FR-07). Log line shows `local=cold` then `local=warm`.
3. Wait 10 minutes idle → Task Manager shows the RAM drop (req NFR-03).
4. Run the NFR-01 20-phrase benchmark with `small` warm → p90 stop→paste ≤ 10 s.
5. Start speaches at `http://localhost:8000/v1`, engine = local server, no key → text pasted; stop the server → "cannot reach localhost:8000", Retry from the tray after restarting it works (req FR-17).
6. Delete `small` while selected → file gone from `%LOCALAPPDATA%\Voicen\models`, engine "none" (req FR-28).
