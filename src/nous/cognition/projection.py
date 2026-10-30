"""Text and explicit-reference projections of typed cognitive content."""

from collections.abc import Iterator, Sequence
from typing import cast

from nous_wave.contracts import (
    CognitiveBody,
    CognitiveRef,
    EntityBody,
    EvidenceLink,
    MediaReference,
    MemorySemanticsBody,
)
from pydantic import BaseModel


def body_text(body: CognitiveBody | None) -> str:
    if body is None:
        return ""
    if isinstance(body, MemorySemanticsBody):
        return "\n".join([*(item.text for item in body.interpretations), *body.qualifications])
    if isinstance(body, EntityBody):
        return "\n".join((body.label, body.profile_text, *body.effective_aliases))
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


def media_references_in(value: object) -> Iterator[MediaReference]:
    """Media dependencies are typed identities; metadata strings never become references."""
    if isinstance(value, MediaReference):
        yield value
    elif isinstance(value, BaseModel):
        for field in type(value).model_fields:
            yield from media_references_in(getattr(value, field))
    elif isinstance(value, (list, tuple)):
        for child in cast(Sequence[object], value):
            yield from media_references_in(child)


def evidence_in(value: object) -> Iterator[EvidenceLink]:
    """Traverse declared evidence fields, retaining literal metadata as opaque data."""
    if isinstance(value, EvidenceLink):
        yield value
    elif isinstance(value, BaseModel):
        for field in type(value).model_fields:
            yield from evidence_in(getattr(value, field))
    elif isinstance(value, (list, tuple)):
        for child in cast(Sequence[object], value):
            yield from evidence_in(child)
