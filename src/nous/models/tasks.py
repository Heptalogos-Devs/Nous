"""Static task semantics, source-key validation and interpretation of named outputs."""

from __future__ import annotations

import json
import math
from collections.abc import Mapping, Sequence
from dataclasses import MISSING, dataclass, fields
from typing import TYPE_CHECKING, Literal, cast

from pydantic import BaseModel, ConfigDict, JsonValue, TypeAdapter

from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    ContentSelection,
    InputGroup,
    NamedSelection,
    OutputContent,
    OutputMapping,
    OutputRole,
    SourceFacts,
    SourceSelection,
    is_text_media_type,
)
from nous.media.contracts import MediaPlan, PreparedSource

from .contracts import ReadingInput
from .evaluation_contracts import EvaluationResponse, EvaluationScale, RankResponse

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


def schema_semantics(schema: dict[str, JsonValue]) -> dict[str, JsonValue]:
    """Drop documentation annotations while retaining the actual validation contract."""
    maps = {"$defs", "definitions", "properties", "patternProperties", "dependentSchemas"}
    singles = {
        "items",
        "additionalProperties",
        "unevaluatedProperties",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
    }
    arrays = {"allOf", "anyOf", "oneOf", "prefixItems"}
    ignored = {"title", "description", "examples", "example", "$comment"}
    result: dict[str, JsonValue] = {}
    for key, value in schema.items():
        if key in ignored:
            continue
        if key in maps and isinstance(value, dict):
            result[key] = {
                name: schema_semantics(item) if isinstance(item, dict) else item
                for name, item in value.items()
            }
        elif key in singles and isinstance(value, dict):
            result[key] = schema_semantics(value)
        elif key in arrays and isinstance(value, list):
            result[key] = [
                schema_semantics(item) if isinstance(item, dict) else item for item in value
            ]
        else:
            result[key] = value
    return result


@dataclass(frozen=True)
class NamedOutput:
    """A named task output and its user-facing role and representation."""

    name: str
    description: str
    role: OutputRole = "primary"
    representation_kind: str | None = None


@dataclass(frozen=True)
class InterpretedOutputs:
    """Task-owned outputs and the precise relations known at interpretation time."""

    outputs: Mapping[str, OutputContent]
    mappings: tuple[OutputMapping, ...] = ()
    selections: tuple[NamedSelection, ...] = ()


def interpreted_outputs(
    outputs: Mapping[str, OutputContent],
    reading: ReadingInput,
    *,
    selections: tuple[NamedSelection, ...] = (),
) -> InterpretedOutputs:
    """Record broad derivation when a task does not declare finer relations."""
    origins = tuple(
        dict.fromkeys(
            origin
            for source in reading.sources
            if source.group is not None
            for part in source.group.parts
            for origin in (tuple(item.origin for item in part.origins) or (part.content,))
        )
    )
    return InterpretedOutputs(
        outputs,
        tuple(
            OutputMapping(
                output_name=name,
                selection=SourceSelection(start_byte=0, end_byte=len(output.data)),
                relation="derived",
                origins=origins,
            )
            for name, output in outputs.items()
            if output.data and origins and output.role != "diagnostic"
        ),
        selections,
    )


