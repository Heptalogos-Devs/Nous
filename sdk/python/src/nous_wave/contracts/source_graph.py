"""Bounded, authorized provenance graphs."""

from typing import Literal

from pydantic import Field, JsonValue

from .common import Contract
from .targets import ContextTarget


class ExpandSources(Contract):
    targets: tuple[ContextTarget, ...] = Field(min_length=1, max_length=1000)
    max_depth: int = Field(default=4, ge=0, le=16)
    max_nodes: int = Field(default=128, ge=1, le=1024)
    text_bytes: int = Field(default=65536, ge=0, le=1048576)


class SourceNode(Contract):
    key: str
    target: ContextTarget | None = None
    observation_id: str | None = None
    observation: dict[str, JsonValue] | None = None
    material_id: str | None = None
    text: str = ""
    status: Literal["available", "unavailable"] = "available"
    reason: str | None = None


class SourceEdge(Contract):
    source: str
    target: str
    polarity: str = "support"
    role: str = "grounding"


class ExpandedSources(Contract):
    roots: tuple[str, ...]
    nodes: tuple[SourceNode, ...]
    edges: tuple[SourceEdge, ...]
    watermark: int
    complete: bool
