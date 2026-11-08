"""The application operations available to local and hosted clients."""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MethodType
from typing import Literal, cast, get_type_hints

from pydantic import ConfigDict, TypeAdapter, validate_call
from pydantic_core import to_jsonable_python

from nous.application import tools as application_tools
from nous.application.configuration import ConfigurationOperations
from nous.application.editing import EditSession
from nous.application.inputs import InputParameter, bind_inputs, read_input
from nous.cognition.annotation.operations import AnnotationOperations
from nous.cognition.concept_contracts import AnnotationProposalView, AnnotationView
from nous.cognition.concepts import ConceptOperations, ConceptTarget
from nous.cognition.feedback import FeedbackOperations
from nous.cognition.maintenance.operations import MaintenanceOperations
from nous.cognition.retrieval.operations import CognitiveRetrieval
from nous.cognition.scope import NamespaceOperations
from nous.cognition.writing.contracts import AfterWrite, DraftOverride, RecordOutcome, SourceChanges
from nous.cognition.writing.generation import WritingOperations
from nous.cognition.writing.operations import RecordOperations
from nous.common.errors import NousError
from nous.configuration import Configuration
from nous.content.annotations import AnnotationProposal
from nous.content.contracts import InputSpec
from nous.content.input_access import current_input_access
from nous.models.prompts import PromptCatalog
from nous.runtime.bootstrap import Runtime, task_catalog
from nous.runtime.instance import InstanceLocator
from nous.wave.operations import WaveOperations


@dataclass(frozen=True)
class Action:
    scope: str
    method: Callable[..., Awaitable[object]]

    def result(self, value: object) -> object:
        return TypeAdapter(get_type_hints(self.method)["return"]).validate_python(value)


# These are complete application operations, not an exposed Runtime object graph.
_METHODS: tuple[tuple[str, type[object], tuple[str, ...]], ...] = (
    (
        "records",
        RecordOperations,
        (
            "add_note",
            "create_journal",
            "read",
            "replace",
            "apply",
            "diff",
            "history",
            "archive",
            "restore",
            "set_recall",
            "list",
        ),
    ),
    (
        "writing",
        WritingOperations,
        (
            "journalize",
            "read_draft",
            "recover",
            "propose_edit",
            "rewrite",
            "reflect",
        ),
    ),
    ("annotations", AnnotationOperations, ("read", "read_proposal", "recover")),
    ("concepts", ConceptOperations, ("create", "read", "find", "revise")),
    ("feedback", FeedbackOperations, ("record", "record_query", "read", "list", "resolve")),
    ("maintenance", MaintenanceOperations, ("request_annotation",)),
    ("configuration", ConfigurationOperations, ("unset", "remove_resource")),
    ("retrieval", CognitiveRetrieval, ("query", "refresh")),
    ("namespaces", NamespaceOperations, ("create", "list", "rename", "select")),
    (
        "wave",
        WaveOperations,
        (
            "build",
            "inspect",
            "neighborhood",
            "query",
            "read",
            "segment",
        ),
    ),
)

ACTIONS = {
    f"{scope}.{name}": Action(scope, cast(Callable[..., Awaitable[object]], getattr(owner, name)))
    for scope, owner, names in _METHODS
    for name in names
}


async def _cleanup_wave(
    runtime: Runtime, keep_recent: int, retain: Sequence[str]
) -> dict[str, object]:
    refs = tuple([(await runtime.wave.describe(value)).ref for value in retain])
    return await runtime.wave.cleanup(keep_recent=keep_recent, retain=refs)


ACTIONS["wave.cleanup_named"] = Action("runtime", _cleanup_wave)


async def _adopt_draft(
    records: RecordOperations,
    ref: str,
    markdown: str | None,
    source_changes: SourceChanges | None,
    title: str | None,
    fork: bool,
    after: AfterWrite,
) -> RecordOutcome:
    if markdown is None and source_changes is not None:
        raise NousError(
            "input.override_required",
            "Source changes require an explicit complete Markdown override",
        )
    override = (
        DraftOverride(markdown=markdown, source_changes=source_changes)
        if markdown is not None
        else None
    )
    return await records.adopt(ref, override=override, title=title, fork=fork, after=after)


ACTIONS["records.adopt_draft"] = Action("records", _adopt_draft)


async def _begin_edit(
    runtime: Runtime, ref: str, namespace: str | None, after: AfterWrite
) -> EditSession:
    original = await (await runtime.cognition(namespace)).records.read(ref)
    return EditSession.create(runtime.paths, original, after)


ACTIONS["records.begin_edit"] = Action("runtime", _begin_edit)


