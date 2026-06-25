# Transcribator On Tray (ToT)

Голосовая транскрибация через GPU-кластер. Запись по Ctrl+Space, автоматическое копирование в буфер и вставка в активное поле.

## Возможности

- **Ctrl+Space** — запись микрофона, отпускание → транскрибация
- **Clipboard + Auto-paste** — результат сразу в буфере и вставляется в активное окно
- **История** — ежедневные файлы `history-YYYY-MM-DD.txt` с ротацией
- **Settings** — окно настроек (ПКМ по иконке в трее)
- **Автозагрузка** — опция запуска вместе с Windows
- **large-v3** — самая точная модель Whisper

## Быстрый старт

Скачать `voicen.exe` из [releases](../../releases), положить рядом `config.json` и запустить.

```json
{
  "api_url": "http://10.10.10.110:8000/v1/audio/transcriptions",
  "api_key": "your-api-key",
  "language": "ru",
  "model": "large-v3"
}
```

## Сборка из исходников

```bash
pip install -r requirements.txt
pyinstaller --onefile --windowed --name "voicen" --hidden-import pyperclip --noconfirm main.py
```

## Конфигурация

| Поле | Тип | По умолчанию | Описание |
|------|-----|-------------|----------|
| `api_url` | string | `http://10.10.10.110:8000/v1/audio/transcriptions` | URL Whisper API |
| `api_key` | string | `""` | API ключ (опционально) |
| `language` | string | `"ru"` | Язык (`ru`, `en`, `auto`) |
| `model` | string | `"large-v3"` | Модель Whisper (`large-v3`, `turbo`) |
| `sample_rate` | int | `16000` | Частота дискретизации |
| `channels` | int | `1` | Количество каналов |
| `transcription_timeout` | int | `120` | Таймаут запроса (сек) |
| `auto_paste` | bool | `true` | Автовставка Ctrl+V |
| `save_history` | bool | `true` | Сохранение истории |
| `auto_start` | bool | `false` | Автозагрузка с Windows |

## Архитектура

```
main.py                 — точка входа, DI-композиция
app.py                  — бизнес-логика (state machine)
tray.py                 — системный трей
settings_window.py      — окно настроек (tkinter)
transcriber.py          — HTTP-клиент Whisper API
recorder.py             — запись аудио (PyAudio)
hotkey.py               — глобальный хук Ctrl+Space
history.py              — сохранение истории
autostart.py            — автозагрузка Windows
config.py               — загрузка/сохранение config.json
icons.py                — генерация иконок трея
state.py                — enum состояний
```

## Стек

- Python 3.12 + PyInstaller 6.21
- PyAudio, pystray, Pillow, keyboard, requests, pyperclip
- Backend: Whisper (large-v3/turbo) на RTX 3090

## Лицензия

MIT
