"""Incremental retrieval uses exact revisions and protects in-flight work."""

import asyncio
import threading
from pathlib import Path
from unittest.mock import patch

import pytest
import sqlalchemy as sa

from nous.cognition.contracts import CreateNamespace, RenameNamespace
from nous.cognition.retrieval.contracts import CognitiveQuery, RefreshRetrieval
from nous.cognition.retrieval.operations import QUERY, REFRESH
from nous.cognition.scope import CREATE_NAMESPACE, RENAME_NAMESPACE
from nous.cognition.writing.contracts import (
    AddNote,
    AfterWrite,
    ReadRecord,
    RecordMembership,
    ReplaceRecord,
    SetRecall,
)
from nous.cognition.writing.operations import (
    ADD_NOTE,
    ARCHIVE_RECORD,
    READ_RECORD,
    REPLACE_RECORD,
    SET_RECALL,
)
from nous.common.errors import NousError
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema


def test_one_revision_reuses_unchanged_bodies_and_old_snapshot(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "incremental"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            long_body = (
                "A long retained section with sharedterm.\n\n" * 600
            ) + "Final endingquartz."
            first = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="First sharedterm."),
                    title="First",
                    remember=True,
                    after=policy,
                ),
            )
            long = await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text=long_body), title="Long", remember=True, after=policy),
            )
            archived = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="Third sharedterm."),
                    title="Third",
                    remember=True,
                    after=policy,
                ),
            )
            original = await runtime.app.call(
                QUERY, CognitiveQuery(text="sharedterm", mode="lexical", top_k=5)
            )
            old_view = await runtime.app.call(
                READ_RECORD, ReadRecord(target=first.commit.revision_ref)
            )
            renamed = await runtime.app.call(
                REPLACE_RECORD,
                ReplaceRecord(
                    target=first.commit.ref,
                    expected=first.commit.revision_ref,
                    body=InlineText(text="First sharedterm."),
                    title="uniqueheading",
                    after=policy,
                ),
            )
            new_view = await runtime.app.call(
                READ_RECORD, ReadRecord(target=renamed.commit.revision_ref)
            )
            assert new_view.body_artifact == old_view.body_artifact
            title = await runtime.app.call(
                QUERY, CognitiveQuery(text="uniqueheading", mode="lexical")
            )
            assert title.groups[0].context.revision_ref == renamed.commit.revision_ref
            assert title.groups[0].passages[0].delivery.reason == "title_preview"
            async with runtime.database.read() as tx:
                summary = await tx.connection.scalar(
                    sa.select(schema.snapshots.c.build_summary).where(
                        schema.snapshots.c.id == title.context.snapshot_ref.id
                    )
                )
            assert summary is not None
            preparation = summary["statistics"]["preparation"]
            assert preparation["bodies"] == preparation["bytes_read"] == preparation["parsed"] == 0
            assert preparation["reused_projection_units"] == 2

            await runtime.app.call(
                ARCHIVE_RECORD,
                RecordMembership(
                    target=archived.commit.ref,
                    expected=archived.commit.revision_ref,
                    after=policy,
                ),
            )
            after_archive = await runtime.app.call(
                QUERY, CognitiveQuery(text="sharedterm", mode="lexical", top_k=5)
            )
            assert {group.context.revision_ref.revision_id for group in after_archive.groups} == {
                renamed.commit.revision_ref.revision_id,
                long.commit.revision_ref.revision_id,
            }
            async with runtime.database.read() as tx:
                archived_summary = await tx.connection.scalar(
                    sa.select(schema.snapshots.c.build_summary).where(
                        schema.snapshots.c.id == after_archive.context.snapshot_ref.id
                    )
                )
            assert archived_summary is not None
            archive_work = archived_summary["statistics"]["preparation"]
            assert archive_work["bytes_read"] == archive_work["parsed"] == 0

            changed = await runtime.app.call(
                REPLACE_RECORD,
                ReplaceRecord(
                    target=long.commit.ref,
                    expected=long.commit.revision_ref,
                    body=InlineText(text="Replacement sharedterm."),
                    after=policy,
                ),
            )
            fresh = await runtime.app.call(
                QUERY, CognitiveQuery(text="sharedterm", mode="lexical", top_k=5)
            )
            revisions = {group.context.revision_ref.revision_id for group in fresh.groups}
            assert revisions == {
                renamed.commit.revision_ref.revision_id,
                changed.commit.revision_ref.revision_id,
            }
            await runtime.app.call(
                SET_RECALL,
                SetRecall(
                    target=changed.commit.ref,
                    expected=changed.commit.revision_ref,
                    enabled=False,
                    after=policy,
                ),
            )
            after_recall_change = await runtime.app.call(
                QUERY, CognitiveQuery(text="sharedterm", mode="lexical", top_k=5)
            )
            assert [group.context.revision_ref for group in after_recall_change.groups] == [
                renamed.commit.revision_ref
            ]
            async with runtime.database.read() as tx:
                recall_summary = await tx.connection.scalar(
                    sa.select(schema.snapshots.c.build_summary).where(
                        schema.snapshots.c.id == after_recall_change.context.snapshot_ref.id
                    )
                )
            assert recall_summary is not None
            recall_work = recall_summary["statistics"]["preparation"]
            assert recall_work["bodies"] == recall_work["bytes_read"] == recall_work["parsed"] == 0
            historical = await runtime.app.call(
                QUERY,
                CognitiveQuery(
                    text="sharedterm",
                    mode="lexical",
                    snapshot=original.context.snapshot_ref,
                    top_k=5,
                ),
            )
            assert not historical.current
            assert {group.context.revision_ref.revision_id for group in historical.groups} == {
                first.commit.revision_ref.revision_id,
                long.commit.revision_ref.revision_id,
                archived.commit.revision_ref.revision_id,
            }
            old_tail = await runtime.app.call(
                QUERY,
                CognitiveQuery(
                    text="endingquartz",
                    mode="lexical",
                    snapshot=original.context.snapshot_ref,
                ),
            )
            assert any(
                "Final endingquartz" in passage.delivery.text
                for group in old_tail.groups
                if group.context.revision_ref == long.commit.revision_ref
                for passage in group.passages
            )
            async with runtime.database.read() as tx:
                summary = await tx.connection.scalar(
                    sa.select(schema.snapshots.c.build_summary).where(
                        schema.snapshots.c.id == fresh.context.snapshot_ref.id
                    )
                )
            assert summary is not None
            assert summary["statistics"]["timings"]["lexical_delta"]["incremental"] is True

    asyncio.run(scenario())


