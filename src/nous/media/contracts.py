from dataclasses import dataclass, field

from pydantic import JsonValue
from pydantic_ai.messages import BinaryContent

from nous.content.contracts import Artifact, InputGroup, ProcessingResult, SourceSelection


@dataclass(frozen=True)
class PreparedSource:
    key: str
    name: str
    text: str | None = None
    media: tuple[BinaryContent, ...] = ()
    group: InputGroup | None = None
    metadata: dict[str, JsonValue] = field(default_factory=dict)


@dataclass(frozen=True)
class PreparedBatch:
    sources: tuple[PreparedSource, ...]
    inputs: tuple[InputGroup, ...]
    selection: SourceSelection
    coverage: dict[str, JsonValue]


@dataclass(frozen=True)
class PreparedMedia:
    artifact: Artifact
    selection: SourceSelection
    batches: tuple[PreparedBatch, ...]
    coverage: dict[str, JsonValue]
    completed_result_refs: tuple[str, ...] = ()


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
