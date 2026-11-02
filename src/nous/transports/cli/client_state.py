"""Connection and current work selection belong to the terminal client."""

from __future__ import annotations

import os
from pathlib import Path

import tomlkit
from platformdirs import user_config_path
from pydantic import BaseModel, Field


class ClientState(BaseModel):
    url: str = "http://127.0.0.1:7441"
    instance: Path | None = None
    credential_file: Path | None = None
    namespace: str | None = None
    work: dict[str, str] = Field(default_factory=dict)


def personal_path() -> Path:
    configured = os.environ.get("NOUS_CLIENT_CONFIG")
    return (
        Path(configured).expanduser()
        if configured
        else user_config_path("Nous", appauthor=False) / "client.toml"
    )


def load() -> tuple[ClientState, Path]:
    personal = personal_path()
    project = Path.cwd() / ".nous.toml"
    values: dict[str, object] = {}
    for path in dict.fromkeys((personal, project)):
        if path.exists():
            values.update(tomlkit.parse(path.read_text(encoding="utf-8")).unwrap())
    return ClientState.model_validate(values), project if project.exists() else personal


def save(path: Path, state: ClientState) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".pending")
    with temporary.open("w", encoding="utf-8") as stream:
        stream.write(tomlkit.dumps(state.model_dump(mode="json", exclude_none=True)))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
