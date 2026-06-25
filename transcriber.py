import logging
from dataclasses import dataclass

import requests

logger = logging.getLogger(__name__)


class TranscriptionError(Exception):
    pass


@dataclass(frozen=True)
class TranscriptionParams:
    api_url: str
    language: str
    model: str
    api_key: str
    timeout: int


def transcribe(
    audio_data: bytes,
    params: TranscriptionParams,
) -> str:
    headers = {}
    if params.api_key:
        headers["Authorization"] = f"Bearer {params.api_key}"

    files = {"file": ("audio.wav", audio_data, "audio/wav")}
    data = {"language": params.language, "model": params.model}

    try:
        resp = requests.post(
            params.api_url,
            headers=headers,
            files=files,
            data=data,
            timeout=params.timeout,
        )
        resp.raise_for_status()
        result = resp.json()
        text = result.get("text", "")
        if not text:
            raise TranscriptionError("API returned empty text")
        return text
    except requests.ConnectionError as e:
        raise TranscriptionError(f"Cannot reach API: {e}") from e
    except requests.Timeout as e:
        raise TranscriptionError(f"API timed out after {params.timeout}s") from e
    except requests.HTTPError as e:
        raise TranscriptionError(f"API error {e.response.status_code}: {e.response.text[:100]}") from e
    except requests.RequestException as e:
        raise TranscriptionError(f"Request failed: {e}") from e
