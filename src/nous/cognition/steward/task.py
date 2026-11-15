"""Interpret a reading brief and retain item-level source relations."""

import json
from collections.abc import Sequence
from typing import TYPE_CHECKING

from pydantic import JsonValue

from nous.content.contracts import (
    MaterialSource,
    OutputContent,
    OutputMapping,
    SourceSelection,
)
from nous.models.contracts import ReadingInput
from nous.models.tasks import InterpretedOutputs, NamedOutput, TaskDefinition

from .contracts import ReadingBrief

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore


class ArrangeReadingTask(TaskDefinition):
    def __init__(self) -> None:
        super().__init__(
            "arrange-reading",
            output_type=ReadingBrief,
            named_outputs=(
                NamedOutput("reading", "Structured reading brief."),
                NamedOutput(
                    "markdown", "Readable arrangement with evidence keys.", role="auxiliary"
                ),
            ),
        )

    @staticmethod
    def _sources(reading: ReadingInput) -> dict[str, MaterialSource]:
        sources: dict[str, MaterialSource] = {}
        for part in reading.sources:
            if part.group is None or part.material_source is None:
                continue
            key, source = part.group.slot_key, part.material_source
            previous = sources.get(key)
            if previous is None:
                sources[key] = source.model_copy(update={"key": key})
            else:
                sources[key] = previous.model_copy(
                    update={
                        "selections": tuple(
                            dict.fromkeys((*previous.selections, *source.selections))
                        )
                    }
                )
        return sources

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        brief = ReadingBrief.model_validate(value)
        cited = {key for item in brief.items for key in item.evidence_keys}
        unknown = cited - self._sources(reading).keys()
        if unknown:
            raise ValueError(f"Unknown evidence keys: {sorted(unknown)}")
        return brief.model_dump(mode="json")

    async def outputs(
        self,
        value: JsonValue,
        reading: ReadingInput,
        artifacts: ArtifactStore,
        *,
        structured: bool,
        producer: dict[str, JsonValue] | None = None,
    ) -> InterpretedOutputs:
        brief = ReadingBrief.model_validate(value)
        sources = self._sources(reading)
        data = json.dumps(brief.model_dump(mode="json"), ensure_ascii=False, indent=2).encode()
        chunks: list[bytes] = []
        mappings: list[OutputMapping] = []
        if brief.title:
            chunks.append(f"# {brief.title}\n\n".encode())
        for item in brief.items:
            text = item.markdown.encode()
            start = sum(map(len, chunks))
            chunks.append(text)
            origins = tuple(
                dict.fromkeys(
                    origin for key in item.evidence_keys for origin in sources[key].selections
                )
            )
            if text and origins:
                mappings.append(
                    OutputMapping(
                        output_name="markdown",
                        selection=SourceSelection(start_byte=start, end_byte=start + len(text)),
                        relation="derived",
                        origins=origins,
                    )
                )
            if item.evidence_keys:
                chunks.append(
                    (
                        " " + " ".join(f"[{key}](source:{key})" for key in item.evidence_keys)
                    ).encode()
                )
            chunks.append(b"\n\n")
        all_origins = tuple(
            dict.fromkeys(origin for mapping in mappings for origin in mapping.origins)
        )
        if data and all_origins:
            mappings.append(
                OutputMapping(
                    output_name="reading",
                    selection=SourceSelection(start_byte=0, end_byte=len(data)),
                    relation="derived",
                    origins=all_origins,
                )
            )
        return InterpretedOutputs(
            outputs={
                "reading": OutputContent(
                    data=data, media_type="application/json", display_name="reading.json"
                ),
                "markdown": OutputContent(data=b"".join(chunks), display_name="reading.md"),
            },
            mappings=tuple(mappings),
            material_sources=tuple(sources.values()),
            value=brief,
        )


def steward_tasks() -> Sequence[TaskDefinition]:
    return (ArrangeReadingTask(),)
