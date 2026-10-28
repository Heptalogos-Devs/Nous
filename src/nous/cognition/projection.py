"""Text and explicit-reference projections of typed cognitive content."""

from collections.abc import Iterator, Sequence
from typing import cast

from nous_wave.contracts import CognitiveBody, CognitiveRef, MemorySemanticsBody
from pydantic import BaseModel


def body_text(body: CognitiveBody | None) -> str:
    if body is None:
        return ""
    if isinstance(body, MemorySemanticsBody):
        return "\n".join([*(item.text for item in body.interpretations), *body.qualifications])
    value = body.model_dump(mode="json")
    return "\n".join(
        str(value[key])
        for key in (
            "text",
            "summary",
            "label",
            "description",
            "title",
            "narrative",
            "claim",
            "understanding",
            "expression",
            "meaning",
            "relation",
            "facet",
        )
        if value.get(key)
    )


def references_in(value: object) -> Iterator[CognitiveRef]:
    """Only typed reference fields carry owner identity; opaque metadata stays literal."""
    if isinstance(value, CognitiveRef):
        yield value
    elif isinstance(value, BaseModel):
        for field in type(value).model_fields:
            yield from references_in(getattr(value, field))
    elif isinstance(value, (list, tuple)):
        for child in cast(Sequence[object], value):
            yield from references_in(child)
