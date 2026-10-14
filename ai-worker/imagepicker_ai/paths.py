"""Filesystem locations (doc 02 §7)."""

from __future__ import annotations

import os
from pathlib import Path

import platformdirs


def default_data_dir() -> Path:
    """`IMAGEPICKER_DATA_DIR` (set by the app; portable mode points it next to the executable),
    else `<platform data dir>/imagePicker` (Win: %APPDATA%, mac: ~/Library/Application Support,
    Linux: ~/.local/share)."""
    if os.environ.get("IMAGEPICKER_DATA_DIR"):
        return Path(os.environ["IMAGEPICKER_DATA_DIR"])
    return Path(platformdirs.user_data_dir("imagePicker", appauthor=False, roaming=True))


def resolve_models_dir(cli_value: str | os.PathLike[str] | None = None) -> Path:
    """`--models-dir` > `IMAGEPICKER_MODELS_DIR` > `<default data dir>/models`."""
    if cli_value:
        p = Path(cli_value)
    elif os.environ.get("IMAGEPICKER_MODELS_DIR"):
        p = Path(os.environ["IMAGEPICKER_MODELS_DIR"])
    else:
        p = default_data_dir() / "models"
    p = p.expanduser().resolve()
    p.mkdir(parents=True, exist_ok=True)
    return p
