"""Deterministic media derivative lookup, adoption and completion tracking."""

from __future__ import annotations

import asyncio
import hashlib
import json
from collections.abc import Awaitable, Callable
from contextvars import ContextVar
from weakref import WeakValueDictionary

from pydantic import JsonValue

from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ContentSelection,
    InputGroup,
    InputPart,
    OutputContent,
    OutputMapping,
    ProgramProducer,
    ResultFacts,
    ResultRef,
    SelectedInputs,
    SelectedSource,
    SourceSelection,
)
from nous.content.results import ResultStore

preparation_results: ContextVar[list[ResultRef] | None] = ContextVar(
    "media_preparation_results", default=None
)


def track_completed(reference: ResultRef) -> None:
    completed = preparation_results.get()
    if completed is not None:
        completed.append(reference)
    from nous.common.operations import record_completed

    record_completed(reference)


class MediaDerivatives:
    """Reuse and transactionally adopt deterministic media conversions."""

    def __init__(self, artifacts: ArtifactStore, results: ResultStore) -> None:
        self.artifacts, self.results = artifacts, results
        self._locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()

    @staticmethod
    def key(
        original: Artifact,
        selection: SourceSelection,
        operation: str,
        parameters: dict[str, JsonValue],
    ) -> str:
        return hashlib.sha256(
            json.dumps(
                {
                    "artifact": original.ref.id,
                    "source_media_type": original.media_type,
                    "selection": selection.model_dump(mode="json"),
                    "operation": operation,
                    "parameters": parameters,
                },
                sort_keys=True,
            ).encode()
        ).hexdigest()

    async def cached(self, operation: str, cache_key: str) -> dict[str, Artifact] | None:
        cached = await self.results.find_cached(operation, cache_key)
        if cached is not None:
            track_completed(cached.ref)
            return {
                output.name: await self.artifacts.get(output.artifact) for output in cached.outputs
            }
        return None

    async def derive(
        self,
        original: Artifact,
        selection: SourceSelection,
        operation: str,
        outputs: dict[str, OutputContent],
        parameters: dict[str, JsonValue],
    ) -> dict[str, Artifact]:
        """Reuse derivatives for the same accepted source, selection and conversion parameters."""

        async def prepared() -> dict[str, OutputContent]:
            return outputs

        return await self.get_or_create(original, selection, operation, parameters, prepared)

    async def get_or_create(
        self,
        original: Artifact,
        selection: SourceSelection,
        operation: str,
        parameters: dict[str, JsonValue],
        create: Callable[[], Awaitable[dict[str, OutputContent]]],
    ) -> dict[str, Artifact]:
        """Serialize same-key conversion and adoption without retaining idle locks."""
        cache_key = self.key(original, selection, operation, parameters)
        lock = self._locks.setdefault(cache_key, asyncio.Lock())
        async with self.artifacts.content.execution.operation("media.derivative"), lock:
            cached = await self.cached(operation, cache_key)
            if cached is not None:
                return cached
            outputs = await create()
            selected = ContentSelection(artifact=original.ref, selection=selection)
            result = await self.results.adopt(
                facts=ResultFacts(
                    operation=operation,
                    producer=ProgramProducer(operation=operation, parameters=parameters),
                    selected_inputs=SelectedInputs(
                        sources=(
                            SelectedSource(
                                slot_key="S1",
                                selections=(selected,),
                                display_name=original.display_name,
                            ),
                        )
                    ),
                    inputs=(
                        InputGroup(
                            key="S1",
                            slot_key="S1",
                            name=original.display_name,
                            parts=(
                                InputPart(
                                    content=ContentSelection(
                                        artifact=original.ref, selection=selection
                                    ),
                                    source_media_type=original.media_type,
                                    media_type=original.media_type,
                                ),
                            ),
                        ),
                    ),
                    parameters={
                        **parameters,
                        "cache_key": cache_key,
                    },
                    coverage={"selection": selection.model_dump(mode="json")},
                    mappings=tuple(
                        OutputMapping(
                            output_name=name,
                            selection=SourceSelection(start_byte=0, end_byte=len(output.data)),
                            relation="atomic",
                            origins=(ContentSelection(artifact=original.ref, selection=selection),),
                        )
                        for name, output in outputs.items()
                        if output.data
                    ),
                    formed_at=self.artifacts.clock.utc_now(),
                ),
                outputs={
                    name: output.model_copy(
                        update={"representation_kind": output.representation_kind or "prepared"}
                    )
                    for name, output in outputs.items()
                },
            )
            track_completed(result.ref)
            return {
                output.name: await self.artifacts.get(output.artifact) for output in result.outputs
            }
