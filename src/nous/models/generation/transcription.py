"""Transcription prerequisites, successful responses and language input sources."""

from __future__ import annotations

import asyncio
import hashlib
import json
from dataclasses import replace
from typing import TYPE_CHECKING, cast
from weakref import WeakValueDictionary

from pydantic import JsonValue, TypeAdapter

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
    SavedGeneration,
    descriptor_text,
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
        parent: FrozenTask,
    ) -> ProcessingResult:
        """Reuse one successful response and adopt it for the exact source context."""
        prerequisite = parent.transcription
        if isinstance(prerequisite, ArtifactRef):
            descriptor = TypeAdapter(dict[str, JsonValue]).validate_json(
                await descriptor_text(self.results, prerequisite)
            )
            frozen = await load_task(
                self.results,
                prerequisite,
                snapshot,
                self.executor.compiler.catalog.tasks,
                check_bindings=False,
            )
            endpoints = [
                cast(dict[str, JsonValue], route["provider"])
                for route in cast(list[dict[str, JsonValue]], descriptor["routes"])
            ]
        else:
            frozen = await self._transcription_for_route(parent, snapshot)
            endpoints = [
                {
                    "protocol": snapshot.values.providers[route.model.provider].protocol,
                    "endpoint": snapshot.values.providers[route.model.provider].endpoint,
                }
                for route in frozen.routes
            ]
        semantics = {
            "task": frozen.task,
            "input": part.model_dump(mode="json"),
            "routes": [
                {
                    "model_name": route.model.model_name,
                    "deployment_revision": route.model.deployment_revision,
                    "protocol": endpoint["protocol"],
                    "endpoint": endpoint["endpoint"],
                }
                for route, endpoint in zip(frozen.routes, endpoints, strict=True)
            ],
            "prompt": frozen.resource.execution_signature if frozen.resource else None,
        }
        response_key = (
            "transcription-response:"
            + hashlib.sha256(
                json.dumps(semantics, sort_keys=True, ensure_ascii=False).encode()
            ).hexdigest()
        )
        group = InputGroup(
            key=source.key,
            slot_key=source.group.slot_key if source.group else source.key,
            name=source.name,
            role=source.group.role if source.group else "source",
            parts=(part,),
        )
        producer_key = (
            "transcription:"
            + hashlib.sha256(
                json.dumps(
                    {"response": response_key, "input": group.model_dump(mode="json")},
                    sort_keys=True,
                    ensure_ascii=False,
                ).encode()
            ).hexdigest()
        )
        lock = self._computation_locks.setdefault(response_key, asyncio.Lock())
        async with self.execution.keyed_operation(
            "model.transcription", lock, snapshot, deadline=context.deadline
        ):
            accepted = await self.results.find_producer(producer_key)
            if accepted is not None:
                self._pending_transcriptions.pop(response_key, None)
                await self._discard_pending(response_key)
                return accepted
            response = await self.results.find_producer(response_key)
            generated = self._pending_transcriptions.get(response_key)
            if response is not None:
                envelope = next(
                    item.artifact for item in response.outputs if item.name == "computation"
                )
                generated = SavedGeneration.model_validate_json(
                    await self.results.artifacts.read_bytes(envelope)
                ).to_result()
                self._pending_transcriptions.pop(response_key, None)
                await self._discard_pending(response_key)
            elif generated is None:
                generated = await self.journal.load(response_key)
            if generated is None:
                bound = await self._transcription_for_route(parent, snapshot)
                artifact = await self.results.artifacts.get(part.content.artifact)
                generated = await self.executor.transcribe(
                    bound,
                    await self.results.artifacts.read_bytes(artifact.ref),
                    artifact.display_name,
                    snapshot,
                    context=context,
                )
                self._pending_transcriptions[response_key] = generated
            if not isinstance(generated.output, str):
                raise NousError(
                    "media.invalid_transcript", "Transcription returned a non-text result"
                )
            durable = response is not None
            try:
                if response is None:
                    await self.journal.save(response_key, generated)
                    durable = True
                    response = await self.results.adopt(
                        operation="model.transcription_response",
                        producer=generated.producer,
                        inputs=(
                            InputGroup(key="AUDIO", slot_key="AUDIO", name="Audio", parts=(part,)),
                        ),
                        outputs={
                            "computation": OutputContent(
                                data=SavedGeneration.from_result(generated)
                                .model_dump_json()
                                .encode(),
                                media_type="application/json",
                                display_name="transcription-response.json",
                            )
                        },
                        request=generated.request,
                        dependencies=(prerequisite,)
                        if isinstance(prerequisite, ArtifactRef)
                        else (),
                        producer_key=response_key,
                    )
                    self._pending_transcriptions.pop(response_key, None)
                    await self._discard_pending(response_key)
                data = generated.output.encode()
                origins = tuple(item.origin for item in part.origins) or (part.content,)
                return await self.results.adopt(
                    operation=frozen.task,
                    producer=generated.producer,
                    inputs=(group,),
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
                    dependencies=(response.ref,),
                    producer_key=producer_key,
                )
            except BaseException as error:
                origins = tuple(item.origin.artifact.identifier for item in part.origins)
                add_error_context(
                    error,
                    {
                        "model_succeeded": True,
                        "producer_key": response_key,
                        "transcription_recovery_durable": durable,
                        "source_artifacts": list(origins) or [part.content.artifact.identifier],
                        "recovery_instruction": (
                            "Retry the operation using the original accepted Artifact and the same selection."
                            if durable
                            else "The response was not saved durably; retrying after this process exits may repeat the model request."
                        ),
                    },
                )
                raise

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
                result = await self.transcribe(source, part, snapshot, context, frozen)
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
