"""Configuration and prompt operations shared by running and offline instances."""

from __future__ import annotations

import json
from collections.abc import AsyncGenerator, Awaitable, Callable
from contextlib import asynccontextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from pydantic import BaseModel, ConfigDict, JsonValue

from nous.application.configuration import ConfigurationOperations, open_configuration_operations
from nous.application.contracts import (
    BasicEvents,
    BoundOperation,
    CallContext,
    OperationDefinition,
    define_operation,
)
from nous.application.dispatch import Application
from nous.application.prompt_operations import (
    DESCRIBE_PROMPT,
    DIFF_PROMPT,
    IMPORT_PROMPT,
    LIST_PROMPTS,
    PREPARE_PROMPT_EDIT,
    READ_PROMPT,
    RESTORE_PROMPT,
    VALIDATE_PROMPT,
    PromptCalls,
)
from nous.application.time_operations import PARSE_TIME, parse_time
from nous.common.errors import NousError
from nous.configuration import Configuration
from nous.content.contracts import ExternalInput
from nous.models.prompts import PromptCatalog
from nous.runtime.execution import ExecutionManager
from nous.runtime.instance import InstanceLocator
from nous.storage.file_input import FileReaderPool


class ListConfiguration(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    level: str = "standard"


class ShowConfiguration(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    saved: bool = False


class EmptyRequest(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")


class SetConfiguration(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    path: str
    value: JsonValue = None
    input: ExternalInput | None = None


class ConfigurationPath(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    path: str


LIST_CONFIGURATION = define_operation(
    "configuration.list",
    request=ListConfiguration,
    result=list[dict[str, object]],
    description="List available configuration fields.",
)
SHOW_CONFIGURATION = define_operation(
    "configuration.show",
    request=ShowConfiguration,
    result=dict[str, object],
    description="Show saved or prospective configuration.",
)
EXPLAIN_CONFIGURATION = define_operation(
    "configuration.explain",
    request=ConfigurationPath,
    result=dict[str, object],
    description="Explain a configuration value and its sources.",
)
VALIDATE_CONFIGURATION = define_operation(
    "configuration.validate",
    request=EmptyRequest,
    result=dict[str, object],
    description="Validate saved configuration and resource references.",
)
LIST_MODELS = define_operation(
    "models.list",
    request=EmptyRequest,
    result=dict[str, object],
    description="List configured models.",
)
SET_CONFIGURATION = define_operation(
    "configuration.set",
    request=SetConfiguration,
    result=dict[str, object],
    events=BasicEvents,
    description="Save a configuration value.",
)
UNSET_CONFIGURATION = define_operation(
    "configuration.unset",
    request=ConfigurationPath,
    result=dict[str, object],
    events=BasicEvents,
    description="Remove a configuration override.",
)
REMOVE_CONFIGURATION_RESOURCE = define_operation(
    "configuration.remove_resource",
    request=ConfigurationPath,
    result=dict[str, object],
    events=BasicEvents,
    description="Remove a configuration resource after checking its references.",
)


class ConfigurationCalls:
    """Inspect and change configuration through the same underlying owner."""

    def __init__(
        self, configuration: Configuration, writer: ConfigurationOperations | None = None
    ) -> None:
        self.configuration, self.writer = configuration, writer

    async def catalog(
        self, request: ListConfiguration, context: CallContext
    ) -> list[dict[str, object]]:
        return self.configuration.catalog(request.level)

    async def show(self, request: ShowConfiguration, context: CallContext) -> dict[str, object]:
        snapshot = context.settings
        return {
            "state": "saved" if request.saved else "prospective",
            "file": str(self.configuration.config_file),
            "values": self.configuration.saved() if request.saved else snapshot.to_dict(),
            "version": snapshot.version,
        }

    async def explain(self, request: ConfigurationPath, context: CallContext) -> dict[str, object]:
        return self.configuration.explain(request.path)

    async def validate(self, request: EmptyRequest, context: CallContext) -> dict[str, object]:
        return self.configuration.validate()

    async def list_models(self, request: EmptyRequest, context: CallContext) -> dict[str, object]:
        return {
            "models": {
                key: value.model_dump(mode="json")
                for key, value in context.settings.values.models.items()
            }
        }

    async def set(self, request: SetConfiguration, context: CallContext) -> dict[str, object]:
        if self.writer is None:
            raise NousError("operation.unavailable", "Configuration changes are unavailable")
        value: object = request.value
        if request.input is not None:
            selected = await context.inputs.read_text(
                request.input.key,
                request.input.selection,
                context.settings.values.content.text_read_max_bytes,
            )
            value = json.loads(selected.text)
        return await self.writer.update({request.path: value})

    async def unset(self, request: ConfigurationPath, context: CallContext) -> dict[str, object]:
        if self.writer is None:
            raise NousError("operation.unavailable", "Configuration changes are unavailable")
        return await self.writer.unset(request.path)

    async def remove_resource(
        self, request: ConfigurationPath, context: CallContext
    ) -> dict[str, object]:
        if self.writer is None:
            raise NousError("operation.unavailable", "Configuration changes are unavailable")
        return await self.writer.remove_resource(request.path)


class OfflineCalls:
    """Assemble the same configuration, prompt and time operations for each runtime."""

    def __init__(
        self,
        configuration: Configuration,
        prompts: PromptCatalog,
        execution: ExecutionManager,
        *,
        temp: Path | None = None,
        writer: ConfigurationOperations | None = None,
    ) -> None:
        self.configuration = ConfigurationCalls(configuration, writer)
        self.prompts = PromptCalls(prompts, execution, temp=temp)

    def bindings(self, *, writable: bool) -> tuple[BoundOperation[object, object], ...]:
        return tuple(item.bind(self) for item in _OFFLINE_BINDINGS if writable or not item.writable)


@dataclass(frozen=True)
class _OfflineBinding:
    definition: OperationDefinition[object, object]
    bind: Callable[[OfflineCalls], BoundOperation[object, object]]
    writable: bool


def _offline[Q, R](
    definition: OperationDefinition[Q, R],
    handler: Callable[[OfflineCalls], Callable[[Q, CallContext], Awaitable[R]]],
    writable: bool = False,
) -> _OfflineBinding:
    def bind(owner: OfflineCalls) -> BoundOperation[object, object]:
        return cast(BoundOperation[object, object], BoundOperation(definition, handler(owner)))

    return _OfflineBinding(cast(OperationDefinition[object, object], definition), bind, writable)


# The offline and running compositions use this same registration.  The final
# flag determines which offline operations require the instance write lock.
_OFFLINE_BINDINGS = (
    _offline(LIST_CONFIGURATION, lambda owner: owner.configuration.catalog),
    _offline(SHOW_CONFIGURATION, lambda owner: owner.configuration.show),
    _offline(EXPLAIN_CONFIGURATION, lambda owner: owner.configuration.explain),
    _offline(VALIDATE_CONFIGURATION, lambda owner: owner.configuration.validate),
    _offline(LIST_MODELS, lambda owner: owner.configuration.list_models),
    _offline(PARSE_TIME, lambda owner: parse_time),
    _offline(LIST_PROMPTS, lambda owner: owner.prompts.list_prompts),
    _offline(READ_PROMPT, lambda owner: owner.prompts.read_prompt),
    _offline(DESCRIBE_PROMPT, lambda owner: owner.prompts.describe_prompt),
    _offline(DIFF_PROMPT, lambda owner: owner.prompts.diff_prompt),
    _offline(VALIDATE_PROMPT, lambda owner: owner.prompts.validate_prompt),
    _offline(SET_CONFIGURATION, lambda owner: owner.configuration.set, True),
    _offline(UNSET_CONFIGURATION, lambda owner: owner.configuration.unset, True),
    _offline(
        REMOVE_CONFIGURATION_RESOURCE, lambda owner: owner.configuration.remove_resource, True
    ),
    _offline(PREPARE_PROMPT_EDIT, lambda owner: owner.prompts.prepare_prompt_edit, True),
    _offline(RESTORE_PROMPT, lambda owner: owner.prompts.restore_prompt, True),
    _offline(IMPORT_PROMPT, lambda owner: owner.prompts.import_prompt, True),
)


def offline_access(name: str) -> bool | None:
    """Return whether an operation needs the offline write lock, or is unavailable offline."""
    return next((item.writable for item in _OFFLINE_BINDINGS if item.definition.name == name), None)


@asynccontextmanager
async def open_configuration_application(
    locator: InstanceLocator, *, writable: bool
) -> AsyncGenerator[tuple[Application, ExecutionManager]]:
    """Assemble only configuration operations when an instance is offline."""
    from nous.runtime.bootstrap import task_catalog

    readers = FileReaderPool(1)
    try:
        if writable:
            async with open_configuration_operations(locator) as writer:
                calls = OfflineCalls(
                    writer.configuration,
                    PromptCatalog(writer.configuration.config_file.parent, task_catalog()),
                    writer.execution,
                    temp=writer.paths.temp,
                    writer=writer,
                )
                yield (
                    Application(
                        calls.bindings(writable=True),
                        execution=writer.execution,
                        artifacts=None,
                        readers=readers,
                    ),
                    writer.execution,
                )
        else:
            configuration = Configuration(locator.config_file)
            execution = ExecutionManager(
                configuration.snapshot.values.execution, lambda: configuration.snapshot
            )
            try:
                calls = OfflineCalls(
                    configuration,
                    PromptCatalog(configuration.config_file.parent, task_catalog()),
                    execution,
                )
                yield (
                    Application(
                        calls.bindings(writable=False),
                        execution=execution,
                        artifacts=None,
                        readers=readers,
                    ),
                    execution,
                )
            finally:
                await execution.aclose()
    finally:
        await readers.aclose()
