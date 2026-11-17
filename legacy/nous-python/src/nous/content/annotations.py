"""Concept annotation proposals and exact-body validation shared by tasks and Wave."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator

from nous.common.errors import NousError
from nous.content.contracts import ArtifactRef, ResultRef

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore


class AnnotationValue(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ConceptProposal(AnnotationValue):
    key: str = Field(min_length=1)
    label: str = ""
    description: str = ""
    reuse_key: str | None = None

    @model_validator(mode="after")
    def meaningful(self) -> ConceptProposal:
        if not self.reuse_key and (not self.label.strip() or not self.description.strip()):
            raise ValueError("A new concept requires label and description")
        return self


class OccurrenceProposal(AnnotationValue):
    key: str = Field(min_length=1)
    concept: str
    blocks: list[str] = Field(min_length=1)


class PathProposal(AnnotationValue):
    key: str = Field(min_length=1)
    label: str
    occurrences: list[str] = Field(min_length=1)


class AnnotationProposal(AnnotationValue):
    concepts: list[ConceptProposal]
    occurrences: list[OccurrenceProposal]
    paths: list[PathProposal]

    def validate_references(self, block_keys: set[str]) -> None:
        def unique(items: list[str], label: str) -> set[str]:
            if len(items) != len(set(items)):
                raise ValueError(f"Duplicate {label} keys")
            return set(items)

        concepts = unique([item.key for item in self.concepts], "concept")
        occurrences = unique([item.key for item in self.occurrences], "occurrence")
        unique([item.key for item in self.paths], "path")
        for occurrence in self.occurrences:
            if occurrence.concept not in concepts:
                raise ValueError(f"Unknown concept key: {occurrence.concept}")
            unknown = set(occurrence.blocks) - block_keys
            if unknown:
                raise ValueError(f"Unknown block keys: {sorted(unknown)}")
        for path in self.paths:
            unknown = set(path.occurrences) - occurrences
            if unknown:
                raise ValueError(f"Unknown occurrence keys: {sorted(unknown)}")


class AnnotationSet(AnnotationProposal):
    body_ref: ArtifactRef
    parser_profile: str
    annotation_revision: str
    producer: dict[str, JsonValue] = Field(default_factory=dict)
    result_ref: ResultRef | None = None


async def read_annotation(artifacts: ArtifactStore, ref: ArtifactRef | str) -> AnnotationSet:
    artifact = await artifacts.get(ref)
    try:
        annotation = AnnotationSet.model_validate(
            json.loads(await artifacts.read_text(artifact.ref))
        )
        view = await artifacts.text_view(annotation.body_ref)
        if view.parser_profile != annotation.parser_profile:
            raise ValueError("Annotation parser profile differs from the body view")
        annotation.validate_references(
            {block.key for block in view.blocks if block.parent_key is None}
        )
        return annotation.model_copy(update={"result_ref": artifact.producer_result_ref})
    except ValueError as error:
        raise NousError(
            "model.annotation_invalid",
            "Annotation does not match its exact body",
            details={"problem": str(error)},
        ) from error
