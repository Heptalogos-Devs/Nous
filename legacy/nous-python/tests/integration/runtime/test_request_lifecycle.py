"""The local runtime's user-visible path and its persistent contracts."""

import asyncio
import threading
from pathlib import Path
from unittest.mock import patch

import pytest
from pydantic import ValidationError

from nous.application.contracts import EventDeliveryError, EventEnvelope, Progress, Saved
from nous.cognition.writing.contracts import (
    AddNote,
    AfterWrite,
    ListRecords,
    ReadRecord,
)
from nous.cognition.writing.operations import (
    ADD_NOTE,
    LIST_RECORDS,
    READ_RECORD,
)
from nous.common.errors import NousError
from nous.common.operations import exception_context
from nous.content.contracts import (
    InlineText,
)
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_application_validation_and_observer_failure_preserve_saved_record(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            with pytest.raises(NousError) as unknown:
                await runtime.app.call_name("unknown.operation", {})
            assert unknown.value.code == "operation.unknown"
            with pytest.raises(ValidationError):
                await runtime.app.call_name("records.read", {"target": "x", "extra": True})

            def observer(event: EventEnvelope) -> None:
                if isinstance(event.event, Saved):
                    raise RuntimeError("observer disconnected")

            with pytest.raises(EventDeliveryError, match="observer failed") as failed:
                await runtime.app.call(
                    ADD_NOTE,
                    AddNote(
                        body=InlineText(text="committed before observer failure"), title="saved"
                    ),
                    observer=observer,
                )
            entries = await runtime.app.call(LIST_RECORDS, ListRecords(kind="note"))
            saved = next(item for item in entries if item.title == "saved")
            saved_refs = exception_context(failed.value)["completed_refs"]
            assert isinstance(saved_refs, list)
            assert saved.revision_ref.model_dump(mode="json") in saved_refs
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=saved.revision_ref))
            ).body == "committed before observer failure"

            async def annotation(_request: object, call: object) -> None:
                from nous.application.contracts import CallContext

                assert isinstance(call, CallContext)
                await call.events.emit(Progress(stage="annotation"))

            def failed_delivery(event: EventEnvelope) -> None:
                if isinstance(event.event, Progress):
                    raise NousError("client.disconnected", "observer disconnected")

            with patch.object(runtime.annotations, "annotate", side_effect=annotation):
                with pytest.raises(EventDeliveryError) as failed_during_maintenance:
                    await runtime.app.call(
                        ADD_NOTE,
                        AddNote(
                            body=InlineText(text="saved before annotation delivery failure"),
                            title="maintained",
                            after=AfterWrite(annotation="run"),
                        ),
                        observer=failed_delivery,
                    )
            entries = await runtime.app.call(LIST_RECORDS, ListRecords(kind="note"))
            maintained = next(item for item in entries if item.title == "maintained")
            completed = exception_context(failed_during_maintenance.value)["completed_refs"]
            assert isinstance(completed, list)
            assert maintained.revision_ref.model_dump(mode="json") in completed

    asyncio.run(scenario())


def test_shutdown_waits_for_cancelled_blocking_worker(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance)
        ready = asyncio.get_running_loop().create_future()
        stop = asyncio.Event()
        started = threading.Event()
        release = threading.Event()

        async def host() -> None:
            async with open_instance(instance) as runtime:
                ready.set_result(runtime)
                await stop.wait()

        def work() -> None:
            started.set()
            if not release.wait(5):
                raise TimeoutError("worker was not released")

        host_task = asyncio.create_task(host())
        runtime = await ready
        waiter = asyncio.create_task(runtime.execution.blocking(work))
        try:
            assert await asyncio.to_thread(started.wait, 2)
            waiter.cancel()
            stop.set()
            await asyncio.sleep(0.05)
            assert not waiter.done()
            assert not host_task.done()
            assert (await runtime.database.check()).foreign_keys
        finally:
            release.set()
            stop.set()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(waiter, 5)
            await asyncio.wait_for(host_task, 5)
        assert runtime.state == "stopped"

    asyncio.run(scenario())
