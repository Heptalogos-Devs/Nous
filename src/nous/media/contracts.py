from dataclasses import dataclass

from pydantic import JsonValue

from nous.content.contracts import Artifact, ProcessingResult, ResultInput, SourceSelection
from nous.models.contracts import PreparedSource


@dataclass(frozen=True)
class PreparedBatch:
    sources: tuple[PreparedSource, ...]
    inputs: tuple[ResultInput, ...]
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
