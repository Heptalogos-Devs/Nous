"""Shared generation-based assessment and explicitly configured reranking."""

import asyncio
import json
import math
import time
from collections.abc import Sequence
from dataclasses import replace

from openai import APIConnectionError, APIStatusError
from openai import BaseModel as APIBaseModel
from pydantic import BaseModel, ConfigDict, JsonValue, ValidationError

from nous.application.contracts import CallContext, LanguageEvents, define_operation
from nous.common.errors import NousError
from nous.content.contracts import ModelProducer
from nous.media.contracts import PreparedSource
from nous.models.configuration import ModelRoute, TaskAssignment
from nous.models.contracts import ReadingInput
from nous.models.generation import GenerationOperations
from nous.models.policy import (
    attempt_scope,
    endpoint_path,
    error_report,
    model_error,
    retry_delay,
    retryable,
    stage_deadline,
)
from nous.models.requests import PreparedGeneration

from .evaluation_contracts import (
    CandidateEvaluation,
    EvaluationBatchFacts,
    EvaluationCandidate,
    EvaluationInputFacts,
    EvaluationOutcome,
    EvaluationResponse,
    EvaluationTask,
    RankedCandidate,
    RankOutcome,
    RankRequest,
    RankResponse,
)

EVALUATE = define_operation(
    "models.evaluate",
    request=EvaluationTask,
    result=EvaluationOutcome,
    events=LanguageEvents,
    description="Evaluate candidates using an explicit rubric.",
)
RANK = define_operation(
    "models.rank",
    request=RankRequest,
    result=RankOutcome,
    events=LanguageEvents,
    description="Rank candidate texts using a configured model task.",
)


class _WireRank(BaseModel):
    model_config = ConfigDict(extra="ignore")
    index: int
    relevance_score: float


class _WireResponse(APIBaseModel):
    model_config = ConfigDict(extra="ignore")
    id: str | None = None
    results: list[_WireRank]
    usage: dict[str, JsonValue] | None = None
    meta: dict[str, JsonValue] | None = None


def _validate_candidates(candidates: Sequence[EvaluationCandidate]) -> None:
    keys = [item.key for item in candidates]
    if not keys or len(set(keys)) != len(keys):
        raise NousError(
            "model.invalid_candidates", "Candidates must be nonempty and have unique keys"
        )


def _batches(
    candidates: Sequence[EvaluationCandidate], max_count: int, max_chars: int
) -> tuple[tuple[EvaluationCandidate, ...], ...]:
    batches: list[tuple[EvaluationCandidate, ...]] = []
    current: list[EvaluationCandidate] = []
    size = 0
    for item in candidates:
        length = len(item.text) + len(item.context or "")
        if length > max_chars:
            raise NousError(
                "model.input_limit",
                "A candidate exceeds the assessment input limit",
                details={"key": item.key},
            )
        if current and (len(current) == max_count or size + length > max_chars):
            batches.append(tuple(current))
            current, size = [], 0
        current.append(item)
        size += length
    if current:
        batches.append(tuple(current))
    return tuple(batches)