@dataclass(frozen=True)
class TaskDefinition:
    """Define a language task's input, validation and named output semantics.

    Subclasses can prepare task-specific source plans, validate the model's
    value against the actual reading, and interpret it into outputs. Source keys
    refer to prepared sources in that reading. Output roles and representations
    are declared once alongside their names.
    """

    id: str
    output_type: type[BaseModel] | None = None
    input_mode: str = "joint"
    preserve_source_keys: bool = False
    required_capabilities: tuple[str, ...] = ("text",)
    named_outputs: tuple[NamedOutput, ...] = (
        NamedOutput("output", "Task response: text or the declared structured output."),
    )
    required_roles: tuple[str, ...] = ()
    text_input_mode: Literal["lossless", "structured"] = "lossless"

    def interpretation_contract(self) -> dict[str, JsonValue]:
        """The installed semantics needed to interpret a saved response."""
        return {
            "id": self.id,
            "output_schema": schema_semantics(
                cast(dict[str, JsonValue], self.output_type.model_json_schema())
            )
            if self.output_type
            else None,
            "named_outputs": [
                {
                    "name": item.name,
                    "role": item.role,
                    "representation_kind": item.representation_kind,
                }
                for item in self.named_outputs
            ],
        }

    def supports_schema(self, schema: JsonValue) -> bool:
        return (
            self.output_type is None
            or isinstance(schema, dict)
            and (
                schema_semantics(cast(dict[str, JsonValue], self.output_type.model_json_schema()))
                == schema_semantics(schema)
            )
        )

    def execution_contract(self) -> dict[str, JsonValue]:
        """Return only semantics that affect execution and interpretation."""
        return {
            **self.interpretation_contract(),
            "input_mode": self.input_mode,
            "text_input_mode": self.text_input_mode,
            "preserve_source_keys": self.preserve_source_keys,
            "required_capabilities": list(self.required_capabilities),
            "required_roles": list(self.required_roles),
        }

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
        structured = self.output_type is not None or self.id == "custom-reading"
        outputs: dict[str, JsonValue] = {
            item.name: {
                "description": item.description,
                "role": item.role,
                "representation_kind": item.representation_kind,
            }
            for item in self.named_outputs
        }
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
            "input_mode": self.input_mode,
            "text_input_mode": self.text_input_mode,
            "preserve_source_keys": self.preserve_source_keys,
            "required_capabilities": capabilities,
            "output_strategies": ["native", "tool", "prompted"] if structured else ["text"],
            "named_outputs": outputs,
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

    def independent_ranges(
        self, source: ContentSelection, facts: SourceFacts | None
    ) -> tuple[SourceSelection, ...]:
        """Keep each selected range intact unless this task declares domain segmentation."""
        return (source.selection,)

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
        """Validate the task's declared output type."""
        if self.output_type is not None:
            value = self.output_type.model_validate(value).model_dump(mode="json")
        return value

    async def outputs(
        self,
        value: JsonValue,
        reading: ReadingInput,
        artifacts: ArtifactStore,
        *,
        structured: bool,
        producer: dict[str, JsonValue] | None = None,
    ) -> InterpretedOutputs:
        """Interpret a validated value into named bytes for later result adoption.

        Args:
            value: Validated response value.
            reading: Actual prepared sources used by the successful request.
            artifacts: Borrowed store for resolving exact source selections.
            structured: Serialize the response as JSON when true, or UTF-8 text.
            producer: Optional producer metadata available to task extensions.

        Returns:
            Named response bytes for this task's output declaration.
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
        return interpreted_outputs({"output": output}, reading)

    def describe_outputs(self, outputs: Mapping[str, OutputContent]) -> dict[str, OutputContent]:
        """Apply this task's output declarations to the completed bytes."""
        declarations = {item.name: item for item in self.named_outputs}
        if len(declarations) != len(self.named_outputs) or set(outputs) - declarations.keys():
            raise NousError(
                "model.invalid_output", "Task returned an undeclared or duplicate output"
            )
        return {
            name: value.model_copy(
                update={
                    "role": declarations[name].role,
                    "representation_kind": declarations[name].representation_kind,
                }
            )
            for name, value in outputs.items()
        }


def _validate_source_keys(keys: Sequence[str], reading: ReadingInput) -> None:
    known = {source.key for source in reading.sources}
    if set(keys) - known:
        raise ValueError(
            f"Unknown source keys: {sorted(set(keys) - known)}; "
            f"use the complete keys {sorted(known)}"
        )


class InformationExtractionTask(TaskDefinition):
    def __init__(self) -> None:
        super().__init__("information-extraction", output_type=ExtractedInformation)

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        result = ExtractedInformation.model_validate(value)
        _validate_source_keys([key for item in result.items for key in item.source_keys], reading)
        return result.model_dump(mode="json")


