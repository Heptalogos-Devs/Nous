"""Static task semantics, source-key validation and interpretation of named outputs."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from typing import TYPE_CHECKING, cast

from pydantic import BaseModel, ConfigDict, JsonValue

from nous.common.errors import NousError
from nous.content.contracts import OutputContent

from .annotations import AnnotationProposal, AnnotationSet

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

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        if self.output_type is not None:
            value = self.output_type.model_validate(value).model_dump(mode="json")
        if self.id == "concept-annotation":
            AnnotationProposal.model_validate(value).validate_references(
                {source.key for source in reading.sources}
            )
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
        if self.id == "concept-annotation":
            bodies = {
                source.group.parts[0].content.artifact.id
                for source in reading.sources
                if source.group
            }
            if len(bodies) != 1 or not reading.sources or reading.sources[0].group is None:
                raise NousError(
                    "model.annotation_body", "Concept annotation requires one exact body"
                )
            body = reading.sources[0].group.parts[0].content.artifact
            view = await artifacts.text_view(body)
            proposal = AnnotationProposal.model_validate(value)
            proposal.validate_references(
                {block.key for block in view.blocks if block.parent_key is None}
            )
            annotation = AnnotationSet(
                **proposal.model_dump(),
                body_ref=body,
                parser_profile=view.parser_profile,
                annotation_revision=hashlib.sha256(output.data).hexdigest(),
                producer=producer or {},
            )
            outputs["output"] = OutputContent(
                data=annotation.model_dump_json(indent=2).encode(),
                media_type="application/json",
                display_name="annotation.json",
            )
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


TASKS = {
    definition.id: definition
    for definition in (
        TaskDefinition("reading-summary"),
        TaskDefinition("media-description"),
        TaskDefinition("information-extraction", ExtractedInformation, "source_keys"),
        TaskDefinition("passage-selection", PassageSelection, "keys", True),
        TaskDefinition("custom-reading"),
        TaskDefinition("transcription"),
        TaskDefinition("journal-draft"),
        TaskDefinition("concept-annotation", AnnotationProposal),
    )
}
