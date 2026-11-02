"""Bounded retrieval tuning over supplied queries, judgments and current parameters."""

from typing import Literal

from pydantic import Field, FiniteFloat, model_validator

from .common import Contract, Operation
from .retrieval import QueryRequest
from .retrieval_policy import RetrievalPolicy

TuningObjective = Literal["precision_priority", "recall_priority", "balanced"]
TuningParameterName = Literal[
    "lexical_weight",
    "dense_weight",
    "rrf_k",
    "vcp.candidate_limit",
    "vcp.pipeline.max_levels",
    "vcp.pipeline.spike_routing.max_safe_hops",
    "vcp.readout.lexical_weight",
    "vcp.readout.max_union_candidates",
]


class TuningCase(Contract):
    id: str = Field(min_length=1, max_length=256)
    query: QueryRequest
    relevance: dict[str, int] = Field(min_length=1, max_length=1000)
    split: Literal["development", "holdout"] = "development"

    @model_validator(mode="after")
    def judgments(self) -> "TuningCase":
        if any(grade < 0 for grade in self.relevance.values()) or not any(self.relevance.values()):
            raise ValueError("Each query requires nonnegative judgments and a relevant target")
        if self.query.current != "none":
            raise ValueError(
                "Tuning queries use accepted snapshots; refresh resources before tuning"
            )
        if self.query.profile is not None:
            raise ValueError("Supply candidate profiles on the tuning request")
        return self


class TuningParameter(Contract):
    name: TuningParameterName
    choices: tuple[FiniteFloat, ...] = Field(min_length=1, max_length=64)


class CreateTuning(Operation):
    cases: tuple[TuningCase, ...] = Field(min_length=1, max_length=500)
    parameters: tuple[TuningParameter, ...] = Field(default=(), max_length=8)
    profiles: tuple[RetrievalPolicy, ...] = Field(default=(), max_length=63)
    objective: TuningObjective = "balanced"
    max_trials: int = Field(default=8, ge=1, le=64)
    seed: int = 0
    query_timeout_seconds: FiniteFloat = Field(default=120, gt=0, le=3600)
    max_mean_latency_ms: FiniteFloat | None = Field(default=None, gt=0)

    @model_validator(mode="after")
    def development_set(self) -> "CreateTuning":
        if len({case.id for case in self.cases}) != len(self.cases):
            raise ValueError("Query ids must be unique")
        if len({parameter.name for parameter in self.parameters}) != len(self.parameters):
            raise ValueError("Each tuning parameter can be declared once")
        if not any(case.split == "development" for case in self.cases):
            raise ValueError(
                "Tuning requires development queries; holdout queries stay outside selection"
            )
        if self.profiles and self.parameters:
            raise ValueError("Supply complete profiles or parameter choices")
        if len(self.profiles) + 1 > self.max_trials:
            raise ValueError(
                "The trial budget must include the baseline and every supplied profile"
            )
        return self


class RankingMetrics(Contract):
    precision_at_5: float
    recall_at_10: float
    ndcg_at_10: float
    mean_latency_ms: float
    failed_queries: int = 0
    empty_queries: int = 0


class TuningQueryResult(Contract):
    query_id: str
    ranking: tuple[str, ...]
    latency_ms: float
    metrics: dict[str, float]
    error: dict[str, object] | None = None
    coverage_complete: bool = True
    candidate_limited: bool = False
    degraded: bool = False
    reasons: tuple[str, ...] = ()


class TuningRound(Contract):
    index: int
    profile: RetrievalPolicy
    metrics: RankingMetrics
    queries: tuple[TuningQueryResult, ...]
    feasible: bool


class TuningTask(Contract):
    id: str
    job_id: str
    status: str
    snapshot_commit: int
    request: CreateTuning | None
    baseline: RetrievalPolicy
    total_trials: int
    completed_trials: int
    best_index: int | None = None
    best_metrics: RankingMetrics | None = None
    holdout: tuple[TuningQueryResult, ...] = ()
    baseline_holdout: tuple[TuningQueryResult, ...] = ()
    purged: bool = False


class TuningTasksPage(Contract):
    items: tuple[TuningTask, ...]
    next_cursor: str | None = None


class TuningRoundsPage(Contract):
    items: tuple[TuningRound, ...]
    next_cursor: int | None = None


class ApplyRetrievalProfile(Operation):
    profile: RetrievalPolicy | None = None
    task_id: str | None = None
    expected_version: int | None = Field(default=None, ge=0)

    @model_validator(mode="after")
    def selection(self) -> "ApplyRetrievalProfile":
        if (self.profile is None) == (self.task_id is None):
            raise ValueError("Select a profile or a completed tuning task")
        return self


class RollbackRetrievalProfile(Operation):
    expected_version: int | None = Field(default=None, ge=0)
