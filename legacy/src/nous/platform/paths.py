"""User data locations shared by installed runtime entry points."""

import os
import sys
from pathlib import Path

from platformdirs import user_data_path


def data_directory() -> Path:
    desktop_name = sys.platform in {"win32", "darwin"}
    current = user_data_path("Nous" if desktop_name else "nous", appauthor=False)
    if sys.platform == "win32" and os.environ.get("LOCALAPPDATA"):
        current = Path(os.environ["LOCALAPPDATA"]) / "Nous"
    return current.expanduser().resolve()


def default_instance_path() -> Path:
    return data_directory() / "instance"
