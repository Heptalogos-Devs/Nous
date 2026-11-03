"""User-supplied collection, query and source selection values."""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator

from nous.content.contracts import SourceSelection
from nous.temporal.values import CognitiveTimes, Interval


class WaveValue(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class Selector(WaveValue):
    kind: Literal["whole", "lines", "bytes", "pages", "time", "image"] = "whole"
    first: int | None = Field(default=None, ge=1)
    last: int | None = Field(default=None, ge=1)
    start: int | None = Field(default=None, ge=0)
    end: int | None = Field(default=None, ge=0)
    pages: tuple[int, ...] = ()
    frame: int | None = Field(default=None, ge=0)
    crop: tuple[int, int, int, int] | None = None

    @model_validator(mode="after")
    def coordinates_match_kind(self) -> Selector:
        selected = {
            name
            for name in ("first", "last", "start", "end", "pages", "frame", "crop")
            if getattr(self, name) not in (None, ())
        }
        permitted = {
            "whole": set[str](),
            "lines": {"first", "last"},
            "bytes": {"start", "end"},
            "pages": {"pages"},
            "time": {"start", "end"},
            "image": {"frame", "crop", "start", "end"},
        }[self.kind]
        if selected - permitted:
            raise ValueError("selector coordinates do not match its kind")
        if self.kind == "lines" and self.first is None and self.last is None:
            raise ValueError("line selection needs a first or last line")
        if self.kind in {"bytes", "time"} and self.start is None and self.end is None:
            raise ValueError("range selection needs a start or end")
        if self.kind == "pages" and not self.pages:
            raise ValueError("page selection needs at least one page")
        if self.last is not None and self.first is not None and self.last < self.first:
            raise ValueError("last line must not precede first line")
        if self.end is not None and self.start is not None and self.end <= self.start:
            raise ValueError("range end must follow its start")
        return self

    def content_selection(self) -> SourceSelection:
        if self.kind == "whole":
            return SourceSelection()
        if self.kind == "lines":
            return SourceSelection(start_line=self.first, end_line=self.last)
        if self.kind == "bytes":
            return SourceSelection(start_byte=self.start, end_byte=self.end)
        if self.kind == "pages":
            return SourceSelection(pages=self.pages)
        if self.kind == "time":
            return SourceSelection(start_ms=self.start, end_ms=self.end)
        return SourceSelection(
            frame=self.frame, crop=self.crop, start_ms=self.start, end_ms=self.end
        )


class DocumentInput(WaveValue):
    file: str | None = None
    text: str | None = None
    artifact: str | None = None
    selector: Selector = Field(default_factory=Selector)

    @model_validator(mode="after")
    def one_source(self) -> DocumentInput:
        if sum(value is not None for value in (self.file, self.text, self.artifact)) != 1:
            raise ValueError("document input requires exactly one of file, text, artifact")
        return self


class Document(WaveValue):
    key: str = Field(min_length=1)
    input: DocumentInput
    derived_from: tuple[str, ...] = ()
    evidence_group: str | None = Field(default=None, min_length=1)
    times: CognitiveTimes | None = None


class Concept(WaveValue):
    key: str = Field(min_length=1)
    label: str = Field(min_length=1)
    description: str = Field(min_length=1)
    aliases: tuple[str, ...] = ()

    def embedding_text(self) -> str:
        lines = [self.label, self.description]
        lines.extend(alias for alias in self.aliases if alias and alias not in lines)
        return "\n".join(lines)


class SourceSequence(WaveValue):
    document: str
    concepts: tuple[str, ...]
    ordered: bool


class CandidateContent(WaveValue):
    document: str
    selector: Selector = Field(default_factory=Selector)


class Candidate(WaveValue):
    key: str = Field(min_length=1)
    content: CandidateContent
    document: str
    concepts: tuple[str, ...] | None = None
    curve: tuple[str, ...] | None = None
    metadata: dict[str, JsonValue] = Field(default_factory=dict)


class Association(WaveValue):
    key: str = Field(min_length=1)
    source: str
    target: str
    relation: str = Field(min_length=1)
    directed: bool
    strength: float = Field(ge=0, le=1, allow_inf_nan=False)
    confidence: float = Field(ge=0, le=1, allow_inf_nan=False)
    polarity: Literal["positive", "negative"]
    evidence: tuple[str, ...] = Field(min_length=1)


class CollectionManifest(WaveValue):
    name: str = Field(min_length=1)
    space: str | None = None
    documents: tuple[Document, ...] = ()
    concepts: tuple[Concept, ...] = ()
    source_sequences: tuple[SourceSequence, ...] = ()
    candidates: tuple[Candidate, ...] = ()
    associations: tuple[Association, ...] = ()

    @model_validator(mode="after")
    def references_exist(self) -> CollectionManifest:
        collections = (
            ("document", self.documents),
            ("concept", self.concepts),
            ("candidate", self.candidates),
            ("association", self.associations),
        )
        for kind, items in collections:
            keys = [item.key for item in items]
            if len(keys) != len(set(keys)):
                raise ValueError(f"duplicate {kind} key")
        documents = {item.key for item in self.documents}
        concepts = {item.key for item in self.concepts}
        for item in self.documents:
            if set(item.derived_from) - documents:
                raise ValueError(f"document {item.key} references an unknown parent")
            if item.evidence_group and item.derived_from:
                raise ValueError("evidence_group belongs to an original source")
        for item in self.source_sequences:
            if item.document not in documents or set(item.concepts) - concepts:
                raise ValueError("source sequence references an unknown document or concept")
        for item in self.candidates:
            if item.document not in documents or item.content.document not in documents:
                raise ValueError(f"candidate {item.key} references an unknown document")
            if set(item.concepts or ()) - concepts or set(item.curve or ()) - concepts:
                raise ValueError(f"candidate {item.key} references an unknown concept")
        for item in self.associations:
            if item.source not in concepts or item.target not in concepts:
                raise ValueError(f"association {item.key} references an unknown concept")
            if set(item.evidence) - documents:
                raise ValueError(f"association {item.key} references an unknown document")
        parents = {item.key: item.derived_from for item in self.documents}
        complete: set[str] = set()
        visiting: set[str] = set()

        def visit(key: str) -> None:
            if key in visiting:
                raise ValueError("document derivation contains a cycle")
            if key in complete:
                return
            visiting.add(key)
            for parent in parents[key]:
                visit(parent)
            visiting.remove(key)
            complete.add(key)

        for key in parents:
            visit(key)
        return self


class ContextContent(WaveValue):
    artifact: str
    selector: Selector = Field(default_factory=Selector)


class ContextFragment(WaveValue):
    key: str = Field(min_length=1)
    role: str = Field(min_length=1)
    sequence: int
    text: str | None = None
    content: ContextContent | None = None
    weight: float = Field(default=1, ge=0, allow_inf_nan=False)
    time: CognitiveTimes | None = None

    @model_validator(mode="after")
    def one_body(self) -> ContextFragment:
        if (self.text is None) == (self.content is None):
            raise ValueError("context requires exactly one of text or content")
        return self


class QueryCue(WaveValue):
    label: str = Field(min_length=1)
    description: str = Field(min_length=1)
    is_core: bool = False

    def embedding_text(self) -> str:
        return f"{self.label}\n{self.description}"


class TimeCondition(WaveValue):
    axis: Literal["occurred", "observed", "formed", "valid", "recorded"]
    range: Interval


class CandidateFilter(WaveValue):
    candidate_keys: tuple[str, ...] | None = None
    times: tuple[TimeCondition, ...] = ()
    include_unknown: bool = False


class ThinkingStage(WaveValue):
    name: str = Field(min_length=1)
    candidates: tuple[str, ...]
    top_k: int = Field(default=10, ge=1)


class WaveQuery(WaveValue):
    snapshot: str
    text: str = ""
    context: tuple[ContextFragment, ...] = ()
    core_concepts: tuple[str, ...] = ()
    cues: tuple[QueryCue, ...] = ()
    candidate_filter: CandidateFilter | None = None
    top_k: int | None = Field(default=None, ge=1)
    mode: Literal["lexical", "dense", "tagmemo", "rivermemo", "hybrid", "dtsc"] | None = None
    include_trace: bool = False
    include_counterevidence: bool | None = None
    thinking_stages: tuple[ThinkingStage, ...] = ()

    @model_validator(mode="after")
    def unique_context_keys(self) -> WaveQuery:
        keys = [item.key for item in self.context]
        if len(keys) != len(set(keys)):
            raise ValueError("context keys must be unique")
        return self