async def _resume_edit(runtime: Runtime, path: Path, kind: str) -> EditSession:
    descriptor = path.with_suffix(".json")
    access = current_input_access.get()
    descriptor = await access.resolve(descriptor) if access is not None else descriptor.resolve()
    try:
        data = await read_input(runtime, InputSpec(kind="file", path=str(descriptor)))
    except OSError as error:
        if isinstance(error, TimeoutError):
            raise
        raise NousError(
            "input.edit_session_invalid",
            "Cannot read the edit recovery session.",
            details={"session": str(descriptor)},
            cause=error,
        ) from error
    return EditSession.resume(descriptor, data, runtime.descriptor.instance_id, kind)


async def _read_edit(runtime: Runtime, path: Path) -> str:
    return (await read_input(runtime, InputSpec(kind="file", path=str(path)))).decode("utf-8")


ACTIONS["records.resume_edit"] = Action("runtime", _resume_edit)
ACTIONS["records.read_edit"] = Action("runtime", _read_edit)


async def _annotate(
    annotations: AnnotationOperations,
    ref: str,
    namespace: str | None,
    proposal: AnnotationProposal | None,
    proposal_result: str | None,
    known_concepts: Mapping[str, ConceptTarget] | None,
    bindings: object,
    draft_only: bool,
    task: str | None,
    activate_for_current: bool,
) -> AnnotationProposalView | AnnotationView:
    if bindings is not None and (
        not isinstance(bindings, dict)
        or any(
            not isinstance(key, str) or (value is not None and not isinstance(value, str))
            for key, value in cast(dict[object, object], bindings).items()
        )
    ):
        raise NousError(
            "input.invalid_bindings",
            "Bindings must map local Concept keys to exact refs or null for new",
        )
    return await annotations.annotate(
        ref,
        namespace=namespace,
        proposal=proposal,
        proposal_result=proposal_result,
        known_concepts=known_concepts,
        bindings=cast(dict[str, str | None] | None, bindings),
        draft_only=draft_only,
        task=task,
        activate_for_current=activate_for_current,
    )


ACTIONS["annotations.from_input"] = Action("annotations", _annotate)


async def _set_configuration(
    configuration: ConfigurationOperations, path: str, value: object
) -> dict[str, object]:
    return await configuration.update({path: value})


ACTIONS["configuration.set"] = Action("configuration", _set_configuration)


async def _write_schema(
    configuration: ConfigurationOperations,
    path: Path,
    text: str,
    on_write: Callable[[Path, str], Awaitable[None]],
) -> dict[str, object]:
    return await write_schema(path, text, on_write)


async def write_schema(
    path: Path, text: str, on_write: Callable[[Path, str], Awaitable[None]]
) -> dict[str, object]:
    await on_write(path, text)
    return {"file": str(path), "state": "saved"}


ACTIONS["configuration.write_schema"] = Action("configuration", _write_schema)


async def _inspect_configuration(
    configuration: Configuration,
    kind: Literal["catalog", "show", "explain", "validate", "models", "prompt", "time"],
    value: str | None = None,
    saved: bool = False,
    task: str | None = None,
    domain: str = "utc",
    timezone: str | None = None,
    fold: int | None = None,
) -> object:
    if kind == "catalog":
        return configuration.catalog(value if value is not None else "standard")
    if kind == "show":
        return {
            "state": "saved" if saved else "prospective",
            "file": str(configuration.config_file),
            "values": configuration.saved() if saved else configuration.snapshot.to_dict(),
            "version": configuration.snapshot.version,
        }
    if kind == "explain":
        assert value is not None
        return configuration.explain(value)
    if kind == "validate":
        return configuration.validate()
    if kind == "models":
        return {
            "models": {
                key: item.model_dump(mode="json")
                for key, item in configuration.snapshot.values.models.items()
            }
        }
    assert value is not None
    if kind == "time":
        from nous.temporal.values import parse_explicit_time

        if domain not in configuration.snapshot.values.time.domains:
            raise NousError("time.domain_unknown", f"Unknown time domain: {domain}")
        zone = timezone or configuration.snapshot.values.localization.timezone
        parsed = parse_explicit_time(value, domain=domain, timezone=zone, fold=fold)
        return {
            "input": value,
            "domain": domain,
            "timezone": zone,
            "value": parsed.model_dump(mode="json"),
        }
    return (
        PromptCatalog(configuration.config_file.parent, task_catalog())
        .describe(value, task_assignment=task, snapshot=configuration.snapshot)
        .to_dict()
    )


ACTIONS["configuration.inspect"] = Action("configuration_read", _inspect_configuration)


