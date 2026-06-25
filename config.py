import json
import os
import sys
from dataclasses import dataclass, asdict


@dataclass
class AppConfig:
    api_url: str = "http://10.10.10.110:8000/v1/audio/transcriptions"
    api_key: str = ""
    language: str = "ru"
    model: str = "large-v3"
    sample_rate: int = 16000
    channels: int = 1
    transcription_timeout: int = 120
    auto_paste: bool = True
    save_history: bool = True
    auto_start: bool = False


def _config_dir() -> str:
    if getattr(sys, "frozen", False):
        return os.path.dirname(sys.executable)
    return os.path.dirname(os.path.abspath(__file__))


def _config_path() -> str:
    return os.path.join(_config_dir(), "config.json")


def get_config_dir() -> str:
    return _config_dir()


def load_config() -> AppConfig:
    path = _config_path()
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            overrides = json.load(f)
        defaults = asdict(AppConfig())
        merged = {**defaults, **overrides}
        return AppConfig(**merged)
    cfg = AppConfig()
    save_config(cfg)
    return cfg


def save_config(cfg: AppConfig) -> None:
    path = _config_path()
    with open(path, "w", encoding="utf-8") as f:
        json.dump(asdict(cfg), f, indent=2, ensure_ascii=False)
