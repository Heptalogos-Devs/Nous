"""Static task semantics, source-key validation and interpretation of named outputs."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, cast

from pydantic import BaseModel, ConfigDict, JsonValue

from nous.common.errors import NousError
from nous.content.annotations import AnnotationProposal, AnnotationSet
from nous.content.contracts import Artifact, InputGroup, InputPart, OutputContent, SourceSelection
from nous.media.contracts import MediaPlan, PreparedBatch, PreparedSource

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore

    from .contracts import ReadingInput


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
    id: str
    output_type: type[BaseModel] | None = None
    source_field: str | None = None
    selected_passages: bool = False
    input_mode: str = "joint"
    preserve_source_keys: bool = False

    async def prepare_sources(
        self, originals: Sequence[tuple[Artifact, SourceSelection]], artifacts: ArtifactStore
    ) -> tuple[MediaPlan, ...] | None:
        return None

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
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


class JournalDraftTask(TaskDefinition):
    """Own the writing task's explicit no-material result and body interpretation."""

    def __init__(self) -> None:
        super().__init__("journal-draft")

    @staticmethod
    def body(output: OutputContent | None) -> str | None:
        if output is None:
            raise NousError("model.invalid_output", "Writing task returned no Markdown output")
        text = output.data.decode("utf-8")
        if not text.strip():
            raise NousError("model.empty_output", "The provider returned no task output")
        return None if text.strip() == "!NO_MATERIAL!" else text


class ConceptAnnotationTask(TaskDefinition):
    """Prepare and interpret an annotation against one complete immutable Markdown body."""

    def __init__(self) -> None:
        super().__init__("concept-annotation", AnnotationProposal, preserve_source_keys=True)

    async def prepare_sources(
        self, originals: Sequence[tuple[Artifact, SourceSelection]], artifacts: ArtifactStore
    ) -> tuple[MediaPlan, ...]:
        if len(originals) != 1 or originals[0][1] != SourceSelection():
            raise NousError(
                "model.annotation_body", "Concept annotation requires one complete immutable body"
            )
        artifact, chosen = originals[0]
        view = await artifacts.text_view(artifact.ref)
        sources = tuple(
            PreparedSource(
                block.key,
                artifact.display_name,
                text=block.text,
                group=InputGroup(
                    key=block.key,
                    name=artifact.display_name,
                    parts=(
                        InputPart(
                            content=block.selection,
                            representation="text",
                            media_type=artifact.media_type,
                            source_media_type=artifact.media_type,
                        ),
                    ),
                ),
            )
            for block in view.blocks
            if block.parent_key is None
        )
        batch = PreparedBatch(
            sources,
            tuple(source.group for source in sources if source.group),
            chosen,
            {
                "body_ref": artifact.ref.model_dump(mode="json"),
                "parser_profile": view.parser_profile,
            },
        )
        return (MediaPlan(artifact, chosen, (batch,), batch.coverage),)

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        proposal = AnnotationProposal.model_validate(value)
        proposal.validate_references({source.key for source in reading.sources})
        return proposal.model_dump(mode="json")

    async def outputs(
        self,
        value: JsonValue,
        reading: ReadingInput,
        artifacts: ArtifactStore,
        *,
        structured: bool,
        producer: dict[str, JsonValue] | None = None,
    ) -> dict[str, OutputContent]:
        outputs = await super().outputs(
            value, reading, artifacts, structured=structured, producer=producer
        )
        bodies = {
            part.content.artifact.id: part.content.artifact
            for source in reading.sources
            if source.group
            for part in source.group.parts
        }
        if len(bodies) != 1:
            raise NousError("model.annotation_body", "Concept annotation requires one exact body")
        body = next(iter(bodies.values()))
        view = await artifacts.text_view(body)
        proposal = AnnotationProposal.model_validate(value)
        proposal.validate_references(
            {block.key for block in view.blocks if block.parent_key is None}
        )
        annotation = AnnotationSet(
            **proposal.model_dump(),
            body_ref=body,
            parser_profile=view.parser_profile,
            annotation_revision=hashlib.sha256(outputs["output"].data).hexdigest(),
            producer=producer or {},
        )
        return {
            "output": OutputContent(
                data=annotation.model_dump_json(indent=2).encode(),
                media_type="application/json",
                display_name="annotation.json",
            )
        }


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
            TaskDefinition("media-description"),
            TaskDefinition("information-extraction", ExtractedInformation, "source_keys"),
            TaskDefinition("passage-selection", PassageSelection, "keys", True),
            TaskDefinition("custom-reading"),
            TaskDefinition("transcription"),
            JournalDraftTask(),
            ConceptAnnotationTask(),
        )
    )
