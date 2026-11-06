"""Deterministic retrieval text with exact original-content provenance."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Sequence
from dataclasses import dataclass

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.common.operations import record_completed

from .citations import citation_spans, project_citation_slice, project_citations
from .contracts import ContentSelection, InputGroup, InputPart, OutputContent, SourceSelection
from .mappings import TextOriginMapping
from .results import ResultStore


@dataclass(frozen=True)
class SemanticText:
    content: ContentSelection
    text: str


@dataclass(frozen=True)
class TextProjection:
    source: ContentSelection
    original: str
    projected: str

    @property
    def cache_key(self) -> str:
        return hashlib.sha256(
            json.dumps(
                {
                    "source": self.source.model_dump(mode="json"),
                    "text": self.projected,
                    "mapping": "source_link_labels",
                },
                sort_keys=True,
            ).encode()
        ).hexdigest()


def _projection_mappings(item: TextProjection, source: ContentSelection) -> list[JsonValue]:
    """Publish spans from the CommonMark transform, without searching projected text."""
    raw = item.original.encode("utf-8")
    output_length = len(item.projected.encode("utf-8"))
    offset, source_end = source.selection.start_byte, source.selection.end_byte
    if offset is None or source_end is None or source_end - offset != len(raw):
        raise NousError(
            "content.invalid_projection", "Projection input differs from its exact source range"
        )
    mappings = [
        TextOriginMapping(
            start_byte=0,
            end_byte=output_length,
            origins=(source,),
            mode="whole",
            output_name="text",
        )
    ]
    spans = citation_spans(item.original)
    if project_citation_slice(raw, spans, 0, len(raw)) != item.projected:
        return [value.model_dump(mode="json") for value in mappings]
    cursor = position = 0
    for low, high, label in (*spans, (len(raw), len(raw), "")):
        if low > cursor:
            size = low - cursor
            mappings.append(
                TextOriginMapping(
                    start_byte=position,
                    end_byte=position + size,
                    origins=(
                        ContentSelection(
                            artifact=source.artifact,
                            selection=SourceSelection(
                                start_byte=offset + cursor, end_byte=offset + low
                            ),
                        ),
                    ),
                    mode="identity",
                    output_name="text",
                )
            )
            position += size
        size = len(label.encode("utf-8"))
        if size:
            mappings.append(
                TextOriginMapping(
                    start_byte=position,
                    end_byte=position + size,
                    origins=(
                        ContentSelection(
                            artifact=source.artifact,
                            selection=SourceSelection(
                                start_byte=offset + low, end_byte=offset + high
                            ),
                        ),
                    ),
                    mode="atomic",
                    output_name="text",
                )
            )
            position += size
        cursor = high
    return [value.model_dump(mode="json") for value in mappings]


async def semantic_text_many(
    requests: Sequence[TextProjection], results: ResultStore
) -> tuple[SemanticText, ...]:
    """Reuse or adopt projections together; original bytes keep their own identity."""
    changed = {item.cache_key: item for item in requests if item.projected != item.original}
    if not changed:
        return tuple(SemanticText(item.source, item.original) for item in requests)
    async with results.database.read() as tx:
        adopted = await results.cached_outputs_in(
            tx, "content.semantic_text", tuple(changed), output="text"
        )
    pending = [key for key in changed if key not in adopted]
    # Bound transaction size while sharing metadata reads and commits across real inputs.
    for start in range(0, len(pending), 100):
        keys = pending[start : start + 100]
        artifacts = await results.artifacts.get_many([changed[key].source.artifact for key in keys])
        normalized = await results.artifacts.normalize_selections(
            [changed[key].source for key in keys]
        )
        mappings = [
            await results.artifacts.content.execution.blocking(
                _projection_mappings, changed[key], source
            )
            for key, source in zip(keys, normalized, strict=True)
        ]
        prepared = [
            await results.prepare_outputs(
                {
                    "text": OutputContent(
                        data=changed[key].projected.encode("utf-8"),
                        media_type="text/plain",
                        display_name="semantic-text.txt",
                    )
                }
            )
            for key in keys
        ]
        async with results.database.write() as tx:
            # Another build can have adopted a projection after the first read.
            existing = await results.cached_outputs_in(
                tx, "content.semantic_text", keys, output="text"
            )
            adopted.update(existing)
            for key, artifact, output, mapping in zip(
                keys, artifacts, prepared, mappings, strict=True
            ):
                if key in adopted:
                    continue
                source = changed[key].source
                result = await results.adopt_in(
                    tx,
                    operation="content.semantic_text",
                    producer={"kind": "deterministic"},
                    inputs=(
                        InputGroup(
                            key="S1",
                            slot_key="S1",
                            name="Original text",
                            parts=(
                                InputPart(
                                    content=source,
                                    representation="original_text",
                                    media_type=artifact.media_type,
                                ),
                            ),
                        ),
                    ),
                    prepared_outputs=output,
                    parameters={"cache_key": key, "representations": {"text": {"kind": "text"}}},
                    coverage={"source": source.model_dump(mode="json"), "text_mappings": mapping},
                )
                artifact_ref = next(item.artifact for item in result.outputs if item.name == "text")
                adopted[key] = (result.ref, artifact_ref)
    for ref, _ in adopted.values():
        record_completed(ref.identifier)
    return tuple(
        SemanticText(
            ContentSelection(artifact=adopted[item.cache_key][1]),
            item.projected,
        )
        if item.projected != item.original
        else SemanticText(item.source, item.original)
        for item in requests
    )


async def semantic_text(
    source: ContentSelection,
    results: ResultStore,
    *,
    original: str | None = None,
    projected: str | None = None,
) -> SemanticText:
    """A single projection uses the same batched adoption path."""
    if original is None:
        original = await results.artifacts.read_selection(source)
    if projected is None:
        projected = project_citations(original)
    return (await semantic_text_many((TextProjection(source, original, projected),), results))[0]