async def _prepare_prompt(configuration: Configuration, prompt: str) -> dict[str, object]:
    catalog = PromptCatalog(configuration.config_file.parent, task_catalog())
    directory = catalog.editable_directory(prompt, snapshot=configuration.snapshot)
    return {"directory": str(directory), "templates": list(catalog.get(prompt).templates)}


async def _prompt_context(configuration: Configuration, prompt: str) -> None:
    PromptCatalog(configuration.config_file.parent, task_catalog()).write_context(
        prompt, snapshot=configuration.snapshot
    )


ACTIONS["prompt.prepare_edit"] = Action("configuration_read", _prepare_prompt)
ACTIONS["prompt.write_context"] = Action("configuration_read", _prompt_context)


async def _runtime_check(runtime: Runtime) -> dict[str, object]:
    return await runtime.check()


ACTIONS["runtime.check"] = Action("runtime", _runtime_check)

for _name in (
    "model_discover",
    "model_probe",
    "model_run",
    "model_recover",
    "prompt_preview",
    "content_add",
    "content_read",
    "content_export",
    "media_inspect",
    "media_analyze",
    "media_recover",
    "result_read",
    "embedding_probe",
    "embedding_run",
):
    ACTIONS[f"tools.{_name}"] = Action(
        "runtime", cast(Callable[..., Awaitable[object]], getattr(application_tools, _name))
    )

_EVENTS = {
    "writing.journalize": {"on_delta"},
    "tools.model_run": {"on_event"},
    "tools.content_export": {"on_export"},
    "configuration.write_schema": {"on_write"},
}


async def perform(
    runtime: Runtime,
    name: str,
    namespace: str | None,
    args: Sequence[object],
    kwargs: Mapping[str, object],
    *,
    events: Mapping[str, Callable[..., object]] | None = None,
    inputs: Sequence[InputParameter] = (),
    read_stream: Callable[[], Awaitable[str]] | None = None,
) -> object:
    action = ACTIONS.get(name)
    if action is None:
        raise NousError("operation.unknown", "Unknown application operation")
    if set(events or ()) - _EVENTS.get(name, set()):
        raise NousError("operation.invalid_event", "The operation does not support this event")
    async with runtime.execution.request():
        if action.scope == "runtime":
            owner = runtime
        elif action.scope == "configuration_read":
            owner = Configuration(InstanceLocator(runtime.paths.config).config_file)
        elif action.scope in {"wave", "namespaces", "maintenance", "configuration"}:
            owner = {
                "wave": runtime.wave,
                "namespaces": runtime.namespaces,
                "maintenance": runtime.maintenance,
                "configuration": runtime.configuration,
            }[action.scope]
        else:
            cognition = await runtime.cognition(namespace)
            owners = {
                "records": cognition.records,
                "writing": cognition.writing,
                "annotations": cognition.annotations,
                "concepts": cognition.concepts,
                "feedback": cognition.feedback,
                "retrieval": cognition.retrieval,
            }
            owner = owners[action.scope]
        bound = cast(Callable[..., Awaitable[object]], MethodType(action.method, owner))
        checked = validate_call(config=ConfigDict(arbitrary_types_allowed=True))(bound)
        values = await bind_inputs(runtime, dict(kwargs), tuple(inputs), read_stream)
        value = await checked(*args, **values, **(events or {}))
    if name == "runtime.check":
        status = cast(dict[str, object], value)
        status.update(
            state=runtime.state,
            execution=runtime.execution.status(),
            in_flight=runtime.execution.in_flight,
        )
    return value


async def perform_configuration(
    configuration: ConfigurationOperations | Configuration,
    name: str,
    args: Sequence[object],
    kwargs: Mapping[str, object],
    inputs: Sequence[InputParameter],
    events: Mapping[str, Callable[..., object]] | None = None,
    read_stream: Callable[[], Awaitable[str]] | None = None,
) -> object:
    action = ACTIONS.get(name)
    if action is None or action.scope not in {"configuration", "configuration_read"}:
        raise NousError("operation.unknown", "Unknown configuration operation")
    timeout = configuration.snapshot.values.execution.default_timeout_seconds
    async with asyncio.timeout(timeout):
        values = await bind_inputs(None, dict(kwargs), tuple(inputs), read_stream)
        bound = cast(Callable[..., Awaitable[object]], MethodType(action.method, configuration))
        checked = validate_call(config=ConfigDict(arbitrary_types_allowed=True))(bound)
        return await checked(*args, **values, **(events or {}))


def encode(value: object) -> object:
    return to_jsonable_python(value, round_trip=True)
