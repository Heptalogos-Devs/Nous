"""Durable proposal generation and explicit, atomic review of cognitive growth."""

from typing import TYPE_CHECKING
from uuid import uuid4

import sqlalchemy as sa
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.growth import GrowthEntry, GrowthSnapshot
from nous_sdk.contracts.maintenance import (
    MaintenanceCatalog,
    MaintenanceProposal,
    MaintenanceReview,
    MaintenanceRun,
    MaintenanceSubmission,
    ReviewMaintenance,
    SnapshotReference,
    StartMaintenance,
)
from pydantic import JsonValue, TypeAdapter

from nous.cognition import growth_sources
from nous.cognition.growth import GrowthService
from nous.cognition.lifecycle import selectable
from nous.cognition.projection import media_references_in
from nous.cognition.records import CognitiveRecords
from nous.identity.access import require_namespace_in
from nous.material.service import observations
from nous.platform.configuration.service import ConfigurationService
from nous.platform.errors import NousError
from nous.platform.execution import DurableExecution, JobSpec
from nous.platform.models import ModelService
from nous.platform.observability import operation_span
from nous.platform.persistence import Authority, OperationConflict, ReadView, WriteTx
from nous.retrieval.service import RetrievalService

from . import natural, proposals, scheduling, tasks
from .generation import generate_validated
from .privacy import redact

if TYPE_CHECKING:
    from nous.imports.service import ImportService
from .schema import runs


