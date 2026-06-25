import logging
import os
from datetime import date

logger = logging.getLogger(__name__)


def save_transcription(text: str, history_dir: str) -> None:
    path = os.path.join(history_dir, f"history-{date.today().isoformat()}.txt")
    line = text.replace("\n", " ").strip()
    if not line:
        return
    try:
        with open(path, "a", encoding="utf-8") as f:
            f.write(line + "\n")
    except OSError as e:
        logger.warning("Failed to save history: %s", e)
