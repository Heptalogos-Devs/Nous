"""Atomic, checksummed records for successful computations awaiting adoption."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from pathlib import Path
from typing import cast

from pydantic import JsonValue, TypeAdapter, ValidationError

from nous.common.errors import NousError
from nous.runtime.execution import ExecutionManager

_JSON = TypeAdapter(dict[str, JsonValue])


def _encoded(value: dict[str, JsonValue]) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )


class RecoveryFiles:
    """Own one kind of complete pending computation under the content root.

    Callers serialize access to each producer key. A file is fsynced and
    atomically renamed before adoption; uncommitted records survive process exit.
    """

    def __init__(
        self,
        root: Path,
        execution: ExecutionManager,
        *,
        invalid_code: str,
        conflict_code: str,
    ) -> None:
        self.root, self.execution = root, execution
        self.invalid_code, self.conflict_code = invalid_code, conflict_code

    def invalid(self, error: Exception) -> NousError:
        return NousError(
            self.invalid_code, "Saved computation recovery data is invalid", cause=error
        )

    def _name(self, key: str) -> str:
        return hashlib.sha256(key.encode("utf-8")).hexdigest()

    def _path(self, key: str) -> Path:
        return self.root / (self._name(key) + ".json")

    def _discard_staging(self, key: str) -> None:
        for path in self.root.glob(f".staging-{self._name(key)}-*.json"):
            path.unlink(missing_ok=True)

    def _load(self, key: str) -> dict[str, JsonValue] | None:
        try:
            data = self._path(key).read_bytes()
        except FileNotFoundError:
            return None
        try:
            envelope = _JSON.validate_json(data)
            value = envelope.get("value")
            digest = envelope.get("digest")
            if set(envelope) != {"value", "digest"} or not isinstance(value, dict):
                raise ValueError("invalid recovery envelope")
            payload = cast(dict[str, JsonValue], value)
            if (
                payload.get("producer_key") != key
                or digest != hashlib.sha256(_encoded(payload)).hexdigest()
            ):
                raise ValueError("recovery identity or checksum differs")
        except (ValidationError, ValueError, TypeError) as error:
            raise self.invalid(error) from error
        return payload

    def _save(self, key: str, value: dict[str, JsonValue]) -> None:
        if value.get("producer_key") != key:
            raise NousError(self.conflict_code, "Computation recovery key differs")
        previous = self._load(key)
        if previous is not None:
            if previous != value:
                raise NousError(
                    self.conflict_code, "Another successful computation is saved for this key"
                )
            return
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self._discard_staging(key)
        payload: dict[str, JsonValue] = {
            "value": value,
            "digest": hashlib.sha256(_encoded(value)).hexdigest(),
        }
        fd, name = tempfile.mkstemp(
            prefix=f".staging-{self._name(key)}-", suffix=".json", dir=self.root
        )
        staging = Path(name)
        try:
            with os.fdopen(fd, "wb") as output:
                output.write(_encoded(payload))
                output.flush()
                os.fsync(output.fileno())
            os.replace(staging, self._path(key))
            if hasattr(os, "O_DIRECTORY"):
                directory_fd = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY)
                try:
                    os.fsync(directory_fd)
                finally:
                    os.close(directory_fd)
        finally:
            staging.unlink(missing_ok=True)

    def _discard(self, key: str) -> None:
        self._path(key).unlink(missing_ok=True)
        self._discard_staging(key)

    async def load(self, key: str) -> dict[str, JsonValue] | None:
        return await self.execution.blocking(self._load, key)

    async def save(self, key: str, value: dict[str, JsonValue]) -> None:
        await self.execution.blocking(self._save, key, value)

    async def discard(self, key: str) -> None:
        await self.execution.blocking(self._discard, key)
