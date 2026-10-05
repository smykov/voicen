# Contract: user-visible messages of this feature (FR-034)

Stable keys with English and Russian texts. `{x}` marks a parameter. The catalog is the one `i18n/en.json`, `i18n/ru.json` at the repository root (teamwright T-005, decisions #13); add these keys there. `MessageKey` maps to `voicen_core::i18n::MessageId`, except the UI-only ids `overlay.recording` and `overlay.processing` (T-053): the overlay page renders them with `t`, and they are not declared in `messages!`. Parity (every key in both languages, non-empty, same placeholders) is checked only by T-005's tests.

| Key | en | ru | Where |
|---|---|---|---|
| `failure.invalid_api_key` | Invalid API key | Неверный API-ключ | toast + overlay |
| `failure.network_unavailable` | Network unavailable | Сеть недоступна | toast + overlay |
| `failure.cannot_reach` | Cannot reach {host} | Не удаётся подключиться к {host} | toast + overlay |
| `failure.timeout` | The server did not answer in time | Сервер не ответил вовремя | toast + overlay |
| `failure.server_error` | Server error (HTTP {status}) | Ошибка сервера (HTTP {status}) | toast + overlay |
| `failure.unexpected_response` | Unexpected response from the server | Неожиданный ответ сервера | toast + overlay |
| `failure.key_store_unavailable` | The API key could not be read from Windows Credential Manager. | Не удалось прочитать API-ключ из диспетчера учётных данных Windows. | toast + overlay (decision #44) |
| `failure.engine_not_configured` | The transcription engine is not set up. Check the Engine settings. | Движок распознавания не настроен. Проверьте настройки движка. | toast + overlay (decision #44) |
| `failure.clipboard_unavailable` | Clipboard unavailable | Буфер обмена недоступен | toast + overlay |
| `failure.microphone_unavailable` | Microphone unavailable: {reason} | Микрофон недоступен: {reason} | toast + overlay |
| `failure.hotkey_unavailable` | Hotkey unavailable | Сочетание клавиш недоступно | toast + overlay + settings |
| `mic_reason.no_device` | no input device | нет устройства ввода | parameter |
| `mic_reason.access_denied` | access denied in Windows privacy settings | доступ запрещён в настройках конфиденциальности Windows | parameter |
| `mic_reason.busy` | the device is busy | устройство занято | parameter |
| `mic_reason.other` | the device could not be opened | не удалось открыть устройство | parameter |
| `action.retry` | Retry | Повторить | toast button |
| `notice.no_speech` | No speech detected | Речь не распознана | toast + overlay |
| `notice.max_length` | Maximum length reached | Достигнута максимальная длительность | toast + overlay |
| `notice.copied` | Copied to clipboard | Скопировано в буфер обмена | toast + overlay |
| `notice.copied_paste_manually` | Copied — paste manually | Скопировано — вставьте вручную | toast + overlay |
| `notice.mic_fallback` | Using {device} | Используется {device} | toast |
| `notice.choose_engine` | Choose a transcription engine | Выберите движок распознавания | toast + overlay (T-051; decision #64) |
| `notice.hotkey_failed_startup` | Hotkey {hotkey} could not be registered — choose another one | Не удалось зарегистрировать {hotkey} — выберите другое сочетание | toast |
| `overlay.recording` | Recording {elapsed} | Запись {elapsed} | overlay |
| `overlay.processing` | Transcribing… | Распознавание… | overlay |
| `tray.settings` | Settings | Настройки | tray menu |
| `tray.history` | History | История | tray menu |
| `tray.open_logs` | Open logs folder | Открыть папку журналов | tray menu |
| `tray.retry_last` | Retry last failed dictation | Повторить последнюю неудачную диктовку | tray menu |
| `tray.exit` | Exit | Выход | tray menu |
| `tray.tooltip.idle` | Voicen | Voicen | tray tooltip |
| `tray.tooltip.recording` | Voicen — recording | Voicen — запись | tray tooltip |
| `tray.tooltip.error` | Voicen — last dictation failed | Voicen — последняя диктовка не удалась | tray tooltip |
| `tray.tooltip.hotkey_error` | Voicen — hotkey not registered | Voicen — сочетание клавиш не зарегистрировано | tray tooltip |

The failure toasts for retryable reasons carry the `action.retry` button. The requirement text "network unavailable — Retry" (req FR-11) is rendered as the `failure.network_unavailable` text plus the Retry button.

In the catalog and declared with `messages!` (`voicen_core::i18n`, `FailureReason::message_id`) since T-040: `failure.invalid_api_key`, `failure.network_unavailable`, `failure.cannot_reach`, `failure.timeout`, `failure.server_error`, `failure.unexpected_response`, `failure.key_store_unavailable`, `failure.engine_not_configured`. Since T-042 (`FailureReason::MicrophoneUnavailable`): `failure.microphone_unavailable` and, in the `nested:` group of `messages!`, `mic_reason.no_device`, `mic_reason.access_denied`, `mic_reason.busy`, `mic_reason.other`; `message_params()` puts the cause's `mic_reason.*` id into `{reason}`, and Rust `i18n::text` renders it in the same language. Since T-001: `failure.clipboard_unavailable` (`FailureReason::ClipboardUnavailable`), `notice.no_speech` (`JobEnd::Notice` of a job without speech), `notice.copied` and `notice.copied_paste_manually` (`DeliveryResult::notice()` for `CopiedOnly` / `CopyManual`). Since T-051, `notice.choose_engine` (already in the catalog) is also shown on the overlay: a press blocked by engine = none raises it with `RecordingController::notice` for 3 s before settings open on the Engine tab; its toast comes with T-007's notifier. Since T-052 (`voicen_core::tray`: `TrayAction::label`, `tooltip`): `tray.settings`, `tray.exit`, `tray.tooltip.idle`, `tray.tooltip.recording`, `tray.tooltip.error`, `tray.tooltip.hotkey_error`, rendered by the shell's tray with `i18n::text` in the settings' `ui_language` and re-rendered on a saved language change. Since T-053, `overlay.recording` and `overlay.processing` are in the catalog as UI-only ids: the overlay page (`src/routes/overlay/+page.svelte`) renders them with `t` in the payload's `lang`, and they are not declared in `messages!`; every other overlay text is a core message, rendered by Rust `i18n::text` before it is sent (contracts/ipc.md). The other keys are added by the tasks that first use them (`tray.retry_last` T-007, `tray.open_logs` T-054, `tray.history` feature 005).
