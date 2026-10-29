from __future__ import annotations

from typing import Literal

from pydantic import Field, JsonValue, model_validator

from .cognition import CognitiveRef, FormationCatalogEntry
from .common import Contract, Operation, TimePoint
from .media import MediaReference


class ImportDocument(Contract):
    text: str | None = Field(default=None, min_length=1)
    material_id: str | None = None
    format: Literal["text", "json", "jsonl"] = "text"
    filename: str = "input.txt"
    media_type: str = "text/plain"
    file_identity: str | None = None
    source_type: str = "document"
    source: str | None = Field(default=None, max_length=4096)
    origin: dict[str, str] = Field(default_factory=dict)
    event_time: TimePoint | None = None
    observed_at: TimePoint | None = None

    @model_validator(mode="after")
    def exact_input(self) -> ImportDocument:
        if (self.text is None) == (self.material_id is None):
            raise ValueError("Import requires exactly one inline text or original material_id")
        return self

    def inline_text(self) -> str:
        if self.text is None:
            raise ValueError("This document refers to an original Material")
        return self.text


class ImportRecord(Contract):
    external_id: str = Field(min_length=1)
    text: str = Field(min_length=1)
    source: str | None = None
    source_type: str | None = None
    event_time: TimePoint | None = None
    observed_at: TimePoint | None = None
    origin: dict[str, str] = Field(default_factory=dict)
    tags: tuple[str, ...] = ()
    metadata: dict[str, JsonValue] = Field(default_factory=dict)
    structured: JsonValue = None


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
    subject_id: str
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


class FormObservation(Operation):
    anchor: CognitiveRef | None = None
    vectors: Literal["disabled", "metadata_allowed", "required"] = "disabled"
    observation_id: str
    source: MediaReference | None = None
    llm_enabled: bool = True


class ObservationFormation(Contract):
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
