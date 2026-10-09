# Contract: IPC surface touched by post-processing

**Feature**: `003-llm-post-processing`

This feature adds no new IPC command. It adds fields and message keys to the commands and events owned by 004 (settings) and 001 (notices and overlay). The e2e tests mock exactly these shapes through `window.__TAURI_INTERNALS__`.

## Settings commands (owned by 004): `post_processing` fragment

In `Settings` (004's `SettingsView.settings` from `settings_get`, and `SaveRequest.settings` of `settings_save`):

```json
"post_processing": {
  "enabled": false,
  "base_url": "",
  "model": "",
  "prompt": "Correct punctuation, capitalization and …"
}
```

- Whether a key is stored is `SettingsView.keys.post_processing` (read-only, from the credential store). The key itself is never in a DTO sent to the UI (004 FR-014).
- A key is sent in `settings_save` through 004's `KeyEdits`, slot `post_processing`: `"Untouched"` keeps it, `"Clear"` deletes it, `{ "Replace": "<key>" }` stores a new one.
- Validation (`post_process::settings::validate`, called by 004's settings validation for every engine) runs only while `enabled` is true. Errors come back in 004's `Refused` shape, all at once, in the order `post_processing.base_url` (`required` | `url.malformed` | `url.credentials`, the same rule as the engine URLs), `post_processing.model` (`required`, after trim), `post_processing.prompt` (`required`, after trim) (FR-011). A key is never required. While `enabled` is false no post-processing field is refused.
- The toggle's field id is `post_processing.enabled` (a control `data-field` and label; never the target of an error).

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
| `notice.post_processing_skipped.not_configured` | Post-processing skipped — not set up. Check the Post-processing settings. | Постобработка пропущена — не настроена. Проверьте настройки постобработки. |
| `settings.post_processing.privacy_note` | When post-processing is on, the transcript text is sent to this endpoint. | Когда постобработка включена, текст расшифровки отправляется на этот адрес. |
