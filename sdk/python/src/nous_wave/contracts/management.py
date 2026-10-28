from typing import Literal

from pydantic import Field, JsonValue

from .cognition import CognitiveRecord, CognitiveRef
from .common import Contract, Operation
from .imports import UnitOutput


class SourceCommand(Operation):
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
    references: tuple[CognitiveRef, ...] | None = None


class InstallBundle(Operation):
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


class SubjectBundle(Contract):
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
