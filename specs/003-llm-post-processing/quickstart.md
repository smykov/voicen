# Quickstart: validating LLM post-processing

**Feature**: `003-llm-post-processing` · Contracts: [core](contracts/core-post-process.md), [IPC](contracts/ipc.md) · Data: [data-model.md](data-model.md)

## Prerequisites

- `pnpm install && make core-image` (Rust runs only in the `voicen-rust:1.99` image).
- Features 001 (pipeline, shared client, timeouts, notices) and 004 (settings model, key slots, Post-processing tab) are merged up to the points listed in [plan.md](plan.md) › Dependencies.

## 1. Core on the Linux host (fakes and mock server)

```sh
scripts/tw-run core -- cargo test -p voicen-core post_process     # stage: success, every skip reason, trim, auth header
scripts/tw-run core -- cargo test -p voicen-core pipeline          # off → no request; skipped → raw delivered + notice; ordering
scripts/tw-run core -- cargo test -p voicen-core secrets_not_logged # log capture over success + all failure modes
```

Expected results:

| Scenario (spec) | Observation |
|---|---|
| US1-1/2: mock returns `"Привет, как дела?\n"` | delivered text is `Привет, как дела?`; the mock recorded one `POST /chat/completions` with model, system = prompt, user = raw, and the bearer key |
| US1-4: post-processing off | the mock recorded 0 requests; raw delivered |
| US2-1: mock never answers (real 15 s test) | outcome `Skipped(Timeout)` within 15 s ± 0.5 s; raw delivered |
| US2-2: refused port and non-routable host | `Skipped(Unreachable{host})`, with the host from the base URL |
| US2-3/4: mock 401, 403, 404, 429, 500 | `InvalidKey` for 401 and 403; `Http{status}` for the others |
| US2-5: `{}`, `{"choices":[]}`, non-JSON, `"   "` | `InvalidResponse` |
| US2-7: any skip | job outcome delivered; no pending recording |
| US3-2 / SC-005: run all of the above with a log capture | the captured log has `post_process outcome=… duration_ms=…` lines and contains neither the key, the prompt, the raw text, the reply nor the error body |
| FR-23: fake post-processor delays dictation A by 300 ms, B has none | delivery order A then B |

## 2. UI on the Linux host (Playwright, mocked IPC)

```sh
pnpm e2e -- post-processing
```

Expected results: the Post-processing tab shows the starter prompt and "off" from the mocked `get_settings`, and shows the privacy note. Saving with post-processing on and an empty model or prompt is refused with the field highlighted, while the same save with it off succeeds. A mocked `post_processing_skipped` notice with `reason: "http", status: 500` renders "Post-processing skipped — HTTP 500" in the overlay, and the Russian text when the UI language is `ru`.

## 3. Gate

```sh
make check
```

## 4. Windows CI runner

The job builds, installs silently and runs the smoke check. `cargo test --workspace` covers the shell wiring that installs `ChatPostProcessor` and 004's Credential Manager round trip for slot `PostProcessing`. No real LLM is called in CI.

## 5. Owner's manual check (per release, Windows 11)

1. Turn post-processing on with a real chat endpoint (e.g. `https://api.openai.com/v1` with `gpt-4o-mini`, or Ollama `http://localhost:11434/v1`). Dictate "привет как дела" into Notepad. The pasted text is punctuated.
2. Set the base URL to `http://localhost:9` and dictate. The raw text is pasted; the toast and overlay show "Post-processing skipped — cannot reach localhost" for 3 s; the tray shows the error state.
3. Open `%LOCALAPPDATA%\Voicen\logs\voicen.log` and confirm there is a `post_process` line with outcome and duration, and no dictated words, prompt or key.
