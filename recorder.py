import io
import logging
import threading
import wave

import pyaudio

logger = logging.getLogger(__name__)

_CHUNK_SIZE = 1024
_STOP_TIMEOUT = 10.0


class Recorder:
    def __init__(self, rate: int = 16000, channels: int = 1):
        self._rate = rate
        self._channels = channels
        self._format = pyaudio.paInt16
        self._sample_width = pyaudio.get_sample_size(pyaudio.paInt16)
        self._chunk = _CHUNK_SIZE
        self._stop_event = threading.Event()
        self._done_event = threading.Event()
        self._audio_data: bytes | None = None

    def start(self) -> None:
        self._stop_event.clear()
        self._done_event.clear()
        t = threading.Thread(target=self._record, daemon=True)
        t.start()

    def stop(self) -> bytes | None:
        self._stop_event.set()
        self._done_event.wait(timeout=_STOP_TIMEOUT)
        return self._audio_data

    def _record(self) -> None:
        p = pyaudio.PyAudio()
        stream = None
        frames = []

        try:
            stream = p.open(
                format=self._format,
                channels=self._channels,
                rate=self._rate,
                input=True,
                frames_per_buffer=self._chunk,
            )
            while not self._stop_event.is_set():
                data = stream.read(self._chunk, exception_on_overflow=False)
                frames.append(data)
        except OSError as e:
            logger.error("Microphone error: %s", e)
            self._audio_data = None
            self._done_event.set()
            return
        finally:
            if stream:
                try:
                    stream.stop_stream()
                    stream.close()
                except Exception as e:
                    logger.warning("Stream close failed: %s", e)
            p.terminate()

        buf = io.BytesIO()
        with wave.open(buf, "wb") as wf:
            wf.setnchannels(self._channels)
            wf.setsampwidth(self._sample_width)
            wf.setframerate(self._rate)
            wf.writeframes(b"".join(frames))

        self._audio_data = buf.getvalue()
        self._done_event.set()
