"""SQLite connections honor the configured wait when another writer holds the lock."""

import asyncio
import sqlite3
from pathlib import Path

import pytest
import sqlalchemy as sa

from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.storage.database import time_domain_definitions


def test_database_waits_for_an_independent_writer(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})

        async with open_instance(instance, environment={}) as runtime:
            async with runtime.database.read() as tx:
                assert await tx.connection.scalar(sa.text("PRAGMA busy_timeout")) == 5000

        async with open_instance(
            instance, environment={}, overrides={"database.busy_timeout_ms": 3000}
        ) as runtime:
            async with runtime.database.read() as first, runtime.database.read() as second:
                assert await first.connection.scalar(sa.text("PRAGMA busy_timeout")) == 3000
                assert await second.connection.scalar(sa.text("PRAGMA busy_timeout")) == 3000

            database_path = (
                runtime.paths.data / runtime.configuration.snapshot.values.database.filename
            )
            blocker = sqlite3.connect(database_path, timeout=0, autocommit=True)
            blocker.execute("BEGIN IMMEDIATE")
            started = asyncio.Event()

            async def write_domain() -> None:
                async with runtime.database.write() as tx:
                    started.set()
                    await tx.connection.execute(
                        sa.insert(time_domain_definitions).values(
                            domain_id="independent-writer", kind="opaque", name="Independent writer"
                        )
                    )

            waiting = asyncio.create_task(write_domain())
            try:
                await asyncio.wait_for(started.wait(), timeout=5)
                with pytest.raises(TimeoutError):
                    await asyncio.wait_for(asyncio.shield(waiting), timeout=0.15)
                blocker.execute("COMMIT")
                await asyncio.wait_for(waiting, timeout=5)
                assert "independent-writer" in await runtime.time.domains()
            finally:
                if blocker.in_transaction:
                    blocker.execute("ROLLBACK")
                blocker.close()
                await asyncio.gather(waiting, return_exceptions=True)

    asyncio.run(scenario())
