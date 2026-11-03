"""Committed user content survives contention, transaction failure and reopening."""

import asyncio
import sqlite3
from pathlib import Path

import pytest

from nous.common.errors import NousError
from nous.content.contracts import OutputContent, ResultInput
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_content_result_survives_independent_writer_and_reopen(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(
            instance, environment={}, overrides={"database.busy_timeout_ms": 3000}
        ) as runtime:
            original = await runtime.artifacts.ingest("原文\n\nwith exact text\n")
            database_path = (
                runtime.paths.data / runtime.configuration.snapshot.typed_values().database.filename
            )
            blocker = sqlite3.connect(database_path, timeout=0, autocommit=True)
            blocker.execute("BEGIN IMMEDIATE")
            waiting = asyncio.create_task(
                runtime.results.adopt(
                    operation="convert",
                    producer={"kind": "deterministic", "tool": "integration"},
                    inputs=(ResultInput(artifact=original.ref, key="S1"),),
                    outputs={"output": OutputContent(data="派生结果\n".encode())},
                )
            )
            try:
                with pytest.raises(TimeoutError):
                    await asyncio.wait_for(asyncio.shield(waiting), timeout=0.15)
                blocker.execute("COMMIT")
                result = await asyncio.wait_for(waiting, timeout=5)
                output = result.outputs[0].artifact
                assert (await runtime.artifacts.get(output)).producer_result_ref == result.ref
            finally:
                if blocker.in_transaction:
                    blocker.execute("ROLLBACK")
                blocker.close()
                await asyncio.gather(waiting, return_exceptions=True)
        async with open_instance(instance, environment={}) as runtime:
            saved = await runtime.results.get(result.ref)
            assert await runtime.artifacts.read_text(saved.outputs[0].artifact) == "派生结果\n"
            assert (
                await runtime.artifacts.read_text(saved.inputs[0].artifact)
                == "原文\n\nwith exact text\n"
            )

    asyncio.run(scenario())


def test_result_and_output_references_share_callers_transaction(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest("source")
            prepared = await runtime.results.prepare_outputs(
                {"output": OutputContent(data=b"derived")}
            )
            with pytest.raises(RuntimeError, match="outer owner failed"):
                async with runtime.database.write() as tx:
                    rolled_back = await runtime.results.adopt_in(
                        tx,
                        operation="convert",
                        producer={"kind": "deterministic"},
                        inputs=(ResultInput(artifact=original.ref, key="S1"),),
                        prepared_outputs=prepared,
                    )
                    raise RuntimeError("outer owner failed")
            for reference in (rolled_back.ref, rolled_back.outputs[0].artifact):
                with pytest.raises(NousError, match="does not exist"):
                    if reference.address.startswith("res:"):
                        await runtime.results.get(reference.address)
                    else:
                        await runtime.artifacts.get(reference.address)
            async with runtime.database.write() as tx:
                adopted = await runtime.results.adopt_in(
                    tx,
                    operation="convert",
                    producer={"kind": "deterministic"},
                    inputs=(ResultInput(artifact=original.ref, key="S1"),),
                    prepared_outputs=prepared,
                )
            assert await runtime.artifacts.read_text(adopted.outputs[0].artifact) == "derived"

    asyncio.run(scenario())
