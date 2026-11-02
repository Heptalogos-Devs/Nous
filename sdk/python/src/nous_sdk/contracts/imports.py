from __future__ import annotations

from typing import Literal

from pydantic import Field, JsonValue, model_validator

from .cognition import CognitiveRef, FormationCatalogEntry, FormationDraft, ProposedMemory
from .common import ApplicableInput, Contract, Operation, TimePoint, TimeRange
from .fact_body import FactClaimValue
from .media import MediaReference, MediaStrategy
from .vector_projection import ProjectionReceipt

ImportFormat = Literal[
    "text", "markdown", "html", "pdf", "json", "jsonl", "media", "parquet", "sqlite", "graphml"
]


class RecordSelection(Contract):
    after_sequence: int | None = Field(default=None, ge=0)
    through_sequence: int | None = Field(default=None, ge=0)
    conversation_refs: tuple[str, ...] | None = None
    group_refs: tuple[str, ...] = ()
    user_refs: tuple[str, ...] = ()
    event: TimeRange | None = None
    limit: int | None = Field(default=None, ge=1, le=10000000)


class ImportDocument(ApplicableInput):
    text: str | None = Field(default=None, min_length=1)
    material_id: str | None = None
    format: ImportFormat = "text"
    media_strategy: MediaStrategy = "description"
    filename: str = "input.txt"
    media_type: str = "text/plain"
    file_identity: str | None = None
    source_type: str = "document"
    source: str | None = Field(default=None, max_length=4096)
    origin: dict[str, str] = Field(default_factory=dict)
    event_time: TimePoint | None = None
    observed_at: TimePoint | None = None
    selection: RecordSelection | None = Field(default=None, exclude_if=lambda value: value is None)
    declared_vector_dimensions: int | None = Field(
        default=None, gt=0, exclude_if=lambda value: value is None
    )
    read_batch_size: int | None = Field(
        default=None, ge=1, le=1000000, exclude_if=lambda value: value is None
    )

    @model_validator(mode="after")
    def exact_input(self) -> ImportDocument:
        if (self.text is None) == (self.material_id is None):
            raise ValueError("Import requires exactly one inline text or original material_id")
        return self

    def inline_text(self) -> str:
        if self.text is None:
            raise ValueError("This document refers to an original Material")
        return self.text


class ImportFact(FactClaimValue, ApplicableInput):
    person_key: str | None = None
    conversation_ref: str | None = None
    authority: Literal["manual", "direct_user", "imported", "summary_derived"] = "imported"

    @model_validator(mode="after")
    def explicit_scope(self) -> ImportFact:
        if (self.person_key is None) == (self.conversation_ref is None):
            raise ValueError(
                "Source facts require exactly one person key or conversation reference"
            )
        return self


class ImportAssertions(FormationDraft):
    memories: tuple[ProposedMemory, ...] = ()
    facts: tuple[ImportFact, ...] = ()
    references: tuple[ImportReference, ...] = ()


class ImportReference(Contract):
    key: str = Field(min_length=1)
    kind: Literal["memory", "entity", "association"]
    identifier_key: str = Field(min_length=1)
    identifier: str = Field(min_length=1)
    aliases: tuple[str, ...] = ()


class ImportRecord(ApplicableInput):
    external_id: str = Field(min_length=1)
    text: str = Field(min_length=1)
    source: str | None = None
    source_type: str | None = None
    event_time: TimePoint | None = None
    observed_at: TimePoint | None = None
    origin: dict[str, str] = Field(default_factory=dict)
    tags: tuple[str, ...] = ()
    metadata: dict[str, JsonValue] = Field(default_factory=dict)
    identifiers: dict[str, str] = Field(default_factory=dict, exclude_if=lambda value: not value)
    structured: JsonValue = None
    assertions: ImportAssertions | None = Field(
        default=None, exclude_if=lambda value: value is None
    )


class ImportRecordError(Contract):
    input_source_id: str
    record_index: int
    line_number: int | None = None
    external_id: str | None = None
    code: str
    message: str


class TextSplitOptions(Contract):
    record_units: bool = False
    chunk_size: int = Field(default=2400, ge=128, le=100000)
    chunk_overlap: int = Field(default=200, ge=0)
    strategy_override: Literal["auto", "factual", "narrative", "quote", "chat_log"] | None = None
    narrative_window_size: int | None = Field(default=None, ge=200, le=100000)
    narrative_overlap: int | None = Field(default=None, ge=0)
    factual_target_size: int | None = Field(default=None, ge=200, le=100000)
    chat_log: bool = False
    chat_reference_time: str | None = None
    extract_time: bool = True
    content_category: str | None = None
    timezone: str | None = None
    reference_time: TimePoint | None = None


class FreezeCatalog(Operation, TextSplitOptions):
    inputs: tuple[ImportDocument, ...] = Field(min_length=1)
    parent_catalog_id: str | None = None