class PassageSelectionTask(TaskDefinition):
    def __init__(self) -> None:
        super().__init__(
            "passage-selection",
            output_type=PassageSelection,
            named_outputs=(
                NamedOutput("output", "Selected source keys."),
                NamedOutput("passages", "Exact selected source passages.", role="auxiliary"),
            ),
        )

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        result = PassageSelection.model_validate(value)
        _validate_source_keys(result.keys, reading)
        return result.model_copy(update={"keys": list(dict.fromkeys(result.keys))}).model_dump(
            mode="json"
        )

    async def outputs(
        self,
        value: JsonValue,
        reading: ReadingInput,
        artifacts: ArtifactStore,
        *,
        structured: bool,
        producer: dict[str, JsonValue] | None = None,
    ) -> InterpretedOutputs:
        interpreted = await super().outputs(
            value, reading, artifacts, structured=structured, producer=producer
        )
        outputs = dict(interpreted.outputs)
        source_map = {source.key: source for source in reading.sources}
        excerpts: list[dict[str, JsonValue]] = []
        selections: list[NamedSelection] = []
        for key in PassageSelection.model_validate(value).keys:
            source = source_map[key]
            if source.group is None:
                raise NousError(
                    "model.invalid_source", "Selected source has no exact content reference"
                )
            selected = tuple(part.content for part in source.group.parts)
            selections.append(NamedSelection(key=key, sources=selected))
            for part in source.group.parts:
                artifact = await artifacts.get(part.content.artifact)
                if is_text_media_type(part.media_type or artifact.media_type):
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
        origins = tuple(
            dict.fromkeys(item for selection in selections for item in selection.sources)
        )
        mappings = tuple(
            OutputMapping(
                output_name=name,
                selection=SourceSelection(start_byte=0, end_byte=len(output.data)),
                relation="derived",
                origins=origins,
            )
            for name, output in outputs.items()
            if output.data and origins
        )
        return InterpretedOutputs(outputs, mappings, tuple(selections))


class CandidateAssessmentTask(TaskDefinition):
    """Interpret a structured assessment using keys of actual candidate inputs."""

    def __init__(self, task_id: str, output_type: type[BaseModel]) -> None:
        super().__init__(task_id, output_type=output_type)

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        assert self.output_type is not None
        result = self.output_type.model_validate(value)
        if not isinstance(result, (EvaluationResponse, RankResponse)):
            raise ValueError("Unknown candidate assessment contract")
        keys = [item.key for item in result.items]
        known = {
            source.group.slot_key if source.group is not None else source.key
            for source in reading.sources
        }
        if len(set(keys)) != len(keys) or set(keys) - known:
            raise ValueError("Assessment contains duplicate or unknown candidate keys")
        if isinstance(result, EvaluationResponse):
            instructions = TypeAdapter(dict[str, JsonValue]).validate_json(reading.instructions)
            scale = EvaluationScale.model_validate(instructions["scale"])
            for item in result.items:
                if scale.labels:
                    if not isinstance(item.value, str) or item.value not in scale.labels:
                        raise ValueError(
                            f"Candidate value must use a declared label: {scale.labels}"
                        )
                elif not isinstance(item.value, (float, int)) or not (
                    scale.minimum is not None
                    and scale.maximum is not None
                    and scale.minimum <= item.value <= scale.maximum
                ):
                    raise ValueError("Candidate value must be within the declared numerical scale")
                if item.distribution is not None:
                    weights = item.distribution
                    if (
                        not weights
                        or any(
                            not math.isfinite(value) or not 0 <= value <= 1
                            for value in weights.values()
                        )
                        or abs(sum(weights.values()) - 1) > 0.0001
                        or bool(scale.labels)
                        and bool(set(weights) - set(scale.labels))
                    ):
                        raise ValueError(
                            "Distribution must use declared labels and probabilities summing to one"
                        )
        return result.model_dump(mode="json")


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
            TaskDefinition(
                "media-description",
                named_outputs=(
                    NamedOutput("output", "Media description.", representation_kind="description"),
                ),
            ),
            InformationExtractionTask(),
            PassageSelectionTask(),
            CandidateAssessmentTask("evaluate-candidates", EvaluationResponse),
            CandidateAssessmentTask("rank-candidates", RankResponse),
            TaskDefinition("custom-reading"),
            TaskDefinition(
                "transcription",
                required_capabilities=("audio",),
                named_outputs=(
                    NamedOutput("output", "Transcript.", representation_kind="transcript"),
                ),
            ),
        )
    )
