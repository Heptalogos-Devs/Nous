"""Transcription prerequisites, successful responses and language input sources."""

from __future__ import annotations

import asyncio
import hashlib
import json
from dataclasses import replace
from typing import TYPE_CHECKING
from weakref import WeakValueDictionary

from nous.common.errors import NousError
from nous.common.operations import add_error_context
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
    OriginSlice,
    OutputContent,
    OutputMapping,
    ProcessingResult,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.media.contracts import PreparedSource
from nous.observability.instrumentation import get_logger
from nous.runtime.execution import ExecutionManager, OperationContext

from ..contracts import GenerationResult, ReadingInput
from ..requests import FrozenTask, UnavailableTask
from .computation import (
    load_task,
)
from .transcription_journal import TranscriptionJournal

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot

    from ..executor import ModelExecutor


class TranscriptionOperations:
    """Resolve and adopt audio transcriptions before preparing language inputs."""

    def __init__(
        self,
        executor: ModelExecutor,
        results: ResultStore,
        execution: ExecutionManager,
        locks: WeakValueDictionary[str, asyncio.Lock],
    ) -> None:
        self.executor, self.results, self.execution, self._computation_locks = (
            executor,
            results,
            execution,
            locks,
        )
        self.journal = TranscriptionJournal(results.artifacts.content.root, execution)
        self._pending_transcriptions: dict[str, GenerationResult] = {}

    async def _discard_pending(self, key: str) -> None:
        try:
            await self.journal.discard(key)
        except OSError as error:
            get_logger(__name__).warning(
                "media.transcription_recovery_cleanup_failed",
                extra={"error_type": type(error).__name__},
            )

    def freeze_transcription(self, snapshot: ConfigSnapshot) -> FrozenTask | None:
        task = snapshot.values.media.audio.transcription_task
        return self.executor.freeze(task, snapshot) if task else None

    async def _transcription_for_route(
        self, frozen: FrozenTask, snapshot: ConfigSnapshot
    ) -> FrozenTask:
        """Resolve the frozen prerequisite only when this route needs audio text."""
        prerequisite = frozen.transcription
        task = (
            frozen.media_settings.audio.transcription_task
            if frozen.media_settings is not None
            else snapshot.values.media.audio.transcription_task
        )
        if isinstance(prerequisite, UnavailableTask):
            raise NousError(
                "media.audio_capability_missing",
                "The selected route requires an unavailable transcription task",
                details={
                    "transcription_task": prerequisite.task,
                    "configuration_error": {
                        "code": prerequisite.code,
                        "message": prerequisite.message,
                        "details": prerequisite.details,
                        "retryable": prerequisite.retryable,
                    },
                },
            )
        try:
            if isinstance(prerequisite, ArtifactRef):
                return await load_task(
                    self.results, prerequisite, snapshot, self.executor.compiler.catalog.tasks
                )
            if frozen.media_settings is None:
                current = self.freeze_transcription(snapshot)
                if current is not None:
                    return current
        except NousError as error:
            if error.code not in {
                "model.task_unconfigured",
                "model.resource_binding",
                "model.task_contract_changed",
            } and not error.code.startswith("prompt."):
                raise
            raise NousError(
                "media.audio_capability_missing",
                "The selected route requires an unavailable transcription task",
                details={"transcription_task": task, "configuration_error": error.to_dict()},
                cause=error,
            ) from error
        raise NousError(
            "media.audio_capability_missing",
            "The selected route requires a configured transcription task",
            details={"transcription_task": task},
        )

    async def transcribe(
        self,
        source: PreparedSource,
        part: InputPart,
        snapshot: ConfigSnapshot,
        context: OperationContext,
        frozen: FrozenTask | None = None,
    ) -> ProcessingResult:
        """Adopt exactly one successful endpoint response, with segment-local body coordinates."""
        task = frozen.task if frozen is not None else snapshot.values.media.audio.transcription_task
        if task is None:
            raise NousError(
                "media.audio_capability_missing",
                "Audio requires native input or a configured transcription task",
            )
        frozen = frozen or self.executor.freeze(task, snapshot)
        identity = hashlib.sha256(
            json.dumps(
                {
                    "task": task,
                    "input": part.model_dump(mode="json"),
                    "source": {
                        "key": source.key,
                        "slot_key": source.group.slot_key if source.group else source.key,
                        "name": source.name,
                        "role": source.group.role if source.group else "source",
                    },
                    "assignment": frozen.assignment.model_dump(mode="json"),
                    "routes": [
                        {
                            "id": route.model_id,
                            "model": route.model.model_dump(mode="json"),
                            "provider": snapshot.values.providers[route.model.provider].model_dump(
                                mode="json"
                            ),
                            "preset": route.preset.model_dump(mode="json"),
                        }
                        for route in frozen.routes
                    ],
                },
                sort_keys=True,
                ensure_ascii=False,
            ).encode()
        ).hexdigest()
        producer_key = "transcription:" + identity
        lock = self._computation_locks.setdefault(producer_key, asyncio.Lock())
        async with self.execution.keyed_operation(
            "model.transcription", lock, snapshot, deadline=context.deadline
        ):
            accepted = await self.results.find_producer(producer_key)
            if accepted is not None:
                self._pending_transcriptions.pop(producer_key, None)
                await self._discard_pending(producer_key)
                return accepted
            generated = self._pending_transcriptions.get(producer_key)
            if generated is None:
                generated = await self.journal.load(producer_key)
                if generated is not None:
                    self._pending_transcriptions[producer_key] = generated
            if generated is None:
                artifact = await self.results.artifacts.get(part.content.artifact)
                generated = await self.executor.transcribe(
                    frozen,
                    await self.results.artifacts.read_bytes(artifact.ref),
                    artifact.display_name,
                    snapshot,
                    context=context,
                )
                self._pending_transcriptions[producer_key] = generated
            if not isinstance(generated.output, str):
                raise NousError(
                    "media.invalid_transcript", "Transcription returned a non-text result"
                )
            durable = False
            try:
                await self.journal.save(producer_key, generated)
                durable = True
                data = generated.output.encode()
                origins = tuple(item.origin for item in part.origins) or (part.content,)
                result = await self.results.adopt(
                    operation=task,
                    producer=generated.producer,
                    inputs=(
                        InputGroup(
                            key=source.key,
                            slot_key=source.group.slot_key if source.group else source.key,
                            name=source.name,
                            role=source.group.role if source.group else "source",
                            parts=(part,),
                        ),
                    ),
                    outputs={
                        "transcript": OutputContent(
                            data=data, media_type="text/plain", display_name="transcript.txt"
                        )
                    },
                    request=generated.request,
                    parameters={
                        "usage": generated.usage,
                        "attempts": generated.attempts,
                        "representations": {"transcript": {"kind": "transcript"}},
                    },
                    coverage={
                        "mode": "transcript",
                        "origins": [origin.model_dump(mode="json") for origin in part.origins],
                    },
                    mappings=(
                        OutputMapping(
                            output_name="transcript",
                            selection=SourceSelection(start_byte=0, end_byte=len(data)),
                            relation="atomic",
                            origins=origins,
                        ),
                    )
                    if data
                    else (),
                    producer_key=producer_key,
                )
            except BaseException as error:
                origins = tuple(item.origin.artifact.identifier for item in part.origins)
                add_error_context(
                    error,
                    {
                        "model_succeeded": True,
                        "producer_key": producer_key,
                        "transcription_recovery_durable": durable,
                        "source_artifacts": list(origins) or [part.content.artifact.identifier],
                        "recovery_instruction": (
                            "Retry the same operation with the original accepted Artifact identifier "
                            "and the same options; retrying the original file path creates a new Artifact."
                            if durable
                            else "The transcription could not be saved durably; retrying after this process exits may repeat the model request."
                        ),
                    },
                )
                raise
            self._pending_transcriptions.pop(producer_key, None)
            await self._discard_pending(producer_key)
            return result

    async def language_sources(
        self,
        reading: ReadingInput,
        frozen: FrozenTask,
        snapshot: ConfigSnapshot,
        context: OperationContext,
        completed: list[str],
    ) -> ReadingInput:
        """Resolve language-dependent representations after deterministic preparation has finished."""
        sources: list[PreparedSource] = []
        transcription: FrozenTask | None = None
        accepted = frozen.routes[0].model.input_formats
        for source in reading.sources:
            if source.group is None or not any(
                media.media_type.startswith("audio/") and media.media_type not in accepted
                for media in source.media
            ):
                sources.append(source)
                continue
            parts: list[InputPart] = []
            texts = [source.text] if source.text else []
            media = [
                medium for medium in source.media if not medium.media_type.startswith("audio/")
            ]
            for part in source.group.parts:
                artifact = await self.results.artifacts.get(part.content.artifact)
                if not artifact.media_type.startswith("audio/") or artifact.media_type in accepted:
                    parts.append(part)
                    continue
                if transcription is None:
                    transcription = await self._transcription_for_route(frozen, snapshot)
                result = await self.transcribe(source, part, snapshot, context, transcription)
                completed.append(result.ref.identifier)
                output = next(output for output in result.outputs if output.name == "transcript")
                texts.append(await self.results.artifacts.read_text(output.artifact))
                parts.append(
                    InputPart(
                        content=ContentSelection(artifact=output.artifact),
                        representation="transcript",
                        media_type="text/plain",
                        source_media_type=part.source_media_type or part.media_type,
                        origins=tuple(
                            OriginSlice(origin=origin, transform_ref=result.ref)
                            for origin in (
                                tuple(item.origin for item in part.origins) or (part.content,)
                            )
                        ),
                    )
                )
            group = source.group.model_copy(update={"parts": tuple(parts)})
            sources.append(replace(source, text="\n".join(texts), media=tuple(media), group=group))
        return replace(reading, sources=tuple(sources))
