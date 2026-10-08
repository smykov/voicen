# Contract: OpenAI-compatible transcription request (FR-017–FR-020)

The one HTTP client used for the API engine (req FR-06), and reused by 002 for the local server (req FR-17).

## Request

```http
POST {base_url_trimmed}/audio/transcriptions   ; or {base_url_trimmed} when it already ends in /audio/transcriptions (decision #96)
Authorization: Bearer {api_key}          ; omitted when no key is configured
Content-Type: multipart/form-data; boundary=…

file            = audio.wav  (audio/wav; RIFF PCM 16-bit, 16 000 Hz, mono)
model           = {model}                ; omitted when the local-server model is empty (002)
language        = {iso-639-1}            ; omitted when language = auto
response_format = json
```

- `base_url_trimmed` is the configured base URL with trailing `/` removed. For example, `https://api.openai.com/v1/` and `https://api.openai.com/v1` give the same path.
- URL join (`engine::openai::transcription_url`, the only builder of the request URL for the API engine, the local server and Test connection): on the parsed stored URL, never by string concatenation or `Url::join`. One empty trailing path segment is dropped. Then, if the last two path segments equal `audio`, `transcriptions` (whole segments, ASCII case-insensitive), the path is kept as typed: the user gave the full endpoint (decision #96), e.g. `http://10.10.10.110:8000/v1/audio/transcriptions` and `…/v1/audio/transcriptions/` both post to `…/v1/audio/transcriptions`, and `…/v1/Audio/Transcriptions` is sent in that case. Otherwise `audio`, `transcriptions` are appended. Near misses are bases and get the suffix: `/v1/audio`, `/v1/transcriptions`, `/v1/xaudio/transcriptions`, `/v1/audio%2Ftranscriptions`, `/audio/transcriptions/x`. A doubled path (`…/audio/transcriptions/audio/transcriptions`) is kept as is, never repaired, so a wrong path still gets the server's 404. The query string is kept after the path in both branches (decision #27(2)): `https://api.example.com/v1?api-version=2024-06-01` → `https://api.example.com/v1/audio/transcriptions?api-version=2024-06-01`; a root base gives `/audio/transcriptions`. The stored setting is never rewritten (stored as typed, `docs/decisions/settings.md`).
- `Authorization`: one rule for a usable key: the text `Bearer <key>` passes `HeaderValue` validation (http 1.5: no control byte other than tab, no DEL). A key that fails it is `InvalidApiKey`, checked before the client or the request is built, so no request is sent (`TransportError::UnusableKey`); never `CannotReach`. The same rule covers non-ASCII keys: their UTF-8 bytes pass it and are sent as given, and the server's 401/403 gives `InvalidApiKey`. The value is marked sensitive.
- Size: ≤ 19.2 MB at the 10-minute cap (decisions #1). The multipart body is encoded into one buffer before sending (not streamed): reqwest's blocking client reports a failed connect of a streamed body as a body-channel error without its connect flag. The buffer is reserved once from the WAV length plus 1 KiB of framing and the text parts, so it is never regrown; the WAV moves into the form without a copy. Transient peak per call: the audio, the WAV and the body, about 3 × 19.2 MB at the cap.
- `model`: the API engine always sends its model. The local-server engine (002, T-018) sends `local_server.model` trimmed, and omits the part when it is empty or whitespace-only (an optional part is never sent empty); `engine_for` decides this once at construction.
- Timeouts (`Timeouts`, the single source, read from the `TranscribeRequest` of each call): connect 5 s (`connect_timeout` of the client, built per call); the whole request, connect to the last body byte, as the per-request timeout (Clarification 1):

  | Engine role | `kind()` | Whole-request deadline |
  |---|---|---|
  | API | `api` | 30 s (`Timeouts::api_transcription`) |
  | Local server (002) | `local_server` | 60 s (`Timeouts::local_server`) |
- The client is reqwest's blocking client (decision #42): `transcribe` must not run inside a tokio runtime.

## Response

| Response | Result |
|---|---|
| 2xx, JSON object with string `text`, non-empty after trim | `Ok(text)` (the text is not trimmed beyond what the server sent, except the leading/trailing whitespace) |
| 2xx, `text` empty / whitespace | `Ok("")` → pipeline `NoSpeech` |
| 2xx, body not JSON, `text` missing or not a string, body > 1 MiB, invalid UTF-8, connection reset while reading | `Err(UnexpectedResponse)` |
| 401, 403; or a key that fails the `Authorization` rule (no request sent) | `Err(InvalidApiKey)` |
| any other non-2xx (e.g. 400, 404, 413, 429, 500, 503) | `Err(ServerError{status})` |
| DNS failure; OS "network unreachable" / "host unreachable" | `Err(NetworkUnavailable)` |
| connection refused; connect not established in 5 s | `Err(CannotReach{host})` (`host[:port]` of the base URL) |
| whole request > 30 s (API) / 60 s (local server) | `Err(Timeout)` |

Classification (`failure::classify`, one mapping for every transport failure; T-040 Investigation probe, reqwest 0.13.5), first match:

1. HTTP status: 401/403 → `InvalidApiKey`; any other non-2xx → `ServerError{status}`. The error body is not read. A key that fails the `Authorization` rule above → `InvalidApiKey` (no request).
2. DNS failure (`is_dns`), or `NetworkUnreachable`/`HostUnreachable` as the first `io::ErrorKind` in the error's source chain → `NetworkUnavailable`. Checked before 3, because a DNS failure is also `is_connect`.
3. `is_connect` (refused, connect timeout, TLS handshake), or the HTTP client cannot be built → `CannotReach{host}`. Checked before 4, because a connect timeout is also `is_timeout`.
4. `is_timeout` → `Timeout`; also a body read that stalls past the deadline (the blocking reader's `io::Error` wraps a `reqwest::Error` with `is_timeout`).
5. Body errors → `UnexpectedResponse`: a read reset or closed mid-body, a body over 1 MiB (read through a 1 MiB + 1 cap; exactly 1 MiB is accepted), and a body that is not a JSON object with a string `text` (parsed with `serde_json`, so invalid UTF-8 is refused); also a send error with none of the flags above.

The `Display` of every reqwest send error contains the request URL with its query, so nothing of a `reqwest::Error`, the URL, a body or the key reaches a `FailureReason` (P-009): the adapter keeps only the flags and the `io::ErrorKind`.

There are no automatic retries (spec edge case). The error body is never read into an error or a log beyond the status code (P-009). Known compatible services: OpenAI (`https://api.openai.com/v1`, `whisper-1`) and Groq (`https://api.groq.com/openai/v1`, `whisper-large-v3-turbo`).

## Test oracle (wiremock, Linux)

One test per row above, plus:

- the trailing-slash path
- no `language` part for auto
- no `Authorization` header without a key
- a key with an inner control character (`\n`, `\r`, NUL, `\x01`, DEL) is `InvalidApiKey` and no request reaches the server, directly and through `engine_for`
- a key and a transcript placed in the mock response never appear in `FailureReason`'s `Display`/`Debug` or in any `DictationEvent`
