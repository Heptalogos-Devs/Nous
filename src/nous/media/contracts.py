from dataclasses import dataclass, field, replace
from typing import TYPE_CHECKING

from pydantic import JsonValue
from pydantic_ai.messages import BinaryContent
from nous.common.errors import NousError

from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    InputGroup,
    ProcessingResult,
    SourceRole,
    SourceSelection,
)

from .configuration import MediaConfig

if TYPE_CHECKING:
    from nous.content.text import TextView


@dataclass(frozen=True)
class PlannedPart:
    source: ContentSelection
    source_media_type: str
    representation: str
    target_media_type: str
    recipe: dict[str, JsonValue]
    known_shape: dict[str, JsonValue] = field(default_factory=dict)


@dataclass(frozen=True)
class PlannedConversion:
    artifact: Artifact
    selection: SourceSelection
    requirements: tuple[str, ...]
    settings: MediaConfig
    document_mode: str
    without_audio: bool
    parts: tuple[PlannedPart, ...]


@dataclass(frozen=True)
class PreparedSource:
    key: str
    name: str
    text: str | None = None
    media: tuple[BinaryContent, ...] = ()
    group: InputGroup | None = None
    metadata: dict[str, JsonValue] = field(default_factory=dict)

    def bind_slot(
        self, slot_key: str, prefix: str, role: SourceRole, *, preserve_key: bool = False
    ) -> PreparedSource:
        """Rename a local representation using its declared slot, retaining actual source ownership."""
        if self.group is None:
            raise NousError("media.invalid_plan", "Prepared source has no exact input group")
        local = self.group.slot_key
        if preserve_key:
            key = self.key
        elif self.key == local or self.key.startswith(local + ":"):
            key = prefix + self.key[len(local) :]
        else:
            raise NousError(
                "media.invalid_plan",
                "Representation key does not belong to its declared slot",
                details={"key": self.key, "slot_key": local},
            )
        return replace(
            self,
            key=key,
            group=self.group.model_copy(update={"key": key, "slot_key": slot_key, "role": role}),
        )


@dataclass(frozen=True)
class PreparedBatch:
    sources: tuple[PreparedSource, ...]
    inputs: tuple[InputGroup, ...]
    selection: SourceSelection
    coverage: dict[str, JsonValue]
    conversion: PlannedConversion | None = None
    completed_result_refs: tuple[str, ...] = ()
    source_prefix: str = "S1"
    source_slot_key: str = "S1"
    source_role: SourceRole = "source"


@dataclass(frozen=True)
class MediaPlan:
    artifact: Artifact
    selection: SourceSelection
    batches: tuple[PreparedBatch, ...]
    coverage: dict[str, JsonValue]
    completed_result_refs: tuple[str, ...] = ()


@dataclass(frozen=True)
class TextMapping:
    content: ContentSelection
    origins: tuple[ContentSelection, ...]


@dataclass(frozen=True)
class TextRepresentation:
    body: ArtifactRef
    view: TextView
    inputs: tuple[InputGroup, ...]
    coverage: dict[str, JsonValue]
    completed_result_refs: tuple[str, ...] = ()
    mappings: tuple[TextMapping, ...] = ()


@dataclass(frozen=True)
class AnalysisResult:
    artifact: Artifact
    selection: SourceSelection
    batches: tuple[ProcessingResult, ...]
    descriptions: tuple[ProcessingResult, ...]
    coverage: dict[str, JsonValue]
    outcome: str = "completed"
    preparations: tuple[str, ...] = ()

    @property
    def completed_result_refs(self) -> tuple[str, ...]:
        return tuple(
            dict.fromkeys(
                (
                    *self.preparations,
                    *(result.ref.address for result in (*self.descriptions, *self.batches)),
                )
            )
        )
