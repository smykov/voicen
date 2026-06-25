import logging
import threading
from typing import Callable, Optional, Protocol

from state import AppState
from config import AppConfig
from transcriber import TranscriptionError, TranscriptionParams

logger = logging.getLogger(__name__)

StateChangeCallback = Callable[[AppState], None]
NotificationCallback = Callable[[str, str], None]


class RecorderProtocol(Protocol):
    def start(self) -> None: ...
    def stop(self) -> bytes | None: ...


class TranscriberFunc(Protocol):
    def __call__(self, audio_data: bytes, params: TranscriptionParams) -> str: ...


_TRUNCATE_MAX = 120
_DONE_DELAY = 2.0
_ERROR_DELAY = 3.0


def _truncate(text: str, max_len: int = _TRUNCATE_MAX) -> str:
    return text[:max_len] + ("…" if len(text) > max_len else "")


class App:
    _MIN_AUDIO_LENGTH = 100

    def __init__(
        self,
        config: AppConfig,
        recorder: RecorderProtocol,
        transcriber_func: TranscriberFunc,
        clipboard_func: Callable[[str], None],
        on_state_change: Optional[StateChangeCallback] = None,
        on_notification: Optional[NotificationCallback] = None,
        on_transcribed: Optional[Callable[[str], None]] = None,
        on_paste: Optional[Callable[[], None]] = None,
    ) -> None:
        self._config = config
        self._recorder = recorder
        self._transcriber = transcriber_func
        self._clipboard = clipboard_func
        self._on_state_change = on_state_change
        self._on_notification = on_notification
        self._on_transcribed = on_transcribed
        self._on_paste = on_paste

        self._state = AppState.IDLE
        self._lock = threading.Lock()
        self._idle_timer: Optional[threading.Timer] = None

    # -- Public API --

    def start_recording(self) -> bool:
        with self._lock:
            if self._state != AppState.IDLE:
                return False
            self._state = AppState.RECORDING
            self._cancel_idle_timer()

        self._notify("Recording", "Recording… release Ctrl+Space to transcribe")
        self._emit_state(AppState.RECORDING)

        self._recorder.start()
        return True

    def stop_recording(self) -> bool:
        with self._lock:
            if self._state != AppState.RECORDING:
                return False
            self._state = AppState.TRANSCRIBING

        self._emit_state(AppState.TRANSCRIBING)

        t = threading.Thread(target=self._process_completed_recording, daemon=True)
        t.start()
        return True

    @property
    def state(self) -> AppState:
        with self._lock:
            return self._state

    def shutdown(self) -> None:
        with self._lock:
            self._cancel_idle_timer()

    # -- Internal --

    def _process_completed_recording(self) -> None:
        audio = self._recorder.stop()

        if audio is None or len(audio) < self._MIN_AUDIO_LENGTH:
            self._set_error("No audio recorded")
            return

        params = TranscriptionParams(
            api_url=self._config.api_url,
            language=self._config.language,
            model=self._config.model,
            api_key=self._config.api_key,
            timeout=self._config.transcription_timeout,
        )

        try:
            text = self._transcriber(audio_data=audio, params=params)
        except TranscriptionError as e:
            logger.exception("Transcription failed")
            self._set_error(str(e)[:_TRUNCATE_MAX])
            return

        if not text:
            self._set_error("Empty transcription result")
            return

        self._handle_transcription_result(text)

    def _handle_transcription_result(self, text: str) -> None:
        if self._on_transcribed:
            self._on_transcribed(text)

        try:
            self._clipboard(text)
            title = "Copied"
        except Exception as e:
            logger.warning("Clipboard copy failed: %s", e)
            title = "Result"

        if title == "Copied" and self._on_paste:
            self._on_paste()

        self._notify(title, _truncate(text))
        self._set_idle_after(AppState.DONE, delay=_DONE_DELAY)

    def _set_error(self, message: str, delay: float = _ERROR_DELAY) -> None:
        logger.error(message)
        self._notify("Error", message)
        self._set_idle_after(AppState.ERROR, delay=delay)

    def _set_idle_after(self, state: AppState, delay: float) -> None:
        with self._lock:
            self._state = state
            self._cancel_idle_timer()
            self._idle_timer = threading.Timer(delay, self._reset_to_idle)
            self._idle_timer.daemon = True
            self._idle_timer.start()
        self._emit_state(state)

    def _reset_to_idle(self) -> None:
        with self._lock:
            if self._state not in (AppState.DONE, AppState.ERROR):
                return
            self._state = AppState.IDLE
            self._idle_timer = None
        self._emit_state(AppState.IDLE)

    def _cancel_idle_timer(self) -> None:
        if self._idle_timer:
            self._idle_timer.cancel()
            self._idle_timer = None

    def _emit_state(self, state: AppState) -> None:
        if self._on_state_change:
            self._on_state_change(state)

    def _notify(self, title: str, message: str) -> None:
        logger.info("%s: %s", title, message)
        if self._on_notification:
            self._on_notification(title, message)
