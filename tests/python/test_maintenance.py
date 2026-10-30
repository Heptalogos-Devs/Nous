"""Growth business facts use DBOS execution, explicit review and source retirement."""

import asyncio

import pytest
import sqlalchemy as sa
from dbos import PortableWorkflowError
from nous_sdk.contracts import AdmitFiles, CallContext, DomainError, ImportDocument, SourceCommand
from nous_sdk.contracts.maintenance import (
    CreateMaintenanceSchedule,
    ReviewMaintenance,
    StartMaintenance,
)

from nous.bootstrap import application
from nous.material.service import materials
from nous.platform.configuration import Settings
from nous.platform.errors import Unavailable
from nous.platform.persistence import OperationConflict


def test_empty_snapshot_review_and_real_dbos_schedule_reconciliation(tmp_path):
    async def scenario():
        async with application(Settings(instance=tmp_path / "instance")) as app:
            ctx = CallContext(
                credential_source_ids=None,
                caller_id="owner",
                namespace_id="namespace",
                subject_id="subject",
            )
            await app.identity.create_namespace(ctx, "Knowledge", "namespace")
            await app.identity.create_subject(ctx, "Subject", "subject", subject_id="subject")
            submitted = await app.maintenance.submit(ctx, StartMaintenance(operation_id="request"))
            assert await app.execution.wait(ctx, submitted.job_id) == {"run_id": submitted.run_id}
            run = await app.maintenance.read(ctx, submitted.run_id)
            assert run.draft.actions == ()
            assert run.producer_call_id is None and run.review is None
            assert await app.models.ledger.list(ctx) == []
            assert (
                await app.maintenance.submit(ctx, StartMaintenance(operation_id="request"))
                == submitted
            )
            decision = ReviewMaintenance(
                operation_id="review",
                expected_revision=run.revision,
                decision="reject",
                reason="No supported changes",
            )
            reviewed = await app.maintenance.review(ctx, run.id, decision)
            assert reviewed.review.decision == "reject"
            assert await app.maintenance.review(ctx, run.id, decision) == reviewed
            with pytest.raises(OperationConflict):
                await app.maintenance.review(
                    ctx, run.id, decision.model_copy(update={"operation_id": "different"})
                )
            assert (await app.maintenance.list_runs(ctx))[0] == reviewed
            with pytest.raises(DomainError, match="unavailable"):
                await app.maintenance.read(ctx.model_copy(update={"subject_id": "other"}), run.id)
            schedule = await app.maintenance.create_schedule(
                ctx,
                CreateMaintenanceSchedule(
                    operation_id="schedule",
                    name="Annual",
                    cron="0 0 1 1 *",
                ),
            )
            dbos = app.execution.client
            await dbos.delete_schedule_async(schedule.id)
            assert await dbos.get_schedule_async(schedule.id) is None
            await app.maintenance.reconcile_schedules()
            assert await dbos.get_schedule_async(schedule.id) is not None
            # Public DBOS triggering exercises its portable serialization, scheduler
            # workflow and atomic owned-job admission; no local polling scheduler.
            tick = await asyncio.to_thread(dbos.trigger_schedule, schedule.id)
            handle = await dbos.retrieve_workflow_async(tick.get_workflow_id())
            result = await asyncio.wait_for(
                handle.get_result(polling_interval_sec=0.05), timeout=10
            )
            assert result["run_id"] != submitted.run_id
            await app.execution.wait(ctx, result["job_id"])
            assert len(await app.maintenance.list_runs(ctx)) == 2
            stopped = await app.maintenance.stop_schedule(ctx, schedule.id, "stop")
            assert stopped.stopped_at
            assert await dbos.get_schedule_async(schedule.id) is None
            await app.maintenance.reconcile_schedules()
            assert await dbos.get_schedule_async(schedule.id) is None
            assert await app.models.ledger.list(ctx) == []

    asyncio.run(scenario())


def test_contact_purge_clears_run_payload_while_shared_original_survives(tmp_path):
    async def scenario():
        async with application(Settings(instance=tmp_path / "instance")) as app:
            ctx = CallContext(
                credential_source_ids=None,
                caller_id="owner",
                namespace_id="namespace",
                subject_id="subject",
            )
            await app.identity.create_namespace(ctx, "Knowledge", "namespace")
            await app.identity.create_subject(ctx, "Subject", "subject", subject_id="subject")
            source = "Sensitive project evidence for one shared immutable original."
            imported = await app.imports.admit_files(
                ctx,
                AdmitFiles(
                    operation_id="literal",
                    inputs=(
                        ImportDocument(text=source, filename="one.txt"),
                        ImportDocument(text=source, filename="two.txt"),
                    ),
                    llm_enabled=False,
                    vectors="disabled",
                    dedupe_policy="none",
                    force=True,
                ),
            )
            assert len(imported.catalog.input_source_ids) == 2
            material_id = imported.catalog.units[0].material_id
            assert imported.catalog.units[1].material_id == material_id
            submitted = await app.maintenance.submit(
                ctx, StartMaintenance(instruction=source, operation_id="unsupported-provider")
            )
            # No provider is configured. Its real failed job still froze exact source
            # identities, so scope projection and contact-only purge are exercised.
            with pytest.raises(
                (Unavailable, PortableWorkflowError), match="Model ID has not been configured"
            ):
                await asyncio.wait_for(app.execution.wait(ctx, submitted.job_id), timeout=10)
            run = await app.maintenance.read(ctx, submitted.run_id)
            assert run.catalog and len(run.catalog.observation_ids) == 2
            denied = await app.maintenance.read(
                ctx.model_copy(update={"allowed_source_ids": ()}), run.id
            )
            assert not denied.sources_available and denied.catalog is None
            assert source not in denied.model_dump_json()
            await app.sources.purge(
                ctx,
                SourceCommand(
                    operation_id="purge-one-contact",
                    reason="Erase one retired contact while retaining independent support",
                    observation_ids=(imported.outputs[0].observation_id,),
                ),
            )
            async with app.authority.read(ctx) as view:
                original = await view.connection.scalar(
                    sa.select(materials.c.record).where(materials.c.id == material_id)
                )
            assert not original.get("purged", False)
            purged = await app.maintenance.read(ctx, run.id)
            assert purged.purged_at and not purged.sources_available
            assert source not in purged.model_dump_json()
            assert purged.command.instruction == ""

    asyncio.run(scenario())