def test_cancelled_native_build_waits_for_worker_and_keeps_base(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "cancel"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text="stable record"), remember=True, after=policy),
            )
            baseline = await runtime.app.call(REFRESH, RefreshRetrieval(mode="lexical"))
            await runtime.app.call(
                ADD_NOTE, AddNote(body=InlineText(text="new record"), remember=True, after=policy)
            )
            snapshots = runtime.wave.snapshots
            engine = snapshots.engine()
            entered, release = threading.Event(), threading.Event()

            class PausedEngine:
                def build(self, *args: object, **kwargs: object) -> object:
                    entered.set()
                    if not release.wait(10):
                        raise TimeoutError("native worker was not released")
                    return engine.build(*args, **kwargs)  # type: ignore[arg-type]

                def open(self, *args: object, **kwargs: object) -> object:
                    return engine.open(*args, **kwargs)  # type: ignore[arg-type]

                def close(self) -> None:
                    engine.close()

            with patch.object(snapshots, "_engine", PausedEngine()):
                task = asyncio.create_task(
                    runtime.app.call(REFRESH, RefreshRetrieval(mode="lexical"))
                )
                try:
                    assert await asyncio.to_thread(entered.wait, 10)
                    task.cancel()
                    with pytest.raises(TimeoutError):
                        await asyncio.wait_for(asyncio.shield(task), 0.1)
                    await runtime.wave.cleanup()
                    assert (await runtime.wave.describe(baseline.snapshot)).ref == baseline.snapshot
                finally:
                    release.set()
                with pytest.raises(asyncio.CancelledError):
                    await task
            current = await runtime.app.call(REFRESH, RefreshRetrieval(mode="lexical"))
            assert current.current and current.captured_generation == 2
            assert baseline.snapshot.identifier in (await runtime.wave.cleanup()).snapshots

    asyncio.run(scenario())


def test_snapshot_owner_and_namespace_references_survive_rename(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "scope"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            namespace = await runtime.app.call(CREATE_NAMESPACE, CreateNamespace(name="research"))
            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    namespace=namespace.ref,
                    body=InlineText(text="scopequartz is in research"),
                    remember=True,
                    after=AfterWrite(annotation="skip"),
                ),
            )
            snapshot = await runtime.app.call(
                REFRESH, RefreshRetrieval(namespace=namespace.ref, mode="lexical")
            )
            await runtime.app.call(
                RENAME_NAMESPACE, RenameNamespace(namespace=namespace.ref, name="renamed")
            )
            for selector in (snapshot.snapshot, snapshot.snapshot.identifier):
                result = await runtime.app.call(
                    QUERY, CognitiveQuery(snapshot=selector, text="scopequartz", mode="lexical")
                )
                assert result.namespace.ref == namespace.ref
                assert result.namespace.name == "renamed"
                assert result.groups
            with pytest.raises(NousError) as error:
                await runtime.app.call(
                    QUERY,
                    CognitiveQuery(
                        snapshot=snapshot.snapshot,
                        namespace="default",
                        text="scopequartz",
                        mode="lexical",
                    ),
                )
            assert error.value.code == "cognition.namespace_mismatch"
            foreign = namespace.ref.model_copy(update={"instance_id": "other-instance"})
            with pytest.raises(NousError) as error:
                await runtime.app.call(REFRESH, RefreshRetrieval(namespace=foreign, mode="lexical"))
            assert error.value.code == "cognition.instance_mismatch"

    asyncio.run(scenario())
