"""External resource registration, immutable source versions and current reads."""

from pathlib import Path
from typing import Literal

from pydantic import Field

from .common import CognitiveTimes, Contract, Operation, ReferenceAddresses
from .media import MediaReference

ResourceTextMethod = Literal["original", "html_body", "pdf_text_layer", "none"]


class ResourceReference(Contract):
    kind: Literal["resource"] = "resource"
    id: str
    revision: int = Field(ge=1)

    def exact(self) -> str:
        return f"resource:{self.id}@{self.revision}"


class ResourceLocation(Contract):
    kind: Literal["file", "http"]
    address: str = Field(min_length=1, max_length=4096)
    file_root: Path | None = None
    credential_file: Path | None = None
    credential_env: str | None = None


class RegisterResource(Operation):
    name: str = Field(min_length=1, max_length=512)
    description: str = Field(default="", max_length=16000)
    location: ResourceLocation
    read_policy: Literal["manual", "on_query"] = "on_query"
    max_bytes: int = Field(default=32 * 1024 * 1024, ge=1, le=512 * 1024 * 1024)


class ChangeResource(Operation):
    reference: ResourceReference
    name: str | None = Field(default=None, min_length=1, max_length=512)
    description: str | None = Field(default=None, max_length=16000)
    enabled: bool | None = None
    read_policy: Literal["manual", "on_query"] | None = None
    location: ResourceLocation | None = None
    max_bytes: int | None = Field(default=None, ge=1, le=512 * 1024 * 1024)


class ReadResource(Operation):
    resource_id: str
    current: Literal["none", "prefer", "required"] = "none"


class ResourceSnapshot(Contract):
    source: MediaReference
    content_hash: str
    byte_len: int
    media_type: str
    fetched_at: str
    version: str
    etag: str | None = None
    last_modified: str | None = None
    text_method: ResourceTextMethod = "none"
    page_count: int | None = None
    text_pages: tuple[int, ...] = ()


class ResourceRecord(Contract):
    reference: ResourceReference
    namespace_id: str
    
    name: str
    description: str
    address: str
    location_kind: Literal["file", "http"]
    read_policy: Literal["manual", "on_query"]
    enabled: bool
    snapshot: ResourceSnapshot | None = None
    status: Literal["registered", "available", "unavailable", "forbidden", "disabled", "purged"]
    checked_at: str | None = None
    error_code: str | None = None
    text: str = ""
    source_roots: tuple[str, ...] = ()
    support_roots: tuple[str, ...] = ()
    source_regions: dict[str, tuple[str, ...]] = Field(default_factory=dict)
    times: CognitiveTimes
    commit_seq: int
    addresses: ReferenceAddresses | None = None


class ResourcePage(Contract):
    items: tuple[ResourceRecord, ...]
    next_cursor: str | None = None
