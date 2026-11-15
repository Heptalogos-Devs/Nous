"""Shared, exact identities and reading values for cognitive material."""

from typing import Literal

from pydantic import Field, model_validator

from nous.cognition.contracts import CognitiveValue, NamespaceSelector
from nous.common.material_refs import RecordRevisionRef, WorkRevisionRef
from nous.content.contracts import (
    ArtifactRef,
    ContentReadView,
    ContentSelection,
    MaterialRef,
    MaterialSource,
    ResultRef,
)
from nous.temporal.values import CognitiveTimes


def material_kind(
    ref: MaterialRef,
) -> Literal["note", "journal", "work", "artifact", "result_output"]:
    if isinstance(ref, RecordRevisionRef):
        return ref.record_ref.kind
    if isinstance(ref, WorkRevisionRef):
        return "work"
    if isinstance(ref, ArtifactRef):
        return "artifact"
    return "result_output"


class MaterialSelection(CognitiveValue):
    material: MaterialRef
    selections: tuple[ContentSelection, ...] = ()


class MaterialRelation(CognitiveValue):
    kind: str = Field(min_length=1)
    target: MaterialRef
    scope: tuple[ContentSelection, ...] = ()
    evidence: tuple[ContentSelection, ...] = ()


class MaterialSection(CognitiveValue):
    key: str
    heading_path: tuple[str, ...] = ()
    selection: ContentSelection
    structural_context: tuple[ContentSelection, ...] = ()


class CognitiveMaterialView(CognitiveValue):
    ref: MaterialRef
    title: str | None = None
    media_type: str | None = None
    body: ContentSelection | None = None
    sections: tuple[MaterialSection, ...] | None = None
    material_sources: tuple[MaterialSource, ...] = ()
    relations: tuple[MaterialRelation, ...] = ()
    times: CognitiveTimes | None = None

    @property
    def kind(self) -> str:
        return material_kind(self.ref)


class MaterialRead(CognitiveValue):
    material: CognitiveMaterialView
    selections: tuple[ContentReadView, ...]
    shared: tuple[ContentSelection, ...] = ()
    sources: dict[str, tuple[ContentReadView, ...]] = Field(default_factory=dict)


class MaterialReadBatch(CognitiveValue):
    readings: tuple[MaterialRead, ...]
    remaining: tuple[MaterialSelection, ...] = ()
    used: int = Field(ge=0)
    unit: Literal["characters", "bytes", "tokens"]


class ReadMaterial(CognitiveValue):
    ref: str | MaterialRef | None = None
    selection: MaterialSelection | None = None
    delivery: str | ResultRef | None = None
    key: str | None = None
    namespace: NamespaceSelector = None
    blocks: tuple[str, ...] = ()
    structure: bool = False
    sources: bool = False

    @model_validator(mode="after")
    def target_present(self) -> ReadMaterial:
        if sum((self.ref is not None, self.selection is not None, self.delivery is not None)) != 1:
            raise ValueError("Choose a reference, selection, or delivery and evidence key")
        if (self.delivery is None) != (self.key is None):
            raise ValueError("Choose a material selection or a delivery and evidence key")
        if self.blocks and self.ref is None:
            raise ValueError("Block keys require a complete material reference")
        return self


class FollowMaterial(CognitiveValue):
    ref: str | MaterialRef
    relation: str | None = None
    limit: int = Field(default=100, ge=1)
    namespace: NamespaceSelector = None


class RelationRead(CognitiveValue):
    ref: MaterialRef
    relations: tuple[MaterialRelation, ...]
