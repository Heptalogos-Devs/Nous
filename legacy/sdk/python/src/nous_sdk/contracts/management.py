from __future__ import annotations

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
from .common import Contract, ModelProvenance, ObservationKind, Operation
from .derivation import DerivedBundle
from .imports import UnitOutput
from .records import RecordsQuery
from .references import ReferenceBinding
from .retention import RetentionExpectation, RetentionSnapshot


class SourceSelection(Contract):
    observation_ids: tuple[str, ...] = ()
    source_names: tuple[str, ...] = ()
    references: tuple[CognitiveRef, ...] = Field(default=(), max_length=1000)
    records: RecordsQuery | None = None
    retention_expectations: tuple[RetentionExpectation, ...] = Field(default=(), max_length=1000)


class SourceCommand(SourceSelection, Operation):
    requested_by: str | None = Field(default=None, max_length=120)
    force: bool = False
    grace_hours: float | None = Field(default=None, ge=0, allow_inf_nan=False)
    reason: str = Field(min_length=1)
    expected_preview: str | None = None

    def operation_payload(self) -> dict[str, JsonValue]:
        omitted = {
            name
            for name, value in {
                "source_names": self.source_names,
                "references": self.references,
                "records": self.records,
                "force": self.force,
                "grace_hours": self.grace_hours,
                "retention_expectations": self.retention_expectations,
            }.items()
            if value in (None, False, ())
        }
        return self.model_dump(mode="json", exclude=omitted)


class SourcePreview(Contract):
    observation_ids: tuple[str, ...]
    affected: tuple[CognitiveRef, ...]
    retiring: tuple[CognitiveRef, ...]
    retained: tuple[CognitiveRef, ...]
    preview_token: str
    references: tuple[CognitiveRef, ...] = ()
    retention_states: tuple[RetentionSnapshot, ...] = ()


class SourceResult(Contract):
    operation_id: str
    source_operation_id: str | None = None
    observation_ids: tuple[str, ...]
    affected: tuple[CognitiveRef, ...]
    retired: tuple[CognitiveRef, ...] = ()
    restored: tuple[CognitiveRef, ...] = ()
    references: tuple[CognitiveRef, ...] = ()
    commit_seq: int


class BundleRequest(Contract):
    package: PackageDescriptor | None = None
    content_level: Literal["full", "knowledge"] = "full"
    selector: BundleSelector = Field(default_factory=AllSelection)
    assets: BundleAssetOptions | None = None
    references: tuple[CognitiveRef, ...] | None = None


class InstallBundle(Contract):
    mode: Literal["merge", "restore"] = "merge"
    placement: BundlePlacement = Field(default_factory=BundlePlacement)
    bundle: dict[str, JsonValue]


class BundleInstallation(Contract):
    installation_id: str
    package: PackageDescriptor | None = None
    content_level: Literal["full", "knowledge"]
    mode: Literal["merge", "restore"]
    placement: BundlePlacement
    content_digest: str
    source_namespace_id: str
    installed_at: str
    state: Literal["active", "uninstalled"]
    owned_observation_ids: tuple[str, ...]
    bundle_id: str
    mapping: dict[str, str]
    vector_provenance: tuple[dict[str, JsonValue], ...]
    cognition_count: int
    output_count: int
    commit_seq: int
    uninstalled_commit_seq: int | None = None


class BundleRemoval(Contract):
    installation_id: str
    state: Literal["uninstalled"]
    affected: tuple[str, ...] = ()
    commit_seq: int | None = None
    already_uninstalled: bool = False


class BundleMaterial(Contract):
    id: str
    namespace_id: str
    content_hash: str
    record: dict[str, JsonValue]


class BundleObservation(Contract):
    id: str
    namespace_id: str

    material_id: str
    kind: ObservationKind
    caller_id: str | None = None
    external_event_id: str | None = None
    active: bool
    record: dict[str, JsonValue]
    commit_seq: int


class BundleCatalog(Contract):
    id: str
    namespace_id: str

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

    input_source_id: str
    record: dict[str, JsonValue]


class BundleVector(Contract):
    provenance: dict[str, JsonValue] = Field(default_factory=dict)
    producer: ModelProvenance | None = None
    reference: str
    space: str
    model: str
    dimensions: int = Field(ge=1)
    vector: tuple[float, ...]
    commit_seq: int

    @model_validator(mode="after")
    def actual_coordinates(self) -> BundleVector:
        if len(self.vector) != self.dimensions or not all(
            math.isfinite(value) for value in self.vector
        ):
            raise ValueError("Vector coordinates must be finite and match their actual dimensions")
        return self


class NamespaceBundle(Contract):
    package: PackageDescriptor | None = None
    content_level: Literal["full", "knowledge"] = "full"
    format: Literal["nous.namespace.bundle"] = "nous.namespace.bundle"
    version: Literal[2] = 2
    id: str
    namespace_id: str

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

    @model_validator(mode="after")
    def single_namespace(self) -> NamespaceBundle:
        for record in (
            *self.cognition,
            *self.materials,
            *self.observations,
            *self.catalogs,
            *self.file_successes,
        ):
            if record.namespace_id != self.namespace_id:
                raise ValueError(
                    "A namespace bundle must contain records from its declared namespace"
                )
        for row in (
            *self.materials,
            *self.observations,
            *self.catalogs,
            *self.inputs,
            *self.units,
            *self.file_successes,
        ):
            if "namespace_id" in row.record and row.record["namespace_id"] != self.namespace_id:
                raise ValueError("Archived record ownership differs from the bundle namespace")
        return self
