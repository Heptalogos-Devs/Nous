"""Literal output and deferred work are atomic, independently of later model work."""

import asyncio
from pathlib import Path

import pytest
from nous_wave.contracts import AdmitFiles, CallContext, ImportDocument
from sqlalchemy import func, select

from nous.cognition.schema import revisions
from nous.cognition.service import CognitionService
from nous.identity.service import IdentityService
from nous.imports.schema import outputs, requests, successes
from nous.imports.service import ImportService
from nous.material.service import observations
from nous.platform.execution import DurableExecution, QueueSpec
from nous.platform.persistence import Authority


class NoModels:
    async def form(self, *args, **kwargs):
        raise AssertionError("Literal-only deferred work must not call a model")

    async def embed(self, *args, **kwargs):
        raise AssertionError("Disabled embeddings must not call a model")


@pytest.mark.parametrize("registered", [True, False])
def test_literal_and_child_queue_are_atomic(tmp_path: Path, registered: bool):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        cognition = CognitionService(authority)
        imports = ImportService(authority, cognition, NoModels())
        execution = DurableExecution(
            authority, queues=(QueueSpec("default", 1), QueueSpec("formation", 1))
        )
        if registered:
            execution.register(
                "nous.imports.form_observation", imports.run_form_observation, preemptible=True
            )
        await authority.initialize()
        await execution.start()
        ctx = CallContext(
            credential_source_ids=None, caller_id="owner", namespace_id="ns", subject_id="subject"
        )
        identity = IdentityService(authority)
        try:
            await identity.create_namespace(ctx, "namespace", "namespace")
            await identity.create_subject(ctx, "subject", "subject", subject_id="subject")
            text = "完整逐字记录。" * 500
            command = AdmitFiles(
                operation_id="literal-with-work",
                inputs=(
                    ImportDocument(text=text, filename="whole.txt", source="urn:original-record"),
                ),
                record_units=True,
                llm_enabled=False,
                deferred_formation=True,
                vectors="disabled",
            )
            if not registered:
                with pytest.raises(ValueError, match="Unknown workflow"):
                    await imports.admit_files(ctx, command)
                async with authority.read(ctx) as view:
                    assert (
                        await view.connection.scalar(select(func.count()).select_from(revisions))
                        == 0
                    )
                    assert (
                        await view.connection.scalar(select(func.count()).select_from(outputs)) == 0
                    )
                    assert (
                        await view.connection.scalar(select(func.count()).select_from(successes))
                        == 0
                    )
                    assert (
                        await view.connection.scalar(
                            select(func.count())
                            .select_from(requests)
                            .where(requests.c.record["kind"].as_string() == "observation_form")
                        )
                        == 0
                    )
                return
            accepted = await imports.admit_files(ctx, command)
            assert len(accepted.catalog.units) == 1
            assert accepted.catalog.units[0].text == text
            literal = accepted.outputs[0]
            assert literal.formation is not None
            formed = await asyncio.wait_for(
                execution.wait(ctx, literal.formation.job_id), timeout=15
            )
            assert formed["cognition"][0] == literal.cognition[0].model_dump(mode="json")
            async with authority.read(ctx) as view:
                assert (
                    await view.connection.scalar(
                        select(func.count())
                        .select_from(revisions)
                        .where(revisions.c.kind == "memory")
                    )
                    == 1
                )
                assert (
                    await view.connection.scalar(select(func.count()).select_from(observations))
                    == 1
                )
            replayed = await imports.admit_files(ctx, command)
            assert replayed.outputs[0].formation.job_id == literal.formation.job_id
        finally:
            await execution.close()
            await authority.close()

    asyncio.run(scenario())