class ImportUnit(Contract):
    id: str
    catalog_id: str
    input_source_id: str
    material_id: str
    ordinal: int
    text: str
    start: int
    end: int
    purged: bool = False
    record_index: int = 0
    observation_id: str | None = None
    source_region_id: str | None = None
    source: MediaReference | None = None
    json_pointer: str | None = None
    line_number: int | None = None
    context: dict[str, JsonValue] = Field(default_factory=dict)
    strategy: str = "narrative"
    requires_llm: bool = True
    split_context: dict[str, JsonValue] = Field(default_factory=dict)
    formation_options: dict[str, JsonValue] = Field(default_factory=dict)


class CatalogView(Contract):
    split_options: TextSplitOptions | None = None
    id: str
    namespace_id: str
    
    units: tuple[ImportUnit, ...]
    input_source_ids: tuple[str, ...]
    original_unit_count: int
    original_record_count: int = 0
    errors: tuple[ImportRecordError, ...] = ()
    formation_catalog: tuple[FormationCatalogEntry, ...] = ()


class FormationOptionsOverride(Contract):
    extract_time: bool | None = None
    content_category: str | None = None
    timezone: str | None = None
    reference_time: TimePoint | str | None = None
    chat_reference_time: str | None = None


class ProcessCatalog(Operation):
    max_errors: int = Field(default=0, ge=0, exclude_if=lambda value: value == 0)
    force: bool = False
    deferred_formation: bool = False
    origin: dict[str, str] | None = None
    retry_parent_job_id: str | None = None
    catalog_id: str
    unit_ids: tuple[str, ...] | None = None
    formation_options_override: FormationOptionsOverride | None = None
    llm_enabled: bool = True
    formation_retries: int = Field(default=1, ge=0, le=3)
    vectors: Literal["disabled", "metadata_allowed", "required"] = "metadata_allowed"


class UnitOutput(Contract):
    projection: ProjectionReceipt | None = None
    formation: ObservationFormSubmission | None = None
    id: str
    unit_id: str
    observation_id: str
    cognition: tuple[CognitiveRef, ...]
    attempt_job_id: str | None = None
    producer_call_id: str | None = None
    vector_policy: Literal["disabled", "metadata_allowed", "required"]
    commit_seq: int


class FileSuccess(Contract):
    id: str
    catalog_id: str
    input_source_id: str
    unit_outputs: dict[str, str]
    content_hash: str
    file_identity: str | None
    origin: dict[str, str]
    commit_seq: int


class ImportResult(Contract):
    catalog: CatalogView | None
    outputs: tuple[UnitOutput, ...]
    file_successes: tuple[FileSuccess, ...]
    skipped_success_ids: tuple[str, ...] = ()
    failures: tuple[ImportUnitFailure, ...] = Field(
        default=(), exclude_if=lambda values: not values
    )


class AdmitFiles(FreezeCatalog):
    deferred_formation: bool = False
    llm_enabled: bool = True
    formation_retries: int = Field(default=1, ge=0, le=3)
    vectors: Literal["disabled", "metadata_allowed", "required"] = "metadata_allowed"
    dedupe_policy: Literal["none", "content_hash", "manifest"] = "content_hash"
    force: bool = False
    clear_manifest: bool = False


class ImportFailure(Contract):
    code: str
    message: str


class ImportUnitFailure(ImportFailure):
    unit_id: str
    details: dict[str, JsonValue] = Field(default_factory=dict)


class UnitAttempt(Contract):
    id: str
    catalog_id: str
    unit_id: str
    operation_id: str
    job_id: str | None = None
    status: Literal["running", "succeeded", "failed", "cancelled"] = "running"
    stage: Literal["formation", "adoption", "embedding", "complete"] = "formation"
    started_at: str
    finished_at: str | None = None
    output_id: str | None = None
    producer_call_id: str | None = None
    error: ImportFailure | None = None
    vector_error: ImportFailure | None = None


class TextPreviewUnit(Contract):
    text: str
    start: int
    end: int
    strategy: str
    requires_llm: bool
    context: dict[str, JsonValue] = Field(default_factory=dict)


class TextImportPreview(Contract):
    content_hash: str
    byte_len: int
    options: TextSplitOptions
    units: tuple[TextPreviewUnit, ...]
    errors: tuple[str, ...] = ()


class FormObservation(Operation):
    media_strategy: MediaStrategy = "description"
    anchor: CognitiveRef | None = None
    vectors: Literal["disabled", "metadata_allowed", "required"] = "disabled"
    observation_id: str
    source: MediaReference | None = None
    llm_enabled: bool = True

    def identity_payload(self) -> dict[str, JsonValue]:
        return self.model_dump(
            mode="json",
            exclude={"media_strategy"} if self.media_strategy == "description" else set(),
        )


class ObservationFormation(Contract):
    projection: ProjectionReceipt | None = None
    vector_policy: Literal["disabled", "metadata_allowed", "required"] = "disabled"
    vector_error: ImportFailure | None = None
    observation_id: str
    cognition: tuple[CognitiveRef, ...]
    producer_call_id: str | None = None
    commit_seq: int


class VectorAdoption(Contract):
    embedded: int
    space: str | None
    references: tuple[str, ...] = ()
    output_id: str | None = None


class EmbeddingSyncResult(Contract):
    embedded: int
    remaining: int
    space: str | None


class ObservationFormSubmission(Contract):
    request_id: str
    job_id: str
    observation_id: str
