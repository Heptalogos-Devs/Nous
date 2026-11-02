"""A-Memorix host tools retain explicit input contracts."""

from typing import Literal
from uuid import uuid4

from pydantic import AliasChoices, Field, JsonValue

from ..contracts import CognitiveRef, Contract, EvidenceLink, TimeRange


class RelationInput(Contract):
    subject: str = Field(min_length=1)
    predicate: str = Field(min_length=1)
    object: str = Field(min_length=1)
    confidence: float | None = Field(default=None, ge=0, le=1)
    metadata: dict[str, JsonValue] = Field(default_factory=dict)


class SearchInput(Contract):
    query: str = ""
    limit: int = Field(default=5, ge=1, le=1000)
    mode: Literal["search", "time", "hybrid", "episode", "aggregate"] = "search"
    chat_id: str = ""
    shared_chat_ids: tuple[str, ...] = ()
    person_id: str = ""
    time_start: str | float | None = None
    time_end: str | float | None = None
    respect_filter: bool = True
    user_id: str = ""
    group_id: str = ""


class IngestInput(Contract):
    external_id: str = Field(min_length=1)
    source_type: str = Field(min_length=1)
    text: str = Field(min_length=1)
    chat_id: str = ""
    person_ids: tuple[str, ...] = ()
    participants: tuple[str, ...] = ()
    timestamp: str | float | None = None
    time_start: str | float | None = None
    time_end: str | float | None = None
    tags: tuple[str, ...] = ()
    metadata: dict[str, JsonValue] = Field(default_factory=dict)
    entities: tuple[str, ...] = ()
    relations: tuple[RelationInput, ...] = ()
    respect_filter: bool = True
    user_id: str = ""
    group_id: str = ""
    operation_id: str | None = None
    observed_at: str | float | None = None
    validity: TimeRange | None = None
    relation_extraction: Literal["default", "enabled", "disabled"] = "default"
    deferred_formation: bool = False
    require_vectors: bool = False


class SummaryInput(IngestInput):
    source_type: str = "chat_summary"


class MaintainInput(Contract):
    action: Literal[
        "reinforce",
        "weaken",
        "protect",
        "unprotect",
        "restore",
        "freeze",
        "forget",
        "trash",
        "purge",
        "decay",
        "recycle_bin",
    ]
    target: str = ""
    strength: float = Field(default=1, ge=0, allow_inf_nan=False)
    force: bool = False
    hours: float | None = Field(default=None, ge=0)
    reason: str = ""
    limit: int = Field(default=50, ge=1, le=1000)
    operation_id: str
    cursor: str | None = Field(default=None, exclude_if=lambda value: value is None)


class V5AdminInput(Contract):
    action: Literal[
        "reinforce", "weaken", "remember_forever", "restore", "forget", "recycle_bin", "status"
    ]
    target: str = Field(default="", validation_alias=AliasChoices("target", "query"))
    strength: float = Field(default=1, gt=0, allow_inf_nan=False)
    limit: int = Field(default=50, ge=1, le=1000)
    reason: str = ""
    updated_by: str = Field(default="", validation_alias=AliasChoices("updated_by", "requested_by"))
    operation_id: str | None = None


class ProfileInput(Contract):
    person_id: str = ""
    person_name: str = Field(
        default="", validation_alias=AliasChoices("person_name", "person_keyword", "keyword")
    )
    chat_id: str = ""
    context_text: str = ""
    limit: int = Field(default=10, ge=1, le=1000, validation_alias=AliasChoices("limit", "top_k"))
    force_refresh: bool = False
    operation_id: str | None = None


class ProfileAdminInput(ProfileInput):
    action: Literal[
        "status",
        "process_pending",
        "list",
        "query",
        "evidence",
        "get_aliases",
        "set_aliases",
        "delete_aliases",
        "set_override",
        "delete_override",
        "correct_evidence",
    ]
    limit: int = Field(default=12, ge=1, le=1000, validation_alias=AliasChoices("limit", "top_k"))
    aliases: tuple[str, ...] | None = None
    text: str | None = Field(default=None, validation_alias=AliasChoices("text", "override_text"))
    requested_by: str | None = Field(
        default=None, validation_alias=AliasChoices("requested_by", "updated_by")
    )
    source: str | None = None
    reason: str = "Person profile management"
    evidence_type: str | None = None
    evidence_key: str | None = None
    hash: str | None = Field(default=None, validation_alias=AliasChoices("hash", "hash_value"))
    attribute_pointer: str | None = None
    reference: CognitiveRef | None = None
    evidence: EvidenceLink | None = None
    refresh: bool = True


class AMemorixOperation(Contract):
    tool: str
    arguments: dict[str, JsonValue] = Field(default_factory=dict)


class AMemorixRequest(Contract):
    namespace_id: str
    
    operation: AMemorixOperation


class AMemorixPolicy(Contract):
    enabled: bool = True
    mode: Literal["blacklist", "whitelist"] = "blacklist"
    chats: tuple[str, ...] = ()
    excluded_source_types: tuple[str, ...] = ()
    ingest_wait_ms: int = Field(default=45000, ge=1, le=86400000)
    search_mode: Literal["lexical", "dense", "hybrid", "vcp"] = "hybrid"
    orphan_sweep_grace_hours: float = Field(default=24, ge=1, allow_inf_nan=False)

    def permits(self, respect: bool, chat: str, group: str, user: str) -> bool:
        if not respect or not self.enabled or not any((chat, group, user)):
            return True
        matched = False
        for pattern in self.chats:
            if ":" in pattern:
                kind, value = pattern.split(":", 1)
                matched |= (
                    (kind == "stream" and value == chat)
                    or (kind == "group" and value == group)
                    or (kind in {"user", "private"} and value == user)
                )
            else:
                matched |= pattern in {chat, group, user}
        return not matched if self.mode == "blacklist" else matched


class GraphSearchInput(Contract):
    action: Literal["search"] = "search"
    query: str = Field(max_length=65536)
    limit: int = Field(default=50, ge=1, le=200)


class ImageInput(Contract):
    action: str
    image_base64: str | None = None
    current_occurrence_id: str | None = None
    target_type: str = ""
    target_id: str = ""
    link_kind: str = "evidence"
    link_id: str | None = None
    status: str | None = None
    description_hash: str = ""
    lease_token: str = ""
    success: bool = False
    error: str = ""
    operation_id: str = Field(default_factory=lambda: str(uuid4()))
    material_id: str | None = None
    asset_id: str | None = None
    content_hash: str | None = Field(default=None, pattern=r"^[a-fA-F0-9]{32}(?:[a-fA-F0-9]{32})?$")
    occurrence_id: str | None = None
    external_ref: str = ""
    source_kind: str = "chat"
    scope_type: Literal["chat", "global"] = "chat"
    chat_id: str = ""
    session_id: str = ""
    chat_ids: tuple[str, ...] | None = None
    message_id: str = ""
    component_path: str = ""
    occurred_at: str | float | None = None
    user_statement: str = ""
    text: str = ""
    current_external_ref: str = ""
    candidate_limit: int = Field(default=10, ge=1, le=1000)
    limit: int = Field(default=50, ge=1, le=1000)
    offset: int = Field(default=0, ge=0)
    similarity_threshold: float | None = Field(default=None, ge=-1, le=1)
    reason: str = "Image source maintenance"
    form_long_term: bool = False
    confirm_status: str = "unconfirmed"
    evidence: dict[str, JsonValue] = Field(default_factory=dict)
    supersedes_id: str | None = None
