"""Durable handoff of successful transcriptions to transactional adoption."""

from __future__ import annotations

from pathlib import Path

from pydantic import ConfigDict, ValidationError, model_validator

from nous.common.errors import NousError
from nous.runtime.execution import ExecutionManager

from ..contracts import GenerationResult
from .computation import SavedGeneration
from .journal import RecoveryFiles


class _SavedTranscription(SavedGeneration):
    model_config = ConfigDict(extra="forbid")

    producer_key: str

    @model_validator(mode="after")
    def validate_output(self) -> _SavedTranscription:
        if not isinstance(self.output, str):
            raise ValueError("saved transcription output must be text")
        return self

    @classmethod
    def capture(cls, key: str, generated: GenerationResult) -> _SavedTranscription:
        if not isinstance(generated.output, str) or generated.reading is not None:
            raise NousError("media.invalid_transcript", "Transcription returned an invalid result")
        return cls(
            producer_key=key,
            **SavedGeneration.from_result(generated).model_dump(),
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
            return _SavedTranscription.model_validate(raw).to_result()
        except ValidationError as error:
            raise self.files.invalid(error) from error

    async def save(self, key: str, generated: GenerationResult) -> None:
        value = _SavedTranscription.capture(key, generated)
        await self.files.save(key, value.model_dump(mode="json"))

    async def discard(self, key: str) -> None:
        await self.files.discard(key)
