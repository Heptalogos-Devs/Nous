"""Static task semantics, source-key validation and interpretation of named outputs."""

from __future__ import annotations

import json
from collections.abc import Sequence
from dataclasses import MISSING, dataclass, fields
from typing import TYPE_CHECKING, Literal, cast

from pydantic import BaseModel, ConfigDict, JsonValue

from nous.common.errors import NousError
from nous.content.contracts import Artifact, InputGroup, OutputContent, SourceSelection
from nous.media.contracts import MediaPlan, PreparedSource

from .contracts import ReadingInput

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore


class InformationItem(BaseModel):
    model_config = ConfigDict(extra="forbid")
    text: str
    source_keys: list[str]


class ExtractedInformation(BaseModel):
    model_config = ConfigDict(extra="forbid")
    title: str
    items: list[InformationItem]


class PassageSelection(BaseModel):
    model_config = ConfigDict(extra="forbid")
    keys: list[str]


@dataclass(frozen=True)
class TaskDefinition:
    """Define a language task's input, validation and named output semantics.

    Subclasses can prepare task-specific source plans, validate the model's
    value against the actual reading, and interpret it into outputs. Source keys
    refer to prepared sources in that reading. ``program_outputs`` identifies
    named outputs computed by task code for result provenance; it is not a request
    for the model to generate those outputs.
    """

    id: str
    output_type: type[BaseModel] | None = None
    source_field: str | None = None
    selected_passages: bool = False
    input_mode: str = "joint"
    preserve_source_keys: bool = False
    required_capabilities: tuple[str, ...] = ("text",)
    named_outputs: tuple[tuple[str, str], ...] = (
        ("output", "Task response: text or the declared structured output."),
    )
    source_representation: str | None = None
    program_outputs: tuple[str, ...] = ()
    required_roles: tuple[str, ...] = ()
    text_input_mode: Literal["lossless", "structured"] = "lossless"

    def describe_contract(self) -> dict[str, JsonValue]:
        """Describe the same input, output and source semantics used by execution."""
        variables: dict[str, JsonValue] = {}
        exposed = ReadingInput().template_values()
        for item in fields(ReadingInput):
            if item.name in exposed:
                variables[item.name] = {
                    "type": str(item.type),
                    "required": item.default is MISSING and item.default_factory is MISSING,
                    "default": exposed[item.name],
                    "description": str(item.metadata.get("description", "")),
                    "dynamic_role": "user",
                }
        output: JsonValue = (
            cast(dict[str, JsonValue], self.output_type.model_json_schema())
            if self.output_type
            else {"provided_by": "caller", "required": True}
            if self.id == "custom-reading"
            else {"type": "string"}
        )
        capabilities: list[JsonValue] = list(self.required_capabilities)
        if self.output_type is not None or self.id == "custom-reading":
            capabilities.append("structured_output")
        outputs = dict(self.named_outputs)
        if self.selected_passages:
            outputs["passages"] = (
                "Exact selected source passages with immutable content references."
            )
        return {
            "id": self.id,
            "input": {
                "type": f"{ReadingInput.__module__}.{ReadingInput.__qualname__}",
                "fields": variables,
                "source_fields": {
                    item.name: {
                        "type": str(item.type),
                        "required": item.default is MISSING and item.default_factory is MISSING,
                    }
                    for item in fields(PreparedSource)
                },
                "source_group": InputGroup.model_json_schema(),
            },
            "variables": variables,
            "output": output,
            "source_field": self.source_field,
            "selected_passages": self.selected_passages,
            "input_mode": self.input_mode,
            "text_input_mode": self.text_input_mode,
            "preserve_source_keys": self.preserve_source_keys,
            "required_capabilities": capabilities,
            "named_outputs": cast(dict[str, JsonValue], outputs),
            "source_representation": self.source_representation,
            "program_outputs": list[JsonValue](self.program_outputs),
            "required_roles": list[JsonValue](self.required_roles),
            "media_capabilities": "Determined from the actual InputPlan representations; no request input supplied.",
        }

    async def prepare_sources(
        self, originals: Sequence[tuple[Artifact, SourceSelection]], artifacts: ArtifactStore
    ) -> tuple[MediaPlan, ...] | None:
        """Optionally plan task-specific representations of the exact inputs.

        Args:
            originals: Accepted artifacts paired with their selected source regions.
            artifacts: Borrowed store for resolving exact bodies and selections.

        Returns:
            None to use ordinary media preparation, or one plan per input in the
            same order. Task-specific plans retain exact input references and
            source keys needed by validation and interpretation.
        """
        return None

    def validate_input(self, reading: ReadingInput) -> None:
        """Require declared source roles in the actual prepared reading.

        Raises:
            NousError: ``model.input_role_required`` when a required role is absent.
        """
        actual = {source.group.role for source in reading.sources if source.group is not None}
        missing = set(self.required_roles) - actual
        if missing:
            raise NousError(
                "model.input_role_required",
                "Task requires declared source roles",
                details={"task_contract": self.id, "missing_roles": sorted(missing)},
            )

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        """Validate structured output and its keys against actual prepared sources.

        Returns:
            The validated JSON value; selected-passage keys are deduplicated in
            first-occurrence order.

        Raises:
            ValueError: A referenced source key is absent from the reading, or
                output validation fails. Execution may use validation feedback
                within its configured repair and attempt budget.
        """
        if self.output_type is not None:
            value = self.output_type.model_validate(value).model_dump(mode="json")
        known = {source.key for source in reading.sources}
        if self.source_field is not None:
            structured = cast(dict[str, JsonValue], value)
            if self.selected_passages:
                keys = cast(list[str], structured[self.source_field])
                structured[self.source_field] = list(dict.fromkeys(keys))
            else:
                keys = [
                    str(key)
                    for item in cast(list[dict[str, JsonValue]], structured["items"])
                    for key in cast(list[str], item[self.source_field])
                ]
            if set(keys) - known:
                raise ValueError(f"Unknown source keys: {sorted(set(keys) - known)}")
        return value

    async def outputs(
        self,
        value: JsonValue,
        reading: ReadingInput,
        artifacts: ArtifactStore,
        *,
        structured: bool,
        producer: dict[str, JsonValue] | None = None,
    ) -> dict[str, OutputContent]:
        """Interpret a validated value into named bytes for later result adoption.

        Args:
            value: Validated response value.
            reading: Actual prepared sources used by the successful request.
            artifacts: Borrowed store for resolving exact source selections.
            structured: Serialize the response as JSON when true, or UTF-8 text.
            producer: Optional producer metadata available to task extensions.

        Returns:
            Named outputs, including ``output`` and, for passage selection,
            exact selected text and content references in ``passages``.

        Raises:
            NousError: ``model.invalid_output`` for non-text unstructured output
                or ``model.invalid_source`` for a passage without an exact group.
        """
        if structured:
            output = OutputContent(
                data=json.dumps(value, ensure_ascii=False, indent=2).encode(),
                media_type="application/json",
                display_name="result.json",
            )
        elif isinstance(value, str):
            output = OutputContent(data=value.encode())
        else:
            raise NousError("model.invalid_output", "Text task returned non-text output")
        outputs = {"output": output}
        if self.selected_passages:
            source_map = {source.key: source for source in reading.sources}
            excerpts: list[dict[str, JsonValue]] = []
            for key in cast(list[str], cast(dict[str, JsonValue], value)["keys"]):
                source = source_map[key]
                if source.group is None:
                    raise NousError(
                        "model.invalid_source", "Selected source has no exact content reference"
                    )
                for part in source.group.parts:
                    artifact = await artifacts.get(part.content.artifact)
                    if (
                        artifact.media_type.startswith("text/")
                        or artifact.media_type == "application/json"
                    ):
                        excerpts.append(
                            {
                                "key": key,
                                "text": await artifacts.read_selection(part.content),
                                "content": part.content.model_dump(mode="json"),
                            }
                        )
            outputs["passages"] = OutputContent(
                data=json.dumps(excerpts, ensure_ascii=False, indent=2).encode(),
                media_type="application/json",
                display_name="passages.json",
            )
        return outputs


class TaskCatalog:
    """Explicit static task contracts shared by prompt loading, preparation and interpretation."""

    def __init__(self, definitions: Sequence[TaskDefinition]) -> None:
        self.definitions = {definition.id: definition for definition in definitions}
        if len(self.definitions) != len(definitions):
            raise ValueError("Task contract IDs must be unique")

    def __contains__(self, task: str) -> bool:
        return task in self.definitions

    def __getitem__(self, task: str) -> TaskDefinition:
        return self.definitions[task]


def builtin_tasks() -> TaskCatalog:
    return TaskCatalog(
        (
            TaskDefinition("reading-summary"),
            TaskDefinition("media-description", source_representation="description"),
            TaskDefinition("information-extraction", ExtractedInformation, "source_keys"),
            TaskDefinition("passage-selection", PassageSelection, "keys", True),
            TaskDefinition("custom-reading"),
            TaskDefinition(
                "transcription",
                required_capabilities=("audio",),
                source_representation="transcript",
            ),
        )
    )
