# Contract: IPC surface touched by post-processing

**Feature**: `003-llm-post-processing`

This feature adds no new IPC command. It adds fields and message keys to the commands and events owned by 004 (settings) and 001 (notices and overlay). The e2e tests mock exactly these shapes through `window.__TAURI_INTERNALS__`.

## Settings commands (owned by 004): `post_processing` fragment

In the settings DTO returned by 004's `get_settings` and accepted by its `save_settings`:

```json
"post_processing": {
  "enabled": false,
  "base_url": "",
  "model": "",
  "prompt": "Correct punctuation, capitalization and …",
  "has_key": false
}
```

- `has_key` is read-only, from the credential store. The key itself is never in the DTO sent to the UI (004 FR-014).
- A new key is sent in `save_settings` through 004's key-slot input, slot `"post_processing"`. An absent value means unchanged, and an empty string means delete.
- Validation errors come back in 004's error shape with the field ids `post_processing.base_url`, `post_processing.model` and `post_processing.prompt` (FR-011).

## Notice event (owned by 001): skipped payload

The overlay and toast receive 001's notice event with:

```json
{ "kind": "post_processing_skipped",
  "reason": "timeout" | "unreachable" | "invalid_key" | "http" | "invalid_response",
  "params": { "host": "api.example.com" } | { "status": 500 } | {} }
```

The UI resolves `notice.post_processing_skipped.<reason>` from 004's catalog in the current UI language.

## Message catalog entries (added to 004's catalog)

| Key | en | ru |
|---|---|---|
| `notice.post_processing_skipped.timeout` | Post-processing skipped — timeout | Постобработка пропущена — превышено время ожидания |
| `notice.post_processing_skipped.unreachable` | Post-processing skipped — cannot reach {host} | Постобработка пропущена — нет связи с {host} |
| `notice.post_processing_skipped.invalid_key` | Post-processing skipped — invalid API key | Постобработка пропущена — неверный ключ API |
| `notice.post_processing_skipped.http` | Post-processing skipped — HTTP {status} | Постобработка пропущена — HTTP {status} |
| `notice.post_processing_skipped.invalid_response` | Post-processing skipped — empty or invalid response | Постобработка пропущена — пустой или неверный ответ |
| `settings.post_processing.privacy_note` | When post-processing is on, the transcript text is sent to this endpoint. | Когда постобработка включена, текст расшифровки отправляется на этот адрес. |
