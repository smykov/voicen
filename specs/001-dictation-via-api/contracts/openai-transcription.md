# Contract: OpenAI-compatible transcription request (FR-017–FR-020)

The one HTTP client used for the API engine (req FR-06), and reused by 002 for the local server (req FR-17).

## Request

```http
POST {base_url_trimmed}/audio/transcriptions
Authorization: Bearer {api_key}          ; omitted when no key is configured
Content-Type: multipart/form-data; boundary=…

file            = audio.wav  (audio/wav; RIFF PCM 16-bit, 16 000 Hz, mono)
model           = {model}
language        = {iso-639-1}            ; omitted when language = auto
response_format = json
```

- `base_url_trimmed` is the configured base URL with trailing `/` removed. For example, `https://api.openai.com/v1/` and `https://api.openai.com/v1` give the same path.
- Size: ≤ 19.2 MB at the 10-minute cap (decisions #1).
- Timeouts (`Timeouts`, the single source): connect 5 s; the whole request 30 s (API) / 60 s (local server, 002) (Clarification 1).

## Response

| Response | Result |
|---|---|
| 2xx, JSON object with string `text`, non-empty after trim | `Ok(text)` (the text is not trimmed beyond what the server sent, except the leading/trailing whitespace) |
| 2xx, `text` empty / whitespace | `Ok("")` → pipeline `NoSpeech` |
| 2xx, body not JSON, `text` missing or not a string, body > 1 MiB, invalid UTF-8, connection reset while reading | `Err(UnexpectedResponse)` |
| 401, 403 | `Err(InvalidApiKey)` |
| any other non-2xx (e.g. 400, 404, 413, 429, 500, 503) | `Err(ServerError{status})` |
| DNS failure; OS "network unreachable" / "host unreachable" | `Err(NetworkUnavailable)` |
| connection refused; connect not established in 5 s | `Err(CannotReach{host})` (`host[:port]` of the base URL) |
| whole request > 30 s | `Err(Timeout)` |

There are no automatic retries (spec edge case). The error body is never read into an error or a log beyond the status code (P-009). Known compatible services: OpenAI (`https://api.openai.com/v1`, `whisper-1`) and Groq (`https://api.groq.com/openai/v1`, `whisper-large-v3-turbo`).

## Test oracle (wiremock, Linux)

One test per row above, plus:

- the trailing-slash path
- no `language` part for auto
- no `Authorization` header without a key
- a key and a transcript placed in the mock response never appear in `FailureReason`'s `Display`/`Debug` or in any `DictationEvent`
