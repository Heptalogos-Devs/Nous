"""Saved records and current-process maintenance have separate lifetimes."""

import asyncio
import json
import sqlite3
from pathlib import Path
from typing import NoReturn
from unittest.mock import patch

import pytest

from nous.application.contracts import EventEnvelope
from nous.cognition.maintenance.contracts import (
    MaintenanceCompleted,
    MaintenanceNotStarted,
    MaintenanceStarted,
)
from nous.cognition.retrieval.contracts import QueryRequest, RefreshRetrieval
from nous.cognition.retrieval.operations import QUERY, REFRESH
from nous.cognition.writing.contracts import AfterWrite, CreateJournal, ListRecords, ReadRecord
from nous.cognition.writing.operations import CREATE_JOURNAL, LIST_RECORDS, READ_RECORD
from nous.common.errors import NousError
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.runtime.execution import OperationContext
from nous.runtime.operations import CANCEL_OPERATION, READ_OPERATION, SelectOperation


def test_background_capacity_cancellation_and_restart_preserve_saved_records(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        background = AfterWrite(annotation="skip", refresh="run", execution="background")
        async with open_instance(instance, environment={}) as runtime:
            with pytest.raises(NousError) as unavailable:
                await runtime.app.call(
                    CREATE_JOURNAL,
                    CreateJournal(body=InlineText(text="must not save"), after=background),
                )
            assert unavailable.value.code == "execution.background_unavailable"
            assert await runtime.app.call(LIST_RECORDS, ListRecords(kind="journal")) == ()
            saved = await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(
                    body=InlineText(text="saved despite annotation failure"),
                    after=AfterWrite(annotation="run", refresh="run"),
                ),
            )
            assert isinstance(saved.maintenance, MaintenanceCompleted)
            assert saved.maintenance.annotation.state == "failed"
            assert saved.maintenance.refresh.state == "published"
            found = await runtime.app.call(QUERY, QueryRequest(text="annotation failure"))
            assert any(
                group.material.ref == saved.commit.revision_ref
                for group in found.evidence.materials
            )

        entered, release = asyncio.Event(), asyncio.Event()
        async with open_instance(
            instance,
            environment={},
            overrides={"execution.max_background_tasks": 1},
            allow_background=True,
        ) as runtime:

            async def hold(_context: OperationContext) -> None:
                entered.set()
                await release.wait()

            occupied = runtime.execution.spawn("test.capacity", hold, target_refs=())
            await asyncio.wait_for(entered.wait(), 5)
            overflow = await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(body=InlineText(text="saved while busy"), after=background),
            )
            assert isinstance(overflow.maintenance, MaintenanceNotStarted)
            assert overflow.maintenance.issue.code == "execution.background_capacity"
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=overflow.commit.revision_ref))
            ).body == "saved while busy"
            release.set()
            assert (await runtime.execution.wait(occupied)).status == "succeeded"

            started, cleaned = asyncio.Event(), asyncio.Event()

            async def interrupted(*_args: object, **_kwargs: object) -> NoReturn:
                started.set()
                try:
                    await asyncio.Event().wait()
                finally:
                    cleaned.set()
                raise AssertionError("unreachable")

            with patch.object(runtime.cognitive_retrieval, "refresh", side_effect=interrupted):
                foreground_events: list[EventEnvelope] = []
                pending = await runtime.app.call(
                    CREATE_JOURNAL,
                    CreateJournal(
                        body=InlineText(text="saved before interruption"), after=background
                    ),
                    observer=foreground_events.append,
                )
                assert isinstance(pending.maintenance, MaintenanceStarted)
                operation_ref = pending.maintenance.operation_ref
                await asyncio.wait_for(started.wait(), 5)
                cancelled = await runtime.app.call(
                    CANCEL_OPERATION, SelectOperation(ref=operation_ref)
                )
                assert cancelled.status == "cancelled" and cleaned.is_set()
                assert foreground_events
                initiating_id = foreground_events[0].operation_id
                assert all(event.operation_id == initiating_id for event in foreground_events)
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=pending.commit.revision_ref))
            ).body == "saved before interruption"
            refreshed = await runtime.app.call(REFRESH, RefreshRetrieval())
            assert refreshed.current

        log_entries = [
            json.loads(line)
            for line in (instance / "logs" / "nous.jsonl").read_text(encoding="utf-8").splitlines()
        ]
        terminal = next(
            entry
            for entry in log_entries
            if entry.get("event") == "operation.terminal"
            and entry.get("operation_id") == operation_ref.operation_id
        )
        assert terminal["initiating_operation"] == initiating_id

        async with open_instance(instance, environment={}) as runtime:
            with pytest.raises(NousError) as expired:
                await runtime.app.call(READ_OPERATION, SelectOperation(ref=operation_ref))
            assert expired.value.code == "operation.expired"
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=pending.commit.revision_ref))
            ).body == "saved before interruption"
            found = await runtime.app.call(QUERY, QueryRequest(text="saved before interruption"))
            assert any(
                group.material.ref == pending.commit.revision_ref
                for group in found.evidence.materials
            )
            database_path = (
                runtime.paths.data / runtime.configuration.snapshot.values.database.filename
            )
        with sqlite3.connect(database_path) as database:
            assert not database.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND lower(name) LIKE '%dbos%'"
            ).fetchall()

    asyncio.run(scenario())
