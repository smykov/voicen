import logging
import threading
from typing import Callable, Optional

import keyboard

logger = logging.getLogger(__name__)


class HotkeyManager:
    def __init__(self):
        self._hook: Optional[Callable[[object], None]] = None
        self._ctrl_pressed = False
        self._combo_active = False
        self._on_activate: Optional[Callable[[], None]] = None
        self._on_deactivate: Optional[Callable[[], None]] = None
        self._lock = threading.Lock()

    def start(
        self,
        on_activate: Callable[[], None],
        on_deactivate: Callable[[], None],
    ) -> None:
        self._on_activate = on_activate
        self._on_deactivate = on_deactivate
        self._hook = keyboard.hook(self._handle_event, suppress=False)

    def stop(self) -> None:
        if self._hook:
            try:
                keyboard.unhook(self._hook)
            except Exception as e:
                logger.warning("Failed to unhook keyboard: %s", e)
            self._hook = None

    def _handle_event(self, event) -> None:
        name = (event.name or "").lower()

        if name in ("ctrl", "left ctrl", "right ctrl", "control"):
            with self._lock:
                released = self._ctrl_pressed and event.event_type == "up"
                self._ctrl_pressed = event.event_type == "down"
            if released and self._combo_active:
                self._combo_active = False
                self._safe_call(self._on_deactivate)
            return

        if name == "space":
            with self._lock:
                ctrl_held = self._ctrl_pressed

            if event.event_type == "down" and ctrl_held:
                self._combo_active = True
                self._safe_call(self._on_activate)
            elif event.event_type == "up" and self._combo_active:
                self._combo_active = False
                self._safe_call(self._on_deactivate)
            return

    @staticmethod
    def _safe_call(fn: Optional[Callable[[], None]]) -> None:
        if fn:
            try:
                fn()
            except Exception as e:
                logger.exception("Callback error: %s", e)
