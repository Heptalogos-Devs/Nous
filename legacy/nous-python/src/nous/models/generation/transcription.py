"""Resolve and persist complete audio transcriptions for language inputs."""

from __future__ import annotations

import asyncio
import hashlib
import json
from dataclasses import replace
from typing import TYPE_CHECKING
from weakref import WeakValueDictionary

from nous.common.errors import NousError
from nous.content.contracts import (
    ContentSelection,
    InputGroup,
    InputPart,
    ModelProducer,
    OriginSlice,
    OutputContent,
    OutputMapping,
    ProcessingResult,
    ResultFacts,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceSelection,
)
from nous.content.results import ResultStore
from nous.media.contracts import PreparedSource
from nous.runtime.execution import ExecutionManager, OperationContext

from ..contracts import ReadingInput
from ..requests import FrozenTask
from .contracts import TranscriptionRequestFacts

if TYPE_CHECKING:
    from nous.configuration import ConfigSnapshot

    from ..executor import ModelExecutor


class TranscriptionOperations:
    """Reuse completed transcripts keyed by exact material and model semantics."""

    def __init__(
        self,
        executor: ModelExecutor,
        results: ResultStore,
        execution: ExecutionManager,
        locks: WeakValueDictionary[str, asyncio.Lock],
    ) -> None:
        self.executor, self.results, self.execution, self._locks = (
            executor,
            results,
            execution,
            locks,
        )

    async def transcribe(
        self,
        source: PreparedSource,
        part: InputPart,
        snapshot: ConfigSnapshot,
        context: OperationContext,
    ) -> ProcessingResult:
        task_name = snapshot.values.media.audio.transcription_task
        if not task_name:
            raise NousError("media.audio_capability_missing", "A transcription task is required")
        try:
            frozen = self.executor.freeze(task_name, snapshot)
        except NousError as error:
            raise NousError(
                "media.audio_capability_missing",
                "The configured transcription task is unavailable",
                details={"transcription_task": task_name, "configuration_error": error.to_dict()},
                cause=error,
            ) from error
        group = InputGroup(
            key=source.key,
            slot_key=source.group.slot_key if source.group else source.key,
            name=source.name,
            role=source.group.role if source.group else "source",
            parts=(part,),
            context=source.group.context if source.group else None,
        )
        origins = tuple(item.origin for item in part.origins) or (part.content,)
        semantics = {
            "task": task_name,
            "input": group.model_dump(mode="json"),
            "routes": [
                {
                    "model_id": route.model_id,
                    "provider": route.model.provider,
                    "model_name": route.model.model_name,
                    "deployment_revision": route.model.deployment_revision,
                    "preset": route.preset.model_dump(mode="json"),
                }
                for route in frozen.routes
            ],
        }
        key = hashlib.sha256(
            json.dumps(semantics, sort_keys=True, ensure_ascii=False).encode()
        ).hexdigest()
        lock = self._locks.setdefault(key, asyncio.Lock())
        async with (
            self.execution.operation("model.transcription", snapshot, deadline=context.deadline),
            lock,
        ):
            cached = await self.results.find_cached(task_name, key)
            if cached is not None:
                return cached
            artifact = await self.results.artifacts.get(part.content.artifact)
            generated = await self.executor.transcribe(
                frozen,
                await self.results.artifacts.read_bytes(artifact.ref),
                artifact.display_name,
                snapshot,
                context=context,
            )
            if not isinstance(generated.output, str):
                raise NousError(
                    "media.invalid_transcript", "Transcription returned a non-text result"
                )
            data = generated.output.encode("utf-8")
            mappings = (
                (
                    OutputMapping(
                        output_name="transcript",
                        selection=SourceSelection(start_byte=0, end_byte=len(data)),
                        relation="atomic",
                        origins=origins,
                    ),
                )
                if data
                else ()
            )
            facts = ResultFacts(
                operation=task_name,
                producer=ModelProducer.model_validate(generated.producer),
                selected_inputs=SelectedInputs(
                    sources=(
                        SelectedSource(
                            slot_key=group.slot_key,
                            role=group.role,
                            selections=origins,
                            display_name=source.name,
                        ),
                    )
                ),
                inputs=(group,),
                request=TranscriptionRequestFacts.model_validate(generated.request).model_dump(
                    mode="json"
                ),
                parameters={
                    "cache_key": key,
                    "usage": generated.usage,
                    "attempts": generated.attempts,
                },
                coverage={
                    "mode": "transcript",
                    "origins": [item.model_dump(mode="json") for item in origins],
                },
                mappings=mappings,
                dependencies=tuple(
                    dict.fromkeys(item.transform_ref for item in part.origins if item.transform_ref)
                ),
                formed_at=self.results.artifacts.clock.utc_now(),
            )
            return await self.results.adopt(
                facts=facts,
                outputs={
                    "transcript": OutputContent(
                        data=data,
                        media_type="text/plain",
                        display_name="transcript.txt",
                        representation_kind="transcript",
                    )
                },
            )

    async def language_sources(
        self,
        reading: ReadingInput,
        frozen: FrozenTask,
        snapshot: ConfigSnapshot,
        context: OperationContext,
        completed: list[ResultRef],
    ) -> ReadingInput:
        sources: list[PreparedSource] = []
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
                result = await self.transcribe(source, part, snapshot, context)
                completed.append(result.ref)
                output = next(item for item in result.outputs if item.name == "transcript")
                texts.append(await self.results.artifacts.read_text(output.artifact))
                parts.append(
                    InputPart(
                        content=ContentSelection(artifact=output.artifact),
                        representation="transcript",
                        media_type="text/plain",
                        source_media_type=part.source_media_type or part.media_type,
                        origins=tuple(
                            OriginSlice(origin=origin, transform_ref=result.ref, precision="atomic")
                            for origin in (
                                tuple(item.origin for item in part.origins) or (part.content,)
                            )
                        ),
                    )
                )
            sources.append(
                replace(
                    source,
                    text="\n".join(texts),
                    media=tuple(media),
                    group=source.group.model_copy(update={"parts": tuple(parts)}),
                )
            )
        return replace(reading, sources=tuple(sources))
