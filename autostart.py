import logging
import os
import subprocess
import sys

logger = logging.getLogger(__name__)

_SHORTCUT_NAME = "WhisperTranscribe.lnk"


def _startup_dir() -> str:
    return os.path.join(
        os.environ["APPDATA"],
        "Microsoft",
        "Windows",
        "Start Menu",
        "Programs",
        "Startup",
    )


def _shortcut_path() -> str:
    return os.path.join(_startup_dir(), _SHORTCUT_NAME)


def enable() -> None:
    exe = sys.executable if not getattr(sys, "frozen", False) else sys.executable
    if not os.path.isfile(exe):
        logger.warning("Cannot enable autostart: exe not found at %s", exe)
        return

    ps = (
        f'$ws = New-Object -ComObject WScript.Shell; '
        f'$sc = $ws.CreateShortcut("{_shortcut_path()}"); '
        f'$sc.TargetPath = "{exe}"; '
        f'$sc.Save()'
    )
    try:
        subprocess.run(
            ["powershell", "-NoProfile", "-Command", ps],
            check=True,
            capture_output=True,
            timeout=15,
        )
        logger.info("Autostart enabled: %s", _shortcut_path())
    except Exception as e:
        logger.warning("Failed to enable autostart: %s", e)


def disable() -> None:
    path = _shortcut_path()
    try:
        if os.path.isfile(path):
            os.remove(path)
            logger.info("Autostart disabled: %s", path)
    except OSError as e:
        logger.warning("Failed to disable autostart: %s", e)


def is_enabled() -> bool:
    return os.path.isfile(_shortcut_path())
