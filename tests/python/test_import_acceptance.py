"""Literal source adoption and durable continuation use the real application owners."""

import asyncio
from uuid import uuid4

from nous_sdk.contracts import (
    AdmitFiles,
    FreezeCatalog,
    ImportDocument,
    ProcessCatalog,
)
from nous.platform.call_context import CallContext
from sqlalchemy import func, select

from nous.application import Application
from nous.cognition.schema import revisions
from nous.material.service import observations
from nous.platform.configuration import Settings


def test_literal_and_child_work_are_accepted_once(tmp_path):
    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(caller_id="owner", namespace_id=str(uuid4()), credential_source_ids=None)
        try:
            await app.identity.create_namespace(
                ctx, "namespace", "create", namespace_id=ctx.namespace_id
            )
            text = "完整逐字记录。" * 500
            command = AdmitFiles(
                inputs=(
                    ImportDocument(text=text, filename="whole.txt", source="urn:original-record"),
                ),
                record_units=True,
                llm_enabled=False,
                deferred_formation=True,
                vectors="disabled",
            )
            accepted = await app.imports.admit_files(ctx, command)
            literal = accepted.outputs[0]
            assert accepted.catalog.units[0].text == text
            assert literal.formation is not None
            formed = await asyncio.wait_for(
                app.execution.wait(ctx, literal.formation.job_id), timeout=15
            )
            assert formed["cognition"][0] == literal.cognition[0].model_dump(mode="json")
            repeated = await app.imports.admit_files(ctx, command)
            assert repeated.outputs[0].formation.job_id == literal.formation.job_id
            async with app.authority.read(ctx) as view:
                assert (
                    await view.connection.scalar(
                        select(func.count())
                        .select_from(revisions)
                        .where(
                            revisions.c.namespace_id == ctx.namespace_id,
                            revisions.c.kind == "memory",
                        )
                    )
                    == 1
                )
                assert (
                    await view.connection.scalar(
                        select(func.count())
                        .select_from(observations)
                        .where(observations.c.namespace_id == ctx.namespace_id)
                    )
                    == 1
                )
        finally:
            await app.close()

    asyncio.run(scenario())


def test_processing_origin_preserves_its_actual_predecessor(tmp_path):
    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(caller_id="owner", namespace_id=str(uuid4()), credential_source_ids=None)
        try:
            await app.identity.create_namespace(
                ctx, "namespace", "create", namespace_id=ctx.namespace_id
            )
            catalog = await app.imports.freeze_catalog(
                ctx, FreezeCatalog(inputs=(ImportDocument(text="Retained source text"),))
            )
            command = ProcessCatalog(
                catalog_id=catalog.id,
                origin={"delivery": "new-origin"},
                deferred_formation=True,
                llm_enabled=False,
                vectors="disabled",
            )
            result = await app.imports.process_catalog(ctx, command)
            output = result.outputs[0]
            formed = await asyncio.wait_for(
                app.execution.wait(ctx, output.formation.job_id), timeout=15
            )
            assert formed["cognition"][0] == output.cognition[0].model_dump(mode="json")
            repeated = await app.imports.process_catalog(ctx, command)
            assert repeated.outputs[0].id == output.id
            async with app.authority.read(ctx) as view:
                event = await view.connection.scalar(
                    select(observations.c.record).where(
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.id == output.observation_id,
                    )
                )
            assert event["previous_observation_id"] == catalog.units[0].observation_id
            assert event["origin"] == {"delivery": "new-origin"}
        finally:
            await app.close()

    asyncio.run(scenario())
