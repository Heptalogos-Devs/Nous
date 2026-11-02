"""Durable, bounded parameter selection over supplied relevance judgments."""

from __future__ import annotations

import itertools
from statistics import fmean
from time import perf_counter
from typing import TYPE_CHECKING
from uuid import uuid4

import anyio
import optuna
import sqlalchemy as sa
from ir_measures import P, R, calc_aggregate, nDCG
from nous_sdk.contracts import CallContext, CognitiveRef, Contract, DomainError
from nous_sdk.contracts.context import CognitiveTarget, MediaTarget
from nous_sdk.contracts.references import EvidenceTarget, ObservationTarget
from nous_sdk.contracts.retrieval_policy import RetrievalPolicy
from nous_sdk.contracts.tuning import (
    ApplyRetrievalProfile,
    CreateTuning,
    RankingMetrics,
    RollbackRetrievalProfile,
    TuningCase,
    TuningQueryResult,
    TuningRound,
    TuningRoundsPage,
    TuningTask,
    TuningTasksPage,
)
from nous_sdk.types import JsonObject
from pydantic import TypeAdapter

from nous.identity.access import require_namespace_in
from nous.platform.errors import NousError
from nous.platform.execution import JobSpec
from nous.platform.persistence import ReadView, WriteTx, metadata

if TYPE_CHECKING:
    from nous.cognition.records import CognitiveRecords
    from nous.data_management.resources import ResourceService
    from nous.material.service import MaterialService
    from nous.platform.configuration.service import ConfigurationService
    from nous.platform.execution import DurableExecution
    from nous.platform.persistence import Authority
    from nous.references.service import ReferenceDirectory

    from .service import RetrievalService

