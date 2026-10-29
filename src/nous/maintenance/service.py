"""Durable proposal generation and explicit, atomic review of cognitive growth."""

from uuid import uuid4

import sqlalchemy as sa
from nous_wave.contracts import CallContext, DomainError
from nous_wave.contracts.growth import GrowthDraft, GrowthEntry, GrowthSnapshot
from nous_wave.contracts.maintenance import (
    MaintenanceCatalog,
    MaintenanceReview,
    MaintenanceRun,
    MaintenanceSubmission,
    ReviewMaintenance,
    SnapshotReference,
    StartMaintenance,
)
from pydantic import JsonValue, TypeAdapter

from nous.cognition.projection import media_references_in
from nous.cognition.service import CognitionService
from nous.identity.service import require_subject_in
from nous.material.service import observations
from nous.platform.execution import DurableExecution, JobSpec
from nous.platform.models import ModelService
from nous.platform.observability import operation_span
from nous.platform.persistence import Authority, OperationConflict, ReadView, WriteTx

from . import scheduling
from .generation import generate_validated
from .schema import runs


class MaintenanceService:
    create_schedule = scheduling.create_schedule
    list_schedules = scheduling.list_schedules
    stop_schedule = scheduling.stop_schedule
    reconcile_schedules = scheduling.reconcile_schedules
    run_tick = scheduling.run_tick

    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        models: ModelService,
        execution: DurableExecution,
    ) -> None:
        self.authority = authority
        self.cognition = cognition
        self.models = models
        self.execution = execution

    async def _run_in(
        self, view: ReadView | WriteTx, ctx: CallContext, run_id: str
    ) -> MaintenanceRun:
        raw = await view.connection.scalar(
            sa.select(runs.c.record).where(
                runs.c.id == run_id,
                runs.c.namespace_id == ctx.namespace_id,
                runs.c.subject_id == ctx.require_subject(),
            )
        )
        if raw is None:
            raise DomainError(
                "maintenance_run_not_found", "Maintenance run is unavailable in this subject"
            )
        return MaintenanceRun.model_validate(raw)

    async def _snapshot_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        catalog: MaintenanceCatalog,
    ) -> GrowthSnapshot:
        subject = await require_subject_in(view, ctx)
        entries = tuple(
            [
                GrowthEntry(
                    key=entry.key, record=await self.cognition.get_in(view, ctx, entry.reference)
                )
                for entry in catalog.entries
            ]
        )
        return GrowthSnapshot(
            watermark=catalog.watermark,
            entries=entries,
            material_ids=catalog.material_ids,
            capabilities=TypeAdapter(tuple[str, ...]).validate_python(subject["capabilities"]),
        )

    async def _project_in(
        self, view: ReadView | WriteTx, ctx: CallContext, record: MaintenanceRun
    ) -> MaintenanceRun:
        available = record.purged_at is None and not (
            record.catalog is None and ctx.source_groups()
        )
        if available and record.catalog is not None:
            try:
                await self._snapshot_in(view, ctx, record.catalog)
            except DomainError as error:
                if error.code not in {"source_unavailable", "cognition_not_found"}:
                    raise
                available = False
        if available:
            return record
        return record.model_copy(
            update={
                "sources_available": False,
                "draft": None,
                "catalog": None,
                "command": record.command.model_copy(update={"instruction": ""}),
                "review": None
                if record.review is None
                else record.review.model_copy(update={"reason": "", "adopted": ()}),
            }
        )

    async def read(self, ctx: CallContext, run_id: str) -> MaintenanceRun:
        async with self.authority.read(ctx) as view:
            return await self._project_in(view, ctx, await self._run_in(view, ctx, run_id))

    async def list_runs(self, ctx: CallContext, *, limit: int = 50) -> tuple[MaintenanceRun, ...]:
        if not 1 <= limit <= 1000:
            raise ValueError("Maintenance list limit must be between 1 and 1000")
        async with self.authority.read(ctx) as view:
            rows = (
                await view.connection.execute(
                    sa.select(runs.c.record)
                    .where(
                        runs.c.namespace_id == ctx.namespace_id,
                        runs.c.subject_id == ctx.require_subject(),
                    )
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
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="maintenance.submit",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return MaintenanceSubmission.model_validate(tx.replay_result)
            return tx.result(await self.submit_in(tx, ctx, command))

    async def submit_in(
        self, tx: WriteTx, ctx: CallContext, command: StartMaintenance
    ) -> MaintenanceSubmission:
        await require_subject_in(tx, ctx)
        identity = str(uuid4())
        job = await tx.enqueue(JobSpec("nous.maintenance.generate", "default", args=(identity,)))
        record = MaintenanceRun(
            id=identity,
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            created_by=ctx.caller_id,
            created_at=tx.recorded_at,
            job_id=job.workflow_id,
            command=command,
        )
        await tx.connection.execute(
            runs.insert().values(
                id=identity,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.subject_id,
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
        async with self.authority.write(
            ctx,
            run_id + ":catalog",
            operation_kind="maintenance.snapshot",
            scope=ctx.require_subject(),
            payload={"run_id": run_id},
        ) as tx:
            if not tx.replayed:
                record = await self._run_in(tx, ctx, run_id)
                snapshot = await self.cognition.growth_snapshot_in(
                    tx, ctx, record.command.kinds, record.command.limit
                )
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
                        )
                    ),
                )
                record = record.model_copy(
                    update={"catalog": catalog, "revision": record.revision + 1}
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == run_id)
                    .values(record=record.model_dump(mode="json"))
                )
                tx.result({"run_id": run_id})
        record = await self.read(ctx, run_id)
        if not record.sources_available:
            raise DomainError(
                "maintenance_source_unavailable", "Maintenance sources are no longer available"
            )
        if record.draft is not None:
            return {"run_id": run_id}
        assert record.catalog is not None
        async with self.authority.read(ctx) as view:
            snapshot = await self._snapshot_in(view, ctx, record.catalog)
        with operation_span(
            "maintenance.generate",
            ctx,
            job_id=record.job_id,
            source_ids=snapshot.material_ids,
            invocation_id=run_id,
        ):
            if snapshot.entries:
                draft, call_id = await generate_validated(self, ctx, record, snapshot)
            else:
                draft = GrowthDraft(
                    summary="No supported cognition is available.",
                    rationale="The authorized knowledge snapshot is empty.",
                    actions=(),
                )
                call_id = None
        async with self.authority.write(
            ctx,
            run_id + ":proposal",
            operation_kind="maintenance.propose",
            scope=ctx.require_subject(),
            payload={"run_id": run_id},
        ) as tx:
            if not tx.replayed:
                current = await self._run_in(tx, ctx, run_id)
                if current.purged_at is not None:
                    raise DomainError(
                        "maintenance_source_purged", "Maintenance sources have been purged"
                    )
                updated = current.model_copy(
                    update={
                        "draft": draft,
                        "producer_call_id": call_id,
                        "revision": current.revision + 1,
                    }
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == run_id)
                    .values(record=updated.model_dump(mode="json"))
                )
                tx.result({"run_id": run_id})
        # DBOS retains references only; source-derived text belongs to this owner.
        return {"run_id": run_id}

    async def record_generation_call(self, ctx: CallContext, run_id: str, call_id: str) -> None:
        """Retain actual provider-call identities without copying their paid output."""
        async with self.authority.write(
            ctx,
            run_id + ":call:" + call_id,
            operation_kind="maintenance.generation_call",
            scope=ctx.require_subject(),
            payload={"run_id": run_id, "call_id": call_id},
        ) as tx:
            if tx.replayed:
                return
            current = await self._run_in(tx, ctx, run_id)
            if current.purged_at is not None:
                raise DomainError(
                    "maintenance_source_purged", "Maintenance sources have been purged"
                )
            if call_id not in current.generation_call_ids:
                updated = current.model_copy(
                    update={
                        "generation_call_ids": (*current.generation_call_ids, call_id),
                        "revision": current.revision + 1,
                    }
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == run_id)
                    .values(record=updated.model_dump(mode="json"))
                )
            tx.result({"run_id": run_id, "call_id": call_id})

    async def review(
        self, ctx: CallContext, run_id: str, command: ReviewMaintenance
    ) -> MaintenanceRun:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="maintenance.review",
            scope=ctx.require_subject(),
            payload={"run_id": run_id, "command": command},
        ) as tx:
            if not tx.replayed:
                record = await self._run_in(tx, ctx, run_id)
                if record.revision != command.expected_revision:
                    raise OperationConflict("Maintenance proposal revision changed")
                if record.purged_at is not None or record.draft is None or record.catalog is None:
                    raise DomainError(
                        "maintenance_proposal_unavailable",
                        "A retained proposal is required for review",
                    )
                if record.review is not None:
                    raise OperationConflict("Maintenance proposal has already been reviewed")
                snapshot = await self._snapshot_in(tx, ctx, record.catalog)
                adopted = ()
                if command.decision == "adopt":
                    adopted = await self.cognition.apply_growth_in(
                        tx, ctx, record.draft, snapshot, producer_call_id=record.producer_call_id
                    )
                review = MaintenanceReview(
                    decision=command.decision,
                    reason=command.reason,
                    caller_id=ctx.caller_id,
                    recorded_at=tx.recorded_at,
                    commit_seq=tx.commit_seq,
                    adopted=tuple(item.reference for item in adopted),
                )
                updated = record.model_copy(
                    update={"review": review, "revision": record.revision + 1}
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == run_id)
                    .values(record=updated.model_dump(mode="json"))
                )
                tx.result({"run_id": run_id})
        return await self.read(ctx, run_id)

    async def purge_materials_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        """Erase proposals touched by purged contacts even when the original survives."""
        purged_roots = {
            str(row["id"])
            for row in (
                await tx.connection.execute(
                    sa.select(observations.c.id, observations.c.record).where(
                        observations.c.namespace_id == tx.ctx.namespace_id,
                        observations.c.subject_id == tx.ctx.require_subject(),
                    )
                )
            ).mappings()
            if row["record"].get("purged")
        }
        rows = (
            await tx.connection.execute(
                sa.select(runs.c.record).where(
                    runs.c.namespace_id == tx.ctx.namespace_id,
                    runs.c.subject_id == tx.ctx.require_subject(),
                )
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
                call_ids = set(record.generation_call_ids)
                if record.producer_call_id is not None:
                    call_ids.add(record.producer_call_id)
                await self.models.ledger.purge_calls_in(tx, tuple(call_ids))
                creator = tx.ctx.model_copy(update={"caller_id": record.created_by})
                await self.models.ledger.purge_invocations_in(
                    tx,
                    creator,
                    "maintenance",
                    (
                        "maintenance:" + record.id,
                        "maintenance:" + record.id + ":correction:1",
                    ),
                )
                updated = record.model_copy(
                    update={
                        "draft": None,
                        "purged_at": tx.recorded_at,
                        "command": record.command.model_copy(update={"instruction": ""}),
                        "review": None
                        if record.review is None
                        else record.review.model_copy(update={"reason": ""}),
                        "revision": record.revision + 1,
                    }
                )
                await tx.connection.execute(
                    runs.update()
                    .where(runs.c.id == record.id)
                    .values(record=updated.model_dump(mode="json"))
                )