class MaintenanceService:
    journal_create = tasks.journal_create
    reflect = tasks.reflect
    dream = tasks.dream
    summarize_work = tasks.summarize_work
    edit_proposal = proposals.edit_proposal
    working_snapshot_in = proposals.working_snapshot_in
    create_schedule = scheduling.create_schedule
    list_schedules = scheduling.list_schedules
    stop_schedule = scheduling.stop_schedule
    reconcile_schedules = scheduling.reconcile_schedules
    run_tick = scheduling.run_tick

    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        models: ModelService,
        execution: DurableExecution,
        *,
        configuration: ConfigurationService | None = None,
        imports: ImportService | None = None,
        growth: GrowthService,
        retrieval: RetrievalService,
    ) -> None:
        self.retrieval = retrieval
        self.growth = growth
        self.authority = authority
        self.cognition = cognition
        self.models = models
        self.execution = execution
        self.configuration = configuration
        self.imports = imports

    async def run_in(
        self, view: ReadView | WriteTx, ctx: CallContext, run_id: str
    ) -> MaintenanceRun:
        raw = await view.connection.scalar(
            sa.select(runs.c.record).where(
                runs.c.id == run_id, runs.c.namespace_id == ctx.namespace_id
            )
        )
        if raw is None:
            raise DomainError(
                "maintenance_run_not_found", "Maintenance run is unavailable in this subject"
            )
        return MaintenanceRun.model_validate(raw)

    async def snapshot_in(
        self, view: ReadView | WriteTx, ctx: CallContext, catalog: MaintenanceCatalog
    ) -> GrowthSnapshot:
        subject = await require_namespace_in(view, ctx)
        entries = tuple(
            [
                GrowthEntry(
                    key=entry.key, record=await self.cognition.get_in(view, ctx, entry.reference)
                )
                for entry in catalog.entries
            ]
        )
        if any(not selectable(entry.record) for entry in entries):
            raise DomainError(
                "maintenance_snapshot_inactive", "Automatic maintenance requires active cognition"
            )
        material_sources = await growth_sources.read_in(self.cognition, view, ctx, catalog.sources)
        snapshot = GrowthSnapshot(
            watermark=catalog.watermark,
            entries=entries,
            material_ids=catalog.material_ids,
            capabilities=TypeAdapter(tuple[str, ...]).validate_python(subject.enabled_features),
            sources=material_sources,
        )
        from nous.cognition.growth_sources import evidence_in

        await evidence_in(self.cognition, view, ctx, snapshot)
        return snapshot

    async def _project_in(
        self, view: ReadView | WriteTx, ctx: CallContext, record: MaintenanceRun
    ) -> MaintenanceRun:
        available = record.purged_at is None and (
            not (record.catalog is None and ctx.source_groups())
        )
        if available and record.catalog is not None:
            try:
                await self.snapshot_in(view, ctx, record.catalog)
            except DomainError as error:
                if error.code not in {"source_unavailable", "cognition_not_found"}:
                    raise
                available = False
        if available:
            return record
        return redact(record)

    async def read(self, ctx: CallContext, run_id: str) -> MaintenanceRun:
        async with self.authority.read(ctx) as view:
            return await self._project_in(view, ctx, await self.run_in(view, ctx, run_id))

    async def existing_seed(self, ctx: CallContext, operation_id: str) -> MaintenanceRun | None:
        async with self.authority.read(ctx) as view:
            raw = await view.connection.scalar(
                sa.select(runs.c.record).where(
                    runs.c.namespace_id == ctx.namespace_id,
                    runs.c.record["created_by"].as_string() == ctx.caller_id,
                    runs.c.record["command"]["operation_id"].as_string() == operation_id,
                    runs.c.record["command"]["kind"].as_string() == "seed",
                )
            )
            return MaintenanceRun.model_validate(raw) if raw is not None else None

    async def list_runs(self, ctx: CallContext, *, limit: int = 50) -> tuple[MaintenanceRun, ...]:
        if not 1 <= limit <= 1000:
            raise ValueError("Maintenance list limit must be between 1 and 1000")
        async with self.authority.read(ctx) as view:
            rows = (
                await view.connection.execute(
                    sa.select(runs.c.record)
                    .where(runs.c.namespace_id == ctx.namespace_id)
                    .order_by(runs.c.commit_seq.desc())
                    .limit(limit)
                )
            ).scalars()
            return tuple(
                [
                    await self._project_in(view, ctx, MaintenanceRun.model_validate(raw))
                    for raw in rows
                ]
            )

    async def submit(self, ctx: CallContext, command: StartMaintenance) -> MaintenanceSubmission:
        async with self.authority.write(ctx) as tx:
            pass
            return await self.submit_in(tx, ctx, command)

    async def retry(
        self, ctx: CallContext, run_id: str, operation_id: str
    ) -> MaintenanceSubmission:
        async with self.authority.write(ctx) as tx:
            pass
            current = await self.run_in(tx, ctx, run_id)
            state = await self.execution.status(ctx, current.job_id)
            failed_groups = any(group.status == "failed" for group in current.groups)
            if (
                not failed_groups
                and current.status != "waiting"
                and state.status
                not in {
                    "ERROR",
                    "CANCELLED",
                    "MAX_RECOVERY_ATTEMPTS_EXCEEDED",
                }
            ):
                raise DomainError(
                    "maintenance_retry_state", "Only failed or cancelled generation can continue"
                )
            if (
                current.review is not None
                and (current.review.decision != "adopt" or not failed_groups)
                or current.purged_at is not None
            ):
                raise DomainError(
                    "maintenance_retry_unavailable", "This proposal is finalized or erased"
                )
            job = await tx.enqueue(JobSpec("nous.maintenance.generate", "default", args=(run_id,)))
            groups = current.groups
            if current.catalog is not None and current.proposal is not None:
                from .groups import compile_proposal

                snapshot = await self.snapshot_in(tx, ctx, current.catalog)
                groups = await compile_proposal(
                    self,
                    tx,
                    ctx,
                    current.proposal,
                    snapshot,
                    role=current.command.kind,
                    previous=current.groups,
                )
            updated = current.model_copy(
                update={
                    "job_id": job.workflow_id,
                    "status": "queued",
                    "waiting_reason": None,
                    "groups": tuple(groups),
                    "review": None,
                    "review_history": (
                        *current.review_history,
                        *((current.review,) if current.review else ()),
                    ),
                    "revision": current.revision + 1,
                }
            )
            await tx.connection.execute(
                runs.update()
                .where(runs.c.id == run_id)
                .values(record=updated.model_dump(mode="json"))
            )
            return MaintenanceSubmission(run_id=run_id, job_id=job.workflow_id)

    async def submit_in(
        self, tx: WriteTx, ctx: CallContext, command: StartMaintenance
    ) -> MaintenanceSubmission:
        await require_namespace_in(tx, ctx)
        identity = str(uuid4())
        job = await tx.enqueue(JobSpec("nous.maintenance.generate", "default", args=(identity,)))
        record = MaintenanceRun(
            id=identity,
            namespace_id=ctx.namespace_id,
            created_by=ctx.caller_id,
            created_at=tx.recorded_at,
            job_id=job.workflow_id,
            command=command,
        )
        await tx.connection.execute(
            runs.insert().values(
                id=identity,
                namespace_id=ctx.namespace_id,
                commit_seq=tx.commit_seq,
                context=ctx.model_dump(mode="json"),
                record=record.model_dump(mode="json"),
            )
        )
        return MaintenanceSubmission(run_id=identity, job_id=job.workflow_id)

    async def run_generate(self, run_id: str) -> JsonValue:
        async with self.authority.read() as view:
            raw = await view.connection.scalar(sa.select(runs.c.context).where(runs.c.id == run_id))
        if raw is None:
            raise DomainError("maintenance_run_not_found", "Maintenance request is unavailable")
        ctx = CallContext.model_validate(raw)
        initial = await self.read(ctx, run_id)
        prepared = (
            await tasks.prepare_scope(self, ctx, initial.command)
            if initial.catalog is None
            else initial.command
        )
        async with self.authority.write(ctx) as tx:
            record = await self.run_in(tx, ctx, run_id)
            if record.catalog is None:
                if record.command != initial.command:
                    raise OperationConflict(
                        "Maintenance inputs changed before the batch was frozen"
                    )
                snapshot = await natural.snapshot_in(self, tx, ctx, prepared)
                catalog = MaintenanceCatalog(
                    watermark=snapshot.watermark,
                    entries=tuple(
                        SnapshotReference(key=entry.key, reference=entry.record.reference)
                        for entry in snapshot.entries
                    ),
                    material_ids=snapshot.material_ids,
                    observation_ids=tuple(
                        sorted(
                            {
                                root
                                for entry in snapshot.entries
                                for root in entry.record.source_roots
                            }
                            | {
                                link.observation_id
                                for source in snapshot.sources
                                for link in source.evidence
                                if link.observation_id is not None
                            }
                        )
                    ),
                    sources=snapshot.sources,
                )
                record = record.model_copy(
                    update={"catalog": catalog, "revision": record.revision + 1}
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == run_id)
                    .values(record=record.model_dump(mode="json"))
                )
        record = await self.read(ctx, run_id)
        if not record.sources_available:
            raise DomainError(
                "maintenance_source_unavailable", "Maintenance sources are no longer available"
            )
        if record.proposal is not None or record.draft is not None:
            await self.adopt_automatic(ctx, record)
            return {"run_id": run_id}
        assert record.catalog is not None
        async with self.authority.read(ctx) as view:
            snapshot = await self.snapshot_in(view, ctx, record.catalog)
        proposal: MaintenanceProposal
        with operation_span(
            "maintenance.generate",
            ctx,
            job_id=record.job_id,
            source_ids=snapshot.material_ids,
            invocation_id=run_id,
        ):
            if snapshot.entries or snapshot.sources:
                try:
                    output, producer = await generate_validated(self, ctx, record, snapshot)
                    from .task_proposals import document

                    proposal = document(record, snapshot, output, producer)
                except (DomainError, NousError) as error:
                    if error.code not in {
                        "Unavailable",
                        "model_unavailable",
                        "model_not_configured",
                        "configuration_missing",
                        "capability_unavailable",
                    }:
                        raise
                    async with self.authority.write(ctx) as tx:
                        current = await self.run_in(tx, ctx, run_id)
                        current = current.model_copy(
                            update={
                                "waiting_reason": {"code": error.code, "message": str(error)},
                                "status": "waiting",
                                "revision": current.revision + 1,
                            }
                        )
                        await tx.connection.execute(
                            runs.update()
                            .where(runs.c.id == run_id)
                            .values(record=current.model_dump(mode="json"))
                        )
                    return {"run_id": run_id, "status": "waiting", "waiting_reason": error.code}

            else:
                proposal = MaintenanceProposal(
                    summary="No supported cognition is available.",
                    rationale="The authorized knowledge snapshot is empty.",
                    actions=(),
                )
        async with self.authority.write(ctx) as tx:
            current = await self.run_in(tx, ctx, run_id)
            if current.purged_at is not None:
                raise DomainError(
                    "maintenance_source_purged", "Maintenance sources have been purged"
                )
            if current.catalog is None:
                raise DomainError(
                    "maintenance_source_unavailable", "Maintenance source catalog is unavailable"
                )
            frozen = await self.snapshot_in(tx, ctx, current.catalog)
            groups = await proposals.compile_proposal(
                self, tx, ctx, proposal, frozen, role=current.command.kind, previous=current.groups
            )
            updated = current.model_copy(
                update={
                    "proposal": proposal,
                    "draft": proposals.compiled_draft(proposal, groups),
                    "groups": groups,
                    "status": "proposed",
                    "waiting_reason": None,
                    "questions": proposal.questions,
                    "revision": current.revision + 1,
                }
            )
            await tx.connection.execute(
                runs.update()
                .where(runs.c.id == run_id)
                .values(record=updated.model_dump(mode="json"))
            )
        await self.adopt_automatic(ctx, await self.read(ctx, run_id))
        return {"run_id": run_id}

    async def adopt_automatic(self, ctx: CallContext, record: MaintenanceRun) -> None:
        if (
            record.command.auto_adopt
            and record.review is None
            and (record.draft is not None)
            and (record.proposal is None or record.proposal.complete)
        ):
            await self.review(
                ctx,
                record.id,
                ReviewMaintenance(
                    operation_id=record.id + ":automatic:" + str(record.revision),
                    expected_revision=record.revision,
                    decision="adopt",
                    reason="Subject automatic growth policy",
                ),
            )

    async def apply_groups(
        self, ctx: CallContext, record: MaintenanceRun, selected: tuple[str, ...] | None = None
    ) -> None:
        wanted = set(selected) if selected is not None else {group.key for group in record.groups}
        known = {group.key: group for group in record.groups}
        if wanted - set(known):
            raise DomainError("maintenance_group_unknown", "Selected proposal group is unavailable")

        def include(key: str) -> None:
            for dependency in known[key].dependencies:
                if dependency not in wanted:
                    wanted.add(dependency)
                    include(dependency)

        for key in tuple(wanted):
            include(key)
        for group in record.groups:
            if group.key not in wanted or group.status != "ready" or group.draft is None:
                continue
            async with self.authority.write(ctx) as tx:
                pass
                current = await self.run_in(tx, ctx, record.id)
                if current.revision != record.revision or (
                    current.review is not None and current.review.decision == "reject"
                ):
                    raise OperationConflict("Maintenance proposal revision changed")
                group = next(value for value in current.groups if value.key == group.key)
                if any(
                    value.status != "adopted"
                    for value in current.groups
                    if value.key in group.dependencies
                ):
                    continue
                assert current.catalog is not None
                assert group.draft is not None
                jobs_before = len(tx.jobs)
                try:
                    async with tx.connection.begin_nested():
                        snapshot = await self.snapshot_in(tx, ctx, current.catalog)
                        snapshot = await self.working_snapshot_in(tx, ctx, snapshot, current.groups)
                        application = await self.growth.apply_growth_in(
                            tx,
                            ctx,
                            group.draft,
                            snapshot,
                            producer=current.proposal.producer if current.proposal else None,
                        )
                except DomainError as error:
                    del tx.jobs[jobs_before:]
                    result = group.model_copy(
                        update={
                            "status": "failed",
                            "error": {
                                "code": error.code,
                                "message": str(error),
                                "context": error.details,
                            },
                        }
                    )
                else:
                    result = group.model_copy(
                        update={
                            "status": "adopted",
                            "adopted": tuple(item.reference for item in application.records),
                            "bindings": tuple(
                                (
                                    SnapshotReference(key=key, reference=reference)
                                    for key, reference in application.bindings.items()
                                )
                            ),
                        }
                    )
                updated = current.model_copy(
                    update={
                        "groups": tuple(
                            result if value.key == group.key else value for value in current.groups
                        )
                    }
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == record.id)
                    .values(record=updated.model_dump(mode="json"))
                )

    async def review(
        self, ctx: CallContext, run_id: str, command: ReviewMaintenance
    ) -> MaintenanceRun:
        initial = await self.read(ctx, run_id)
        if initial.revision != command.expected_revision and initial.review is None:
            raise OperationConflict("Maintenance proposal revision changed")
        if (
            command.decision == "adopt"
            and initial.revision == command.expected_revision
            and initial.groups
            and (initial.review is None or initial.review.decision == "adopt")
        ):
            await self.apply_groups(ctx, initial, command.group_keys)
        async with self.authority.write(ctx) as tx:
            record = await self.run_in(tx, ctx, run_id)
            if record.revision != command.expected_revision:
                raise OperationConflict("Maintenance proposal revision changed")
            if record.purged_at is not None or record.draft is None or record.catalog is None:
                raise DomainError(
                    "maintenance_proposal_unavailable", "A retained proposal is required for review"
                )
            if record.review is not None and (
                record.review.decision == "reject" or command.decision == "reject"
            ):
                raise OperationConflict("Maintenance proposal has already been reviewed")
            adopted = ()
            if command.decision == "adopt" and record.groups:
                adopted = tuple(
                    [
                        await self.cognition.get_in(tx, ctx, reference)
                        for group in record.groups
                        for reference in group.adopted
                    ]
                )
            elif command.decision == "adopt":
                snapshot = await self.snapshot_in(tx, ctx, record.catalog)
                application = await self.growth.apply_growth_in(
                    tx,
                    ctx,
                    record.draft,
                    snapshot,
                    producer=record.proposal.producer if record.proposal else None,
                )
                adopted = application.records
            review = MaintenanceReview(
                decision=command.decision,
                reason=command.reason,
                caller_id=ctx.caller_id,
                recorded_at=tx.recorded_at,
                commit_seq=tx.commit_seq,
                adopted=tuple((item.reference for item in adopted)),
            )
            updated = record.model_copy(
                update={
                    "review": review,
                    "status": "rejected"
                    if command.decision == "reject"
                    else "adopted"
                    if not record.groups
                    or all((group.status == "adopted" for group in record.groups))
                    else "proposed",
                    "review_history": (
                        *record.review_history,
                        *((record.review,) if record.review else ()),
                    ),
                    "revision": record.revision + 1,
                }
            )
            await tx.connection.execute(
                runs.update()
                .where(runs.c.id == run_id)
                .values(record=updated.model_dump(mode="json"))
            )
        return await self.read(ctx, run_id)

    async def purge_materials_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        """Erase proposals touched by purged contacts even when the original survives."""
        if tx.ctx.namespace_id is None:
            from nous.identity.access import require_namespace_in

            namespace = await require_namespace_in(tx, tx.ctx, purpose="management")
            if tx.ctx.source_groups() or namespace.state != "trashed" or (not namespace.purging):
                raise DomainError(
                    "namespace_purge_scope_forbidden",
                    "Proposal removal requires the admitted complete Namespace",
                )
            for raw in (
                await tx.connection.execute(
                    sa.select(runs.c.record).where(runs.c.namespace_id == tx.ctx.namespace_id)
                )
            ).scalars():
                record = MaintenanceRun.model_validate(raw)
                updated = redact(record).model_copy(
                    update={"purged_at": tx.recorded_at, "revision": record.revision + 1}
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == record.id)
                    .values(record=updated.model_dump(mode="json"))
                )
            return
        purged_roots = {
            str(row["id"])
            for row in (
                await tx.connection.execute(
                    sa.select(observations.c.id, observations.c.record).where(
                        observations.c.namespace_id == tx.ctx.namespace_id
                    )
                )
            ).mappings()
            if row["record"].get("purged")
        }
        rows = (
            await tx.connection.execute(
                sa.select(runs.c.record).where(runs.c.namespace_id == tx.ctx.namespace_id)
            )
        ).scalars()
        unavailable: dict[str, bool] = {}
        for raw in rows:
            record = MaintenanceRun.model_validate(raw)
            catalog = record.catalog
            if catalog is None or record.purged_at is not None:
                continue
            missing_dependency = False
            for entry in catalog.entries:
                reference = entry.reference
                if reference.exact() not in unavailable:
                    try:
                        dependency = await self.cognition.get_in(
                            tx, tx.ctx, reference, check_visibility=False
                        )
                    except DomainError as error:
                        if error.code != "cognition_not_found":
                            raise
                        missing = True
                    else:
                        missing = dependency.purged
                        if not missing:
                            for media in media_references_in(dependency.body):
                                state = await self.cognition.derivations.availability_in(
                                    tx, tx.ctx, media
                                )
                                if state in {"purged", "missing"}:
                                    missing = True
                                    break
                    unavailable[reference.exact()] = missing
                missing_dependency |= unavailable[reference.exact()]
            if (
                missing_dependency
                or set(catalog.material_ids).intersection(material_ids)
                or set(catalog.observation_ids).intersection(purged_roots)
            ):
                self.models.purge_sources(material_ids)
                updated = redact(record).model_copy(
                    update={"purged_at": tx.recorded_at, "revision": record.revision + 1}
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == record.id)
                    .values(record=updated.model_dump(mode="json"))
                )
