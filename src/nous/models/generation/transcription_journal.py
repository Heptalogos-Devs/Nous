"""Durable handoff of successful transcriptions to transactional adoption."""

from __future__ import annotations

from pathlib import Path
from typing import Literal

from pydantic import BaseModel, ConfigDict, JsonValue, ValidationError

from nous.common.errors import NousError
from nous.runtime.execution import ExecutionManager

from ..contracts import GenerationResult
from .journal import RecoveryFiles


class _SavedTranscription(BaseModel):
    model_config = ConfigDict(extra="forbid")

    version: Literal[1] = 1
    producer_key: str
    output: str
    producer: dict[str, JsonValue]
    usage: dict[str, JsonValue]
    request: dict[str, JsonValue]
    attempts: int
    finish_reason: str | None
    response: dict[str, JsonValue]
    coverage: dict[str, JsonValue]

    @classmethod
    def capture(cls, key: str, generated: GenerationResult) -> _SavedTranscription:
        if not isinstance(generated.output, str) or generated.reading is not None:
            raise NousError("media.invalid_transcript", "Transcription returned an invalid result")
        return cls(
            producer_key=key,
            output=generated.output,
            producer=generated.producer,
            usage=generated.usage,
            request=generated.request,
            attempts=generated.attempts,
            finish_reason=generated.finish_reason,
            response=generated.response,
            coverage=generated.coverage,
        )

    def result(self) -> GenerationResult:
        return GenerationResult(
            output=self.output,
            producer=self.producer,
            usage=self.usage,
            request=self.request,
            attempts=self.attempts,
            finish_reason=self.finish_reason,
            response=self.response,
            coverage=self.coverage,
        )


class TranscriptionJournal:
    """Retain a successful endpoint response until its result has committed."""

    def __init__(self, content_root: Path, execution: ExecutionManager) -> None:
        self.files = RecoveryFiles(
            content_root / "pending" / "transcriptions",
            execution,
            invalid_code="media.transcription_recovery_invalid",
            conflict_code="media.transcription_recovery_conflict",
        )
        self.root = self.files.root

    async def load(self, key: str) -> GenerationResult | None:
        raw = await self.files.load(key)
        if raw is None:
            return None
        try:
            return _SavedTranscription.model_validate(raw).result()
        except ValidationError as error:
            raise self.files.invalid(error) from error

    async def save(self, key: str, generated: GenerationResult) -> None:
        value = _SavedTranscription.capture(key, generated)
        await self.files.save(key, value.model_dump(mode="json"))

    async def discard(self, key: str) -> None:
        await self.files.discard(key)
