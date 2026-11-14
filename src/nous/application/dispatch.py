"""Type-safe dispatch to the complete operations assembled for an instance."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from copy import deepcopy
from typing import TYPE_CHECKING, cast

from pydantic import JsonValue

from nous.application.contracts import (
    BoundOperation,
    CallContext,
    EventEmitter,
    EventObserver,
    OperationDefinition,
)
from nous.application.inputs import InputProvider, InputReader
from nous.common.errors import NousError
from nous.presentation.json_selection import select_json, validate_json_pointers

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore
    from nous.runtime.execution import ExecutionManager
    from nous.storage.file_input import FileReaderPool


class Application:
    """One instance's complete operations and request boundary."""

    def __init__(
        self,
        operations: Sequence[BoundOperation[object, object]],
        *,
        execution: ExecutionManager,
        artifacts: ArtifactStore | None,
        readers: FileReaderPool,
        results: ResultStore | None = None,
        chunk_size: int = 1024 * 1024,
    ) -> None:
        self._operations = {item.definition.name: item for item in operations}
        if len(self._operations) != len(operations):
            raise ValueError("Duplicate application operation")
        self._execution = execution
        self._artifacts = artifacts
        self._results = results
        self._readers = readers
        self._chunk_size = chunk_size

    def list(self) -> tuple[dict[str, str], ...]:
        return tuple(
            {"name": item.definition.name, "description": item.definition.description}
            for item in self._operations.values()
        )

    def describe(self, name: str) -> dict[str, JsonValue]:
        definition = self._get(name).definition
        return {
            "name": definition.name,
            "description": definition.description,
            "request": definition.request_adapter.json_schema(),
            "result": definition.result_adapter.json_schema(mode="serialization"),
            "events": definition.event_adapter.json_schema(),
        }

    def _get(self, name: str) -> BoundOperation[object, object]:
        try:
            return self._operations[name]
        except KeyError as error:
            raise NousError(
                "operation.unknown", "Unknown application operation", details={"name": name}
            ) from error

    def parse(self, name: str, request: object) -> object:
        """Resolve and capture a wire request before accepting external inputs."""
        definition = self._get(name).definition
        return definition.request_adapter.validate_python(deepcopy(request))

    async def call[Q, R](
        self,
        operation: OperationDefinition[Q, R],
        request: Q,
        *,
        inputs: Mapping[str, InputProvider] | None = None,
        observer: EventObserver | None = None,
        timeout: float | None = None,  # noqa: ASYNC109
    ) -> R:
        bound = self._get(operation.name)
        if bound.definition is not operation:
            raise NousError("operation.unknown", "Operation definition is not registered")
        result = await self._invoke(
            bound, request, inputs=inputs, observer=observer, timeout=timeout
        )
        return cast(R, result)

    async def call_name(
        self,
        name: str,
        request: object,
        *,
        inputs: Mapping[str, InputProvider] | None = None,
        observer: EventObserver | None = None,
        timeout: float | None = None,  # noqa: ASYNC109
        select: Sequence[str] = (),
    ) -> object:
        """Call a named operation and optionally project its serialized result."""
        pointers = validate_json_pointers(select)
        return await self._invoke(
            self._get(name),
            request,
            inputs=inputs,
            observer=observer,
            timeout=timeout,
            select=pointers,
        )

    async def _invoke(
        self,
        bound: BoundOperation[object, object],
        request: object,
        *,
        inputs: Mapping[str, InputProvider] | None,
        observer: EventObserver | None,
        timeout: float | None,  # noqa: ASYNC109
        select: Sequence[str] = (),
    ) -> object:
        if timeout is not None and timeout <= 0:
            raise NousError("operation.invalid_timeout", "Timeout must be positive")
        definition = bound.definition
        checked = definition.request_adapter.validate_python(deepcopy(request))
        async with self._execution.request(definition.name, timeout=timeout) as execution:
            reader = InputReader(
                inputs or {},
                artifacts=self._artifacts,
                results=self._results,
                readers=self._readers,
                execution=self._execution,
                chunk_size=self._chunk_size,
            )
            context = CallContext(
                execution=execution,
                inputs=reader,
                events=EventEmitter(execution.operation_id, definition.event_adapter, observer),
            )
            result = await bound.handle(checked, context)
            result = definition.result_adapter.validate_python(result)
            return select_json(result, select) if select else result
