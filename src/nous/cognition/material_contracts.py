"""Shared, exact identities and reading values for cognitive material."""

from pydantic import Field, JsonValue, model_validator

from nous.cognition.concept_contracts import AnnotationView
from nous.cognition.contracts import (
    CognitiveValue,
    NamespaceRef,
    NamespaceSelector,
    RecordRevisionRef,
    SourceBinding,
)
from nous.cognition.work.refs import WorkRevisionRef
from nous.content.contracts import (
    ArtifactRef,
    ContentReadView,
    ContentSelection,
    ResultOutputRef,
    ResultRef,
)
from nous.temporal.values import CognitiveTimes

type MaterialRef = RecordRevisionRef | ArtifactRef | ResultOutputRef | WorkRevisionRef


class MaterialSelection(CognitiveValue):
    material: MaterialRef
    selections: tuple[ContentSelection, ...] = ()


class MaterialSource(CognitiveValue):
    """One task-local source key bound to an exact formal material and ranges."""

    key: str = Field(min_length=1)
    material: MaterialRef
    selections: tuple[ContentSelection, ...] = Field(min_length=1)
    kind: str = Field(default="source", min_length=1)
    name: str | None = None


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


class MaterialAttributes(CognitiveValue):
    schema_key: str = Field(min_length=1)
    values: dict[str, JsonValue]


class CognitiveMaterialView(CognitiveValue):
    ref: MaterialRef
    kind: str
    title: str | None = None
    body: ContentSelection | None = None
    sections: tuple[MaterialSection, ...] = ()
    sources: tuple[SourceBinding, ...] = ()
    material_sources: tuple[MaterialSource, ...] = ()
    relations: tuple[MaterialRelation, ...] = ()
    times: CognitiveTimes | None = None
    attributes: MaterialAttributes


class MaterialRoot(CognitiveValue):
    """One formal independent source identity and its original witness."""

    key: str = Field(min_length=1)
    artifact: ArtifactRef


class MaterialIndex(CognitiveValue):
    """Owner facts used to select and project this material for retrieval."""

    key: str = Field(min_length=1)
    group_key: str = Field(min_length=1)
    namespace: NamespaceRef | None = None
    available: bool = True
    opt_in: str | None = None
    purpose: str | None = None
    needs_review: bool = False
    annotation: AnnotationView | None = None
    interpretation: bool = False
    root: MaterialRoot | None = None
    provenance: ArtifactRef | None = None
    support_relations: tuple[MaterialRelation, ...] = ()


class MaterialSnapshot(CognitiveValue):
    """Owner facts and immutable body identity captured in one read transaction."""

    view: CognitiveMaterialView
    body_digest: str | None
    index: MaterialIndex


class MaterialSourceRead(CognitiveValue):
    source: MaterialSource
    views: tuple[ContentReadView, ...]


class MaterialRead(CognitiveValue):
    material: CognitiveMaterialView
    selections: tuple[ContentReadView, ...]
    sources: tuple[MaterialSourceRead, ...] = ()


class ReadMaterial(CognitiveValue):
    ref: str | MaterialRef | None = None
    selection: MaterialSelection | None = None
    delivery: str | ResultRef | None = None
    key: str | None = None
    namespace: NamespaceSelector = None
    blocks: tuple[str, ...] = ()
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
