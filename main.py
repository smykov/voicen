import logging

from config import load_config, get_config_dir
from app import App
from tray import TrayController
from recorder import Recorder
from transcriber import transcribe
from hotkey import HotkeyManager
from history import save_transcription

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    datefmt="%H:%M:%S",
)
logger = logging.getLogger(__name__)


def _copy(text: str) -> None:
    import pyperclip
    pyperclip.copy(text)


def _paste() -> None:
    import keyboard
    keyboard.press_and_release("ctrl+v")


def main() -> None:
    config = load_config()
    config_dir = get_config_dir()

    recorder = Recorder(
        rate=config.sample_rate,
        channels=config.channels,
    )

    tray = TrayController()
    icon = tray.create_icon(on_exit=tray.stop)

    app = App(
        config=config,
        recorder=recorder,
        transcriber_func=transcribe,
        clipboard_func=_copy,
        on_state_change=tray.update_icon,
        on_notification=tray.notify,
        on_transcribed=(lambda t: save_transcription(t, config_dir)) if config.save_history else None,
        on_paste=_paste if config.auto_paste else None,
    )

    hotkey = HotkeyManager()
    hotkey.start(
        on_activate=app.start_recording,
        on_deactivate=app.stop_recording,
    )

    try:
        icon.run()
    finally:
        hotkey.stop()
        app.shutdown()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