tasks = sa.Table(
    "retrieval_tuning_tasks",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False),
    sa.Column("caller_id", sa.String, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
)
rounds = sa.Table(
    "retrieval_tuning_rounds",
    metadata,
    sa.Column("task_id", sa.String, primary_key=True),
    sa.Column("index", sa.Integer, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
)
_JSON = TypeAdapter(JsonObject)
_MEASURES = (P @ 5, R @ 10, nDCG @ 10)
_METRICS = ("precision_at_5", "recall_at_10", "ndcg_at_10")


class SavedTask(Contract):
    context: CallContext
    request: CreateTuning | None
    profiles: tuple[RetrievalPolicy, ...]
    snapshot_commit: int
    job_id: str
    material_ids: tuple[str, ...]
    model_configuration: JsonObject | None = None
    best_index: int | None = None
    holdout: tuple[TuningQueryResult, ...] = ()
    baseline_holdout: tuple[TuningQueryResult, ...] = ()
    purged: bool = False


def configured_profile(baseline: RetrievalPolicy, changes: dict[str, float]) -> RetrievalPolicy:
    value = _JSON.validate_python(baseline.model_dump(mode="json", by_alias=False))
    for path, number in changes.items():
        components = path.split(".")
        target = value
        for component in components[:-1]:
            child = target[component]
            if not isinstance(child, dict):
                raise ValueError("Tuning parameter does not identify an algorithm setting")
            target = child
        target[components[-1]] = number
    return RetrievalPolicy.model_validate(value)


def candidates(baseline: RetrievalPolicy, command: CreateTuning) -> tuple[RetrievalPolicy, ...]:
    values = [baseline]
    if command.profiles:
        values.extend(command.profiles)
    elif command.parameters:
        space = {parameter.name: parameter.choices for parameter in command.parameters}
        for parameter in command.parameters:
            for choice in parameter.choices:
                configured_profile(baseline, {parameter.name: choice})
        combinations = 1
        for choices in space.values():
            combinations *= len(choices)
        if combinations <= command.max_trials - 1:
            values.extend(
                configured_profile(baseline, dict(zip(space, choices, strict=True)))
                for choices in itertools.product(*space.values())
            )
        else:
            # RandomSampler's public independent-sampling API plans the bounded
            # parameter candidates. Measured results remain in this domain's DBOS work.
            sampler = optuna.samplers.RandomSampler(seed=command.seed)
            study = optuna.create_study(sampler=sampler, direction="maximize")
            trial = optuna.trial.create_trial(state=optuna.trial.TrialState.RUNNING)
            for _ in range(command.max_trials - 1):
                selection = {
                    name: float(
                        sampler.sample_independent(
                            study,
                            trial,
                            name,
                            optuna.distributions.CategoricalDistribution(choices),
                        )
                    )
                    for name, choices in space.items()
                }
                values.append(configured_profile(baseline, selection))
    unique: dict[str, RetrievalPolicy] = {}
    for value in values:
        unique.setdefault(value.model_dump_json(), value)
    return tuple(unique.values())


def metric_values(case: TuningCase, ranking: tuple[str, ...]) -> dict[str, float]:
    measures = calc_aggregate(
        _MEASURES,
        {case.id: case.relevance},
        {
            case.id: {
                reference: float(len(ranking) - index) for index, reference in enumerate(ranking)
            }
        },
    )
    return {
        name: float(measures[measure]) for name, measure in zip(_METRICS, _MEASURES, strict=True)
    }


def aggregate(results: tuple[TuningQueryResult, ...]) -> RankingMetrics:
    return RankingMetrics(
        **{name: fmean(result.metrics[name] for result in results) for name in _METRICS},
        mean_latency_ms=fmean(result.latency_ms for result in results),
        failed_queries=sum(result.error is not None for result in results),
        empty_queries=sum(not result.ranking for result in results),
    )


def preference(round: TuningRound, objective: str) -> tuple[float, ...]:
    metrics = round.metrics
    values = {
        "precision_priority": (metrics.precision_at_5, metrics.recall_at_10, metrics.ndcg_at_10),
        "recall_priority": (metrics.recall_at_10, metrics.precision_at_5, metrics.ndcg_at_10),
        "balanced": (metrics.ndcg_at_10, metrics.precision_at_5, metrics.recall_at_10),
    }
    return (*values[objective], -metrics.mean_latency_ms, -round.index)


class RetrievalTuningService:
    def __init__(
        self,
        authority: Authority,
        *,
        cognition: CognitiveRecords,
        material: MaterialService,
        references: ReferenceDirectory,
        resources: ResourceService,
        configuration: ConfigurationService,
        execution: DurableExecution,
        retrieval: RetrievalService,
    ) -> None:
        self.authority = authority
        self.cognition = cognition
        self.material = material
        self.references = references
        self.resources = resources
        self.configuration = configuration
        self.execution = execution
        self.retrieval = retrieval

    async def saved_in(self, view: ReadView | WriteTx, ctx: CallContext, task_id: str) -> SavedTask:
        await require_namespace_in(view, ctx)
        raw = await view.connection.scalar(
            sa.select(tasks.c.record).where(
                tasks.c.id == task_id,
                tasks.c.namespace_id == ctx.namespace_id,
                tasks.c.caller_id == ctx.caller_id,
            )
        )
        if raw is None:
            raise DomainError(
                "tuning_task_not_found", "Tuning task is unavailable in this caller scope"
            )
        saved = SavedTask.model_validate(raw)
        if not saved.purged and saved.request:
            for case in saved.request.cases:
                for reference in case.relevance:
                    await self.references.resolve_in(view, ctx, reference)
        return saved

    async def create(self, ctx: CallContext, command: CreateTuning) -> TuningTask:
        async with self.authority.write(ctx) as tx:
            await require_namespace_in(tx, ctx)
            snapshot = tx.commit_seq - 1
            baseline = RetrievalPolicy.model_validate((await self.configuration.effective(ctx, 'retrieval'))['value'])
            material_ids: set[str] = set()
            source_map = await self.cognition.source_roots_in(tx, ctx)
            cases: list[TuningCase] = []
            for case in command.cases:
                relevance: dict[str, int] = {}
                for locator, grade in case.relevance.items():
                    resolved = await self.references.resolve_in(tx, ctx, locator, snapshot)
                    exact = resolved.reference.reference.exact()
                    relevance[exact] = max(grade, relevance.get(exact, 0))
                    target = resolved.reference
                    if isinstance(target, CognitiveTarget):
                        roots = (await self.cognition.get_in(tx, ctx, target.reference)).source_roots
                    elif isinstance(target, MediaTarget):
                        provenance = await self.material.provenance_in(tx, ctx, target.reference)
                        material_ids.update(provenance.material_ids)
                        roots = provenance.roots
                    elif isinstance(target, (ObservationTarget, EvidenceTarget)):
                        roots = (await self.references.read_source_object_in(tx, ctx, target.reference)).source_roots
                    else:
                        roots = (await self.resources.get_in(tx, ctx, target.reference.id, target.reference.revision)).source_roots
                    material_ids.update((source_map[root] for root in roots if root in source_map))
                cases.append(case.model_copy(update={'relevance': relevance}))
            frozen = command.model_copy(update={'cases': tuple(cases)})
            profiles = candidates(baseline, frozen)
            identity = str(uuid4())
            job = await tx.enqueue(JobSpec(workflow_name='nous.retrieval.tune', queue_name='default', args=(identity,), scope=ctx.namespace_id))
            saved = SavedTask(context=ctx, request=frozen, profiles=profiles, snapshot_commit=snapshot, job_id=job.workflow_id, material_ids=tuple(sorted(material_ids)), model_configuration=_JSON.validate_python((await self.configuration.effective(ctx, 'models'))['value']))
            await tx.connection.execute(tasks.insert().values(id=identity, namespace_id=ctx.namespace_id, caller_id=ctx.caller_id, record=saved.model_dump(mode='json')))
        return await self.get(ctx, identity)

    async def read_rounds(
        self, ctx: CallContext, task_id: str, *, limit: int = 100, after: int | None = None
    ) -> TuningRoundsPage:
        if not 1 <= limit <= 100:
            raise ValueError("Round page limit must be between 1 and 100")
        async with self.authority.read(ctx) as view:
            await self.saved_in(view, ctx, task_id)
            statement = sa.select(rounds.c.record).where(rounds.c.task_id == task_id)
            if after is not None:
                statement = statement.where(rounds.c.index > after)
            values = tuple(
                TuningRound.model_validate(row)
                for row in (
                    await view.connection.execute(
                        statement.order_by(rounds.c.index).limit(limit + 1)
                    )
                ).scalars()
            )
            for value in values:
                for query in value.queries:
                    for locator in query.ranking:
                        await self.references.resolve_in(view, ctx, locator)
        return TuningRoundsPage(
            items=values[:limit],
            next_cursor=values[limit - 1].index if len(values) > limit else None,
        )

    async def get(self, ctx: CallContext, task_id: str) -> TuningTask:
        async with self.authority.read(ctx) as view:
            saved = await self.saved_in(view, ctx, task_id)
        results = (await self.read_rounds(ctx, task_id)).items
        status = await self.execution.status(ctx, saved.job_id)
        best = next((item for item in results if item.index == saved.best_index), None)
        return TuningTask(
            id=task_id,
            job_id=saved.job_id,
            status=status.status,
            snapshot_commit=saved.snapshot_commit,
            request=saved.request,
            baseline=saved.profiles[0],
            total_trials=len(saved.profiles),
            completed_trials=len(results),
            best_index=saved.best_index,
            best_metrics=best.metrics if best else None,
            holdout=saved.holdout,
            baseline_holdout=saved.baseline_holdout,
            purged=saved.purged,
        )

    async def list(
        self, ctx: CallContext, *, limit: int = 50, cursor: str | None = None
    ) -> TuningTasksPage:
        if not 1 <= limit <= 100:
            raise ValueError("Task page limit must be between 1 and 100")
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            statement = sa.select(tasks.c.id).where(
                tasks.c.namespace_id == ctx.namespace_id,
                tasks.c.caller_id == ctx.caller_id,
            )
            if cursor:
                statement = statement.where(tasks.c.id > cursor)
            ids = tuple(
                str(value)
                for value in (
                    await view.connection.execute(statement.order_by(tasks.c.id).limit(limit + 1))
                ).scalars()
            )
        return TuningTasksPage(
            items=tuple([await self.get(ctx, identity) for identity in ids[:limit]]),
            next_cursor=ids[limit - 1] if len(ids) > limit else None,
        )

    async def evaluate(
        self, ctx: CallContext, saved: SavedTask, profile: RetrievalPolicy, split: str
    ) -> tuple[TuningQueryResult, ...]:
        if saved.request is None:
            raise DomainError("tuning_sources_purged", "Tuning input was permanently cleared")
        if saved.model_configuration is None:
            raise DomainError(
                "tuning_model_snapshot_unavailable",
                "Submit a new tuning request with the current model resources",
            )
        results: list[TuningQueryResult] = []
        for case in saved.request.cases:
            if case.split != split:
                continue
            if (await self.configuration.effective(ctx, "models"))[
                "value"
            ] != saved.model_configuration:
                raise DomainError(
                    "tuning_model_resources_changed",
                    "Model resources changed during tuning; submit a new request",
                )
            query = case.query.model_copy(
                update={
                    "profile": profile,
                    "as_of_commit": saved.snapshot_commit,
                    "limit": max(case.query.limit, 10),
                }
            )
            started = perf_counter()
            ranking: tuple[str, ...] = ()
            error: dict[str, object] | None = None
            coverage_complete, candidate_limited, degraded = True, False, False
            reasons: tuple[str, ...] = ()
            try:
                with anyio.fail_after(saved.request.query_timeout_seconds):
                    result = await self.retrieval.query(ctx, query)
                if (await self.configuration.effective(ctx, "models"))[
                    "value"
                ] != saved.model_configuration:
                    raise DomainError(
                        "tuning_model_resources_changed", "Model resources changed during the query"
                    )
                coverage_complete = result.coverage.complete
                if result.execution is not None:
                    candidate_limited = result.execution.candidate_limited
                    degraded, reasons = result.execution.degraded, result.execution.reasons
                ranking = tuple(hit.object.reference.exact() for hit in result.hits)
            except (DomainError, NousError, TimeoutError) as failure:
                error = {"code": getattr(failure, "code", "query_timeout"), "message": str(failure)}
            elapsed = (perf_counter() - started) * 1000
            results.append(
                TuningQueryResult(
                    query_id=case.id,
                    ranking=ranking,
                    latency_ms=elapsed,
                    metrics=metric_values(case, ranking),
                    error=error,
                    coverage_complete=coverage_complete,
                    candidate_limited=candidate_limited,
                    degraded=degraded,
                    reasons=reasons,
                )
            )
        return tuple(results)

    async def run(self, task_id: str) -> dict[str, str]:
        async with self.authority.read() as view:
            raw = await view.connection.scalar(
                sa.select(tasks.c.record).where(tasks.c.id == task_id)
            )
        if raw is None:
            raise DomainError("tuning_task_not_found", "Tuning task is unavailable")
        saved = SavedTask.model_validate(raw)
        ctx, command = saved.context, saved.request
        if command is None or saved.purged:
            raise DomainError("tuning_sources_purged", "Tuning input was permanently cleared")
        for index, profile in enumerate(saved.profiles):
            async with self.authority.read(ctx) as view:
                current = await self.saved_in(view, ctx, task_id)
                existing = await view.connection.scalar(
                    sa.select(rounds.c.index).where(
                        rounds.c.task_id == task_id, rounds.c.index == index
                    )
                )
            if current.purged:
                raise DomainError("tuning_sources_purged", "Tuning input was permanently cleared")
            if existing is not None:
                continue
            results = await self.evaluate(ctx, saved, profile, "development")
            metrics = aggregate(results)
            measured = TuningRound(
                index=index,
                profile=profile,
                metrics=metrics,
                queries=results,
                feasible=not metrics.failed_queries
                and (
                    command.max_mean_latency_ms is None
                    or metrics.mean_latency_ms <= command.max_mean_latency_ms
                ),
            )
            async with self.authority.write(ctx) as tx:
                current = await self.saved_in(tx, ctx, task_id)
                if current.purged:
                    raise DomainError('tuning_sources_purged', 'Tuning input was permanently cleared')
                await tx.connection.execute(rounds.insert().values(task_id=task_id, index=index, namespace_id=ctx.namespace_id, record=measured.model_dump(mode='json')))
        choices = [item for item in (await self.read_rounds(ctx, task_id)).items if item.feasible]
        if not choices:
            raise DomainError(
                "tuning_no_feasible_profile",
                "No candidate completed within the supplied constraints",
            )
        best = max(choices, key=lambda item: preference(item, command.objective))
        # Holdout is evaluated only after the development set has fixed the winner.
        holdout = await self.evaluate(ctx, saved, best.profile, "holdout")
        baseline_holdout = (
            await self.evaluate(ctx, saved, saved.profiles[0], "holdout") if best.index else holdout
        )
        async with self.authority.write(ctx) as tx:
            current = await self.saved_in(tx, ctx, task_id)
            if current.purged:
                raise DomainError('tuning_sources_purged', 'Tuning input was permanently cleared')
            current = current.model_copy(update={'best_index': best.index, 'holdout': holdout, 'baseline_holdout': baseline_holdout})
            await tx.connection.execute(tasks.update().where(tasks.c.id == task_id).values(record=current.model_dump(mode='json')))
        return {"task_id": task_id}

    async def cancel(self, ctx: CallContext, task_id: str) -> TuningTask:
        value = await self.get(ctx, task_id)
        await self.execution.cancel(ctx, value.job_id)
        return await self.get(ctx, task_id)

    async def profile(self, ctx: CallContext) -> JsonObject:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
        effective = await self.configuration.effective(ctx, "retrieval")
        effective["version"] = self.configuration.values.version(f"namespace:{ctx.namespace_id}")
        return _JSON.validate_python(effective)

    async def change_profile(
        self, ctx: CallContext, command: ApplyRetrievalProfile | RollbackRetrievalProfile
    ) -> JsonObject:
        if ctx.source_groups():
            raise DomainError(
                "tuning_scope_forbidden", "Profile changes require the complete Subject scope"
            )
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            version = self.configuration.values.version(f"namespace:{ctx.namespace_id}")
            if command.expected_version is not None and command.expected_version != version:
                raise DomainError(
                    "retrieval_profile_conflict",
                    "Retrieval profile changed since it was read",
                    expected=command.expected_version,
                    current=version,
                )
            if isinstance(command, ApplyRetrievalProfile):
                value = command.profile
                if command.task_id:
                    saved = await self.saved_in(view, ctx, command.task_id)
                    if saved.purged or saved.best_index is None:
                        raise DomainError(
                            "tuning_not_ready", "Select a completed, retained tuning result"
                        )
                    models = await self.configuration.effective(ctx, "models")
                    if models["value"] != saved.model_configuration:
                        raise DomainError(
                            "tuning_model_resources_changed",
                            "The selected result used different model resources",
                        )
                    value = saved.profiles[saved.best_index]
                assert value is not None
                target: object = value
            else:
                target = None
        result = await self.configuration.set(
            ctx, "retrieval", target, "namespace", expected_version=version
        )
        return _JSON.validate_python(result)

    async def purge_in(
        self,
        tx: WriteTx,
        *,
        material_ids: tuple[str, ...] = (),
        references: tuple[CognitiveRef, ...] = (),
    ) -> None:
        ids = {reference.id for reference in references}
        selected = set(material_ids)
        values = (
            await tx.connection.execute(
                sa.select(tasks.c.id, tasks.c.record).where(
                    tasks.c.namespace_id == tx.ctx.namespace_id
                )
            )
        ).all()
        for identity, raw in values:
            saved = SavedTask.model_validate(raw)
            if saved.purged or saved.request is None:
                continue
            mentioned = {
                locator.split("@", 1)[0].split(":", 1)[-1]
                for case in saved.request.cases
                for locator in case.relevance
            }
            prior = (
                await tx.connection.execute(
                    sa.select(rounds.c.record).where(rounds.c.task_id == identity)
                )
            ).scalars()
            mentioned.update(
                locator.split("@", 1)[0].split(":", 1)[-1]
                for raw_round in prior
                for result in TuningRound.model_validate(raw_round).queries
                for locator in result.ranking
            )
            if not selected.intersection(saved.material_ids) and not mentioned.intersection(ids):
                continue
            erased = saved.model_copy(
                update={"request": None, "holdout": (), "baseline_holdout": (), "purged": True}
            )
            await tx.connection.execute(
                tasks.update()
                .where(tasks.c.id == identity)
                .values(record=erased.model_dump(mode="json"))
            )
            await tx.connection.execute(rounds.delete().where(rounds.c.task_id == identity))
