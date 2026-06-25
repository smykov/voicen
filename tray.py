import logging
from typing import Callable

import pystray

from state import AppState
from icons import get_icon
from settings_window import SettingsWindow
from config import load_config, get_config_dir

logger = logging.getLogger(__name__)


class TrayController:
    def __init__(self):
        self._icon: pystray.Icon | None = None

    def create_icon(self, on_exit: Callable[[], None]) -> pystray.Icon:
        menu = pystray.Menu(
            pystray.MenuItem("Transcriber In Tray (TiT)", None, enabled=False),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Settings", self._open_settings),
            pystray.MenuItem("Exit", lambda: on_exit()),
        )
        self._icon = pystray.Icon(
            "voicen",
            get_icon(AppState.IDLE),
            "Transcriber In Tray — Ctrl+Space to record",
            menu=menu,
        )
        return self._icon

    def stop(self) -> None:
        if self._icon:
            try:
                self._icon.stop()
            except Exception as e:
                logger.warning("Icon stop failed: %s", e)

    def update_icon(self, state: AppState) -> None:
        if self._icon:
            self._icon.icon = get_icon(state)

    def notify(self, title: str, message: str) -> None:
        if self._icon:
            try:
                self._icon.notify(message, title)
            except Exception as e:
                logger.warning("Notification failed: %s", e)

    def _open_settings(self) -> None:
        cfg = load_config()
        SettingsWindow.open_or_focus(config=cfg, history_dir=get_config_dir())
