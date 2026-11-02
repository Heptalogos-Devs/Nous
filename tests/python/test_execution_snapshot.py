import asyncio
from pathlib import Path
from uuid import uuid4

from nous_sdk.contracts import CallContext

from nous.application import Application
from nous.platform.configuration import Settings
from nous.platform.execution import JobSpec


def test_namespace_restore_retains_results_and_readmits_pending_work(tmp_path: Path):
    async def scenario():
        calls: list[int] = []

        async def calculate(value: int) -> dict[str, int]:
            calls.append(value)
            return {"value": value * 2}

        app = Application(Settings(instance=tmp_path / "instance"))
        app.execution.register("test.snapshot.calculate", calculate)
        await app.start()
        try:
            source = CallContext(
                caller_id="owner", namespace_id=str(uuid4()), credential_source_ids=None
            )
            target = source.model_copy(update={"namespace_id": str(uuid4())})
            for context, slug in ((source, "source"), (target, "target")):
                await app.identity.create_namespace(
                    context, slug, "create:" + slug, namespace_id=context.namespace_id
                )
            async with app.authority.write(source) as tx:
                done = await tx.enqueue(
                    JobSpec("test.snapshot.calculate", "default", args=(7,))
                )
            assert await app.execution.wait(source, done.workflow_id) == {"value": 14}
            async with app.authority.write(source) as tx:
                pending = await tx.enqueue(
                    JobSpec("test.snapshot.calculate", "default", args=(8,), delay_seconds=3600)
                )
            async with app.authority.read(source) as view:
                snapshot = await app.execution.export_namespace_in(view, source)
            assert {job.workflow_id for job in snapshot.jobs} == {
                done.workflow_id,
                pending.workflow_id,
            }
            mapping = {job.workflow_id: str(uuid4()) for job in snapshot.jobs}
            async with app.authority.write(target) as tx:
                receiving = snapshot.model_copy(
                    update={
                        "namespace_id": target.namespace_id,
                        "jobs": tuple(
                            job.model_copy(
                                update={
                                    "workflow_id": mapping[job.workflow_id],
                                    "commit_seq": tx.commit_seq,
                                    "delay_until_epoch_ms": None,
                                }
                            )
                            for job in snapshot.jobs
                        ),
                    }
                )
                resumed = await app.execution.restore_namespace_in(tx, target, receiving)
            assert resumed == (mapping[pending.workflow_id],)
            assert await app.execution.wait(target, mapping[done.workflow_id]) == {"value": 14}
            assert await app.execution.wait(target, mapping[pending.workflow_id]) == {"value": 16}
            assert calls == [7, 8]
            listed = await app.execution.list_jobs(target)
            assert {job.workflow_id: job.status for job in listed} == {
                mapping[done.workflow_id]: "SUCCESS",
                mapping[pending.workflow_id]: "SUCCESS",
            }
            # The completed command remains a Nous result, without a copied DBOS workflow.
            assert not await app.execution.client.list_workflows_async(
                workflow_ids=[mapping[done.workflow_id]]
            )
        finally:
            await app.close()

    asyncio.run(scenario())
