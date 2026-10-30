import math
from typing import Literal

from pydantic import Field, JsonValue, model_validator

from .bundles import (
    AllSelection,
    BundleAssetOptions,
    BundlePlacement,
    BundleSelector,
    PackageDescriptor,
)
from .cognition import CognitiveRecord, CognitiveRef
from .common import Contract, Operation
from .derivation import DerivedBundle
from .imports import UnitOutput
from .model_calls import ArchivedCall
from .references import ReferenceBinding


class SourceCommand(Operation):
    requested_by: str | None = Field(default=None, max_length=120)
    observation_ids: tuple[str, ...] = Field(min_length=1)
    reason: str = Field(min_length=1)
    expected_preview: str | None = None


class SourcePreview(Contract):
    observation_ids: tuple[str, ...]
    affected: tuple[CognitiveRef, ...]
    retiring: tuple[CognitiveRef, ...]
    retained: tuple[CognitiveRef, ...]
    preview_token: str


class SourceResult(Contract):
    operation_id: str
    source_operation_id: str | None = None
    observation_ids: tuple[str, ...]
    affected: tuple[CognitiveRef, ...]
    commit_seq: int


class BundleRequest(Contract):
    package: PackageDescriptor | None = None
    content_level: Literal["full", "knowledge"] = "full"
    format: Literal["native", "amemorix"] = "native"
    selector: BundleSelector = Field(default_factory=AllSelection)
    assets: BundleAssetOptions | None = None
    references: tuple[CognitiveRef, ...] | None = None


class InstallBundle(Operation):
    mode: Literal["merge", "restore"] = "merge"
    placement: BundlePlacement = Field(default_factory=BundlePlacement)
    archive_material_id: str | None = None
    bundle: dict[str, JsonValue]


class BundleMaterial(Contract):
    id: str
    namespace_id: str
    content_hash: str
    record: dict[str, JsonValue]


class BundleObservation(Contract):
    id: str
    namespace_id: str
    subject_id: str
    material_id: str
    kind: Literal["import", "contact"]
    active: bool
    record: dict[str, JsonValue]
    commit_seq: int


class BundleCatalog(Contract):
    id: str
    namespace_id: str
    subject_id: str
    record: dict[str, JsonValue]


class BundleInput(Contract):
    id: str
    catalog_id: str
    material_id: str
    record: dict[str, JsonValue]


class BundleUnit(Contract):
    id: str
    catalog_id: str
    record: dict[str, JsonValue]


class BundleFileSuccess(Contract):
    id: str
    namespace_id: str
    subject_id: str
    input_source_id: str
    record: dict[str, JsonValue]


class BundleVector(Contract):
    provenance: dict[str, JsonValue] = Field(default_factory=dict)
    reference: str
    space: str
    model: str
    dimensions: int = Field(ge=1)
    vector: tuple[float, ...]
    call_id: str | None = None
    commit_seq: int

    @model_validator(mode="after")
    def actual_coordinates(self) -> "BundleVector":
        if len(self.vector) != self.dimensions or not all(
            math.isfinite(value) for value in self.vector
        ):
            raise ValueError("Vector coordinates must be finite and match their actual dimensions")
        return self


class SubjectBundle(Contract):
    package: PackageDescriptor | None = None
    content_level: Literal["full", "knowledge"] = "full"
    format: Literal["nous.subject"] = "nous.subject"
    version: Literal[1] = 1
    id: str
    namespace_id: str
    subject_id: str
    authority_commit_seq: int
    cognition: tuple[CognitiveRecord, ...]
    materials: tuple[BundleMaterial, ...]
    observations: tuple[BundleObservation, ...]
    catalogs: tuple[BundleCatalog, ...] = ()
    inputs: tuple[BundleInput, ...] = ()
    units: tuple[BundleUnit, ...] = ()
    outputs: tuple[UnitOutput, ...] = ()
    file_successes: tuple[BundleFileSuccess, ...] = ()
    regions: tuple[dict[str, JsonValue], ...] = ()
    contents: dict[str, str] = Field(default_factory=dict)
    bindings: tuple[ReferenceBinding, ...] = ()
    derivations: DerivedBundle = Field(default_factory=DerivedBundle)
    vectors: tuple[BundleVector, ...] = ()
    calls: tuple[ArchivedCall, ...] = ()
