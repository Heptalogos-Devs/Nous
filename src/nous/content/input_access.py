"""Optional transport-owned materialization of caller-provided file inputs."""

from contextvars import ContextVar
from pathlib import Path
from typing import Protocol

from .contracts import SourceSelection


class InputAccess(Protocol):
    async def materialize(self, path: Path) -> Path: ...
    async def read_text(self, path: Path, selection: SourceSelection, limit: int) -> str: ...
    async def resolve(self, path: Path) -> Path: ...
    def original(self, path: Path) -> Path: ...


current_input_access: ContextVar[InputAccess | None] = ContextVar("nous_input_access", default=None)
