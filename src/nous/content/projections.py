"""Deterministic retrieval text with exact original-content provenance."""

import hashlib
import json
from dataclasses import dataclass

from nous.common.operations import record_completed

from .citations import project_citations
from .contracts import ContentSelection, InputGroup, InputPart, OutputContent
from .results import ResultStore


@dataclass(frozen=True)
class SemanticText:
    content: ContentSelection
    text: str


async def semantic_text(source: ContentSelection, results: ResultStore) -> SemanticText:
    """Retain plain inputs; adopt a source-link syntax projection with its original selection."""
    original = await results.artifacts.read_selection(source)
    projected = project_citations(original)
    if projected == original:
        return SemanticText(source, original)
    key = hashlib.sha256(
        json.dumps(
            {"source": source.model_dump(mode="json"), "text": projected},
            sort_keys=True,
        ).encode()
    ).hexdigest()
    cached = await results.find_cached("content.semantic_text", key)
    if cached is None:
        artifact = await results.artifacts.get(source.artifact)
        cached = await results.adopt(
            operation="content.semantic_text",
            producer={"kind": "deterministic"},
            inputs=(
                InputGroup(
                    key="S1",
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
            outputs={
                "text": OutputContent(
                    data=projected.encode("utf-8"),
                    media_type="text/plain",
                    display_name="semantic-text.txt",
                )
            },
            parameters={"cache_key": key},
            coverage={"source": source.model_dump(mode="json")},
        )
    record_completed(cached.ref.address)
    output = next(output for output in cached.outputs if output.name == "text")
    return SemanticText(
        ContentSelection(artifact=output.artifact),
        await results.artifacts.read_text(output.artifact),
    )