class EvaluationOperations:
    """Use the configured task and the runtime's shared model connections."""

    def __init__(self, generation: GenerationOperations) -> None:
        self.generation = generation

    async def _generate(
        self,
        task: str,
        contract: str,
        question: str,
        candidates: tuple[EvaluationCandidate, ...],
        instructions: str,
        call: CallContext,
    ) -> tuple[EvaluationResponse | RankResponse, EvaluationBatchFacts]:
        self.generation.require_contract(task, contract, call.settings)
        reading = ReadingInput(
            question=question,
            instructions=instructions,
            sources=tuple(
                PreparedSource(
                    key=item.key,
                    name=item.key,
                    text=item.text + (f"\n\nContext:\n{item.context}" if item.context else ""),
                )
                for item in candidates
            ),
        )
        started = time.monotonic()
        executor = self.generation.executor
        frozen = executor.freeze(task, call.settings)
        prepared = executor.prepare(frozen, reading, call.settings)

        async def prepare_route(route: ModelRoute) -> PreparedGeneration:
            return executor.prepare(replace(frozen, routes=(route,)), reading, call.settings)

        generated = await executor.execute(
            prepared, context=call.execution, prepare_route=prepare_route
        )
        response = (
            EvaluationResponse if contract == "evaluate-candidates" else RankResponse
        ).model_validate(generated.output)
        producer = ModelProducer.model_validate(generated.producer)
        usage = generated.usage
        return response, EvaluationBatchFacts(
            keys=tuple(item.key for item in candidates),
            producer=producer,
            usage=usage,
            elapsed_seconds=time.monotonic() - started,
        )

    async def evaluate(self, request: EvaluationTask, call: CallContext) -> EvaluationOutcome:
        _validate_candidates(request.candidates)
        config = call.settings.values.evaluation
        items: list[CandidateEvaluation] = []
        facts: list[EvaluationBatchFacts] = []
        for candidates in _batches(
            request.candidates, config.max_candidates, config.max_characters
        ):
            response, batch = await self._generate(
                request.task,
                "evaluate-candidates",
                request.question,
                candidates,
                json.dumps(
                    {
                        "rubric": request.rubric,
                        "purpose": request.purpose,
                        "scale": request.scale.model_dump(mode="json"),
                    },
                    ensure_ascii=False,
                ),
                call,
            )
            assert isinstance(response, EvaluationResponse)
            items.extend(
                CandidateEvaluation(
                    key=item.key,
                    value=item.value,
                    scale=request.scale.name,
                    distribution=item.distribution,
                )
                for item in response.items
            )
            facts.append(batch)
        return EvaluationOutcome(
            items=tuple(items),
            producer=facts[0].producer,
            usage={"batches": [item.usage for item in facts]},
            actual_input=EvaluationInputFacts(
                question=request.question,
                candidates=request.candidates,
                rubric=request.rubric,
                purpose=request.purpose,
                scale=request.scale,
            ),
            batches=tuple(facts),
        )

    async def rank(self, request: RankRequest, call: CallContext) -> RankOutcome:
        _validate_candidates(request.candidates)
        config = call.settings.values.evaluation
        batches = _batches(request.candidates, config.max_candidates, config.max_characters)
        if len(batches) != 1:
            raise NousError("model.input_limit", "Ranking requires one comparable candidate batch")
        assignment, routes = self.generation.executor.registry.resolve(request.task, call.settings)
        if assignment.operation == "generation":
            response, batch = await self._generate(
                request.task, "rank-candidates", request.question, batches[0], "", call
            )
            assert isinstance(response, RankResponse)
            items = tuple(RankedCandidate(key=item.key) for item in response.items)
        elif assignment.operation == "rerank":
            items, batch = await self._rerank(request, call, assignment, routes)
        else:
            raise NousError(
                "model.operation_mismatch", "The configured task does not rank candidates"
            )
        return RankOutcome(
            items=items,
            producer=batch.producer,
            usage=batch.usage,
            actual_input=EvaluationInputFacts(
                question=request.question, candidates=request.candidates
            ),
            batches=(batch,),
        )

    async def _rerank(
        self,
        request: RankRequest,
        call: CallContext,
        assignment: TaskAssignment,
        routes: tuple[ModelRoute, ...],
    ) -> tuple[tuple[RankedCandidate, ...], EvaluationBatchFacts]:
        settings = call.settings.values
        max_attempts = min(
            settings.model_execution.max_attempts,
            assignment.budget.max_attempts or settings.model_execution.max_attempts,
        )
        timeout = min(
            settings.model_execution.timeout_seconds,
            assignment.budget.timeout_seconds or settings.model_execution.timeout_seconds,
        )
        started = time.monotonic()
        documents = [
            item.text + (f"\n\nContext:\n{item.context}" if item.context else "")
            for item in request.candidates
        ]
        async with asyncio.timeout_at(stage_deadline(timeout, call.execution)):
            for attempt in range(1, max_attempts + 1):
                route = routes[min(attempt - 1, len(routes) - 1)]
                provider = settings.providers[route.model.provider]
                if provider.protocol != "http-rerank" or "rerank" not in route.model.capabilities:
                    raise NousError(
                        "model.capability_missing",
                        "Rerank requires an explicitly configured rerank protocol and capability",
                    )
                client = self.generation.executor.connections.client(route.model.provider)
                with attempt_scope(
                    attempt,
                    {
                        "model_id": route.model_id,
                        "provider": route.model.provider,
                        "protocol": provider.protocol,
                        "endpoint_path": endpoint_path(
                            call.settings, route.model.provider, "rerank"
                        ),
                    },
                    request.task,
                ) as scope:
                    try:
                        response = await client.post(
                            "/rerank",
                            cast_to=_WireResponse,
                            body={
                                "model": route.model.model_name,
                                "query": request.question,
                                "documents": documents,
                            },
                            options={
                                "timeout": self.generation.executor.connections.timeout(
                                    route.model.provider, call.settings
                                )
                            },
                        )
                        try:
                            response = _WireResponse.model_validate(
                                response.model_dump(warnings=False)
                            )
                        except ValidationError as error:
                            scope.complete(outcome="error", error_code="model.invalid_output")
                            raise NousError(
                                "model.invalid_output",
                                "Rerank returned an invalid response",
                                cause=error,
                            ) from error
                        indices = [item.index for item in response.results]
                        if (
                            len(indices) != len(set(indices))
                            or any(
                                index < 0 or index >= len(request.candidates) for index in indices
                            )
                            or any(
                                not math.isfinite(item.relevance_score) for item in response.results
                            )
                        ):
                            raise NousError(
                                "model.invalid_output",
                                "Rerank returned invalid candidate indices or scores",
                            )
                        usage = response.usage or response.meta or {}
                        producer = ModelProducer(
                            provider=route.model.provider,
                            protocol=provider.protocol,
                            model_id=route.model_id,
                            requested_model_name=route.model.model_name,
                            request_id=response.id,
                        )
                        items = tuple(
                            RankedCandidate(
                                key=request.candidates[item.index].key,
                                score=item.relevance_score,
                                scale="rerank_relevance_score",
                            )
                            for item in response.results
                        )
                        scope.complete(
                            outcome="success", provider_request_id=response.id, reported_usage=usage
                        )
                        return items, EvaluationBatchFacts(
                            keys=tuple(item.key for item in request.candidates),
                            producer=producer,
                            usage=usage,
                            elapsed_seconds=time.monotonic() - started,
                        )
                    except (APIConnectionError, APIStatusError, TimeoutError) as error:
                        retry = attempt < max_attempts and retryable(error)
                        scope.complete(
                            **error_report(error, next_action="next_route" if retry else "raise")
                        )
                        if not retry:
                            raise model_error(error, attempt) from error
                        await retry_delay(error)
        raise AssertionError("Attempt limit must be positive")
