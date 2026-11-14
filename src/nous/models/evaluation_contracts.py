"""Provider-neutral candidate evaluation and ranking values."""

import math

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator

from nous.content.contracts import ModelProducer


class EvaluationValue(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class EvaluationCandidate(EvaluationValue):
    key: str = Field(min_length=1, pattern=r"^\S+$")
    text: str
    context: str | None = None


class EvaluationScale(EvaluationValue):
    """One declared categorical or bounded numerical scale for a whole task."""

    name: str = Field(min_length=1)
    labels: tuple[str, ...] = ()
    minimum: float | None = None
    maximum: float | None = None

    @model_validator(mode="after")
    def valid_scale(self) -> EvaluationScale:
        if self.labels:
            if self.minimum is not None or self.maximum is not None:
                raise ValueError("Choose categorical labels or numerical bounds")
            if any(not label for label in self.labels) or len(set(self.labels)) != len(self.labels):
                raise ValueError("Scale labels must be nonempty and unique")
        elif (
            self.minimum is None
            or self.maximum is None
            or not math.isfinite(self.minimum)
            or not math.isfinite(self.maximum)
            or self.minimum >= self.maximum
        ):
            raise ValueError("A numerical scale needs finite increasing bounds")
        return self


class EvaluationTask(EvaluationValue):
    question: str
    candidates: tuple[EvaluationCandidate, ...]
    rubric: tuple[str, ...]
    purpose: str
    scale: EvaluationScale
    task: str = "evaluate-passages"


class CandidateEvaluation(EvaluationValue):
    key: str
    value: float | str
    scale: str
    distribution: dict[str, float] | None = None


class EvaluationInputFacts(EvaluationValue):
    question: str
    candidates: tuple[EvaluationCandidate, ...]
    rubric: tuple[str, ...] = ()
    purpose: str = ""
    scale: EvaluationScale | None = None


class EvaluationBatchFacts(EvaluationValue):
    keys: tuple[str, ...]
    producer: ModelProducer
    usage: dict[str, JsonValue]
    elapsed_seconds: float = Field(ge=0)


class EvaluationOutcome(EvaluationValue):
    items: tuple[CandidateEvaluation, ...]
    producer: ModelProducer
    usage: dict[str, JsonValue]
    actual_input: EvaluationInputFacts
    batches: tuple[EvaluationBatchFacts, ...] = ()


class RankRequest(EvaluationValue):
    question: str
    candidates: tuple[EvaluationCandidate, ...]
    task: str = "rank-passages"


class RankedCandidate(EvaluationValue):
    key: str
    score: float | None = None
    scale: str = "rank"


class RankOutcome(EvaluationValue):
    items: tuple[RankedCandidate, ...]
    producer: ModelProducer
    usage: dict[str, JsonValue]
    actual_input: EvaluationInputFacts
    batches: tuple[EvaluationBatchFacts, ...] = ()


class EvaluationDecision(EvaluationValue):
    key: str
    value: float | str
    distribution: dict[str, float] | None = None


class EvaluationResponse(EvaluationValue):
    items: tuple[EvaluationDecision, ...]


class RankChoice(EvaluationValue):
    key: str


class RankResponse(EvaluationValue):
    items: tuple[RankChoice, ...]
