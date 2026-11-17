"""Published snapshots preserve delivered passages, readers, and captured generations."""

import asyncio
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from unittest.mock import patch

import pytest
import sqlalchemy as sa

from nous.cognition.feedback import RECORD_QUERY_FEEDBACK, QueryFeedbackRequest
from nous.cognition.retrieval.contracts import CognitiveQuery, RefreshRetrieval
from nous.cognition.retrieval.operations import QUERY, REFRESH
from nous.cognition.writing.contracts import AddNote, AfterWrite, ReadRecord
from nous.cognition.writing.operations import ADD_NOTE, READ_RECORD
from nous.content.contracts import InlineText, ReadSelections
from nous.content.operations import READ_SELECTIONS
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema
from nous.wave.contracts import BuiltProduct, CorpusView, ResolvedBuildProfile


def test_cognitive_passages_feedback_and_safe_cleanup(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            body = (
                "Ordinary context for a lengthy note.\n\n" * 220
                + "The closing answer is embervault."
            )
            record = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text=body),
                    title="Unmistakable quasarheading",
                    remember=True,
                    after=policy,
                ),
            )
            title = await runtime.app.call(
                QUERY, CognitiveQuery(text="quasarheading", mode="lexical")
            )
            group = title.groups[0]
            assert group.context.revision_ref == record.commit.revision_ref
            assert group.passages[0].delivery.reason == "title_preview"
            assert group.passages[0].delivery.matched_fields == ("title",)
            assert group.basis.candidate.snapshot == title.context.snapshot_ref.identifier

            found = await runtime.app.call(QUERY, CognitiveQuery(text="embervault", mode="lexical"))
            delivered = [
                fragment.content
                for item in found.groups[0].passages
                for fragment in item.delivery.fragments
            ]
            assert len(delivered) == len(set(delivered))
            passage = found.groups[0].passages[0]
            assert passage.delivery.reason == "body_match"
            assert "embervault" in passage.delivery.text
            assert (passage.selection.selections[0].selection.start_byte or 0) > 500
            views = await runtime.app.call(
                READ_SELECTIONS, ReadSelections(selections=passage.selection.selections)
            )
            assert tuple(view.body_text for view in views) == tuple(
                fragment.text for fragment in passage.delivery.fragments
            )
            feedback = await runtime.app.call(
                RECORD_QUERY_FEEDBACK,
                QueryFeedbackRequest(
                    result=found, category="retrieval", target=(0, 0), value="positive"
                ),
            )
            assert feedback.target.selection == passage.selection
            old_ref = found.context.snapshot_ref

            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A second independent note."), remember=True, after=policy
                ),
            )
            second = await runtime.app.call(
                QUERY, CognitiveQuery(text="independent", mode="lexical")
            )
            second_ref = second.context.snapshot_ref
            async with runtime.database.read() as tx:
                summary = await tx.connection.scalar(
                    sa.select(schema.snapshots.c.build_summary).where(
                        schema.snapshots.c.id == second_ref.id
                    )
                )
            assert summary is not None
            preparation = summary["statistics"]["preparation"]
            assert preparation["reused_projection_units"] >= 1
            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A third independent note."), remember=True, after=policy
                ),
            )
            current = await runtime.app.call(QUERY, CognitiveQuery(text="third", mode="lexical"))
            async with runtime.wave.snapshots.borrow(second_ref):
                await runtime.wave.cleanup()
                assert (await runtime.wave.describe(second_ref)).ref == second_ref
            removed = (await runtime.wave.cleanup()).snapshots
            assert second_ref.identifier in removed
            assert (await runtime.wave.describe(old_ref)).ref == old_ref
            old = await runtime.app.call(
                QUERY, CognitiveQuery(text="embervault", mode="lexical", snapshot=old_ref)
            )
            assert "embervault" in old.groups[0].passages[0].delivery.text
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=record.commit.revision_ref))
            ).body == body
            assert (
                await runtime.wave.describe(current.context.snapshot_ref)
            ).ref == current.context.snapshot_ref

    asyncio.run(scenario())


def test_interrupted_and_stale_build_never_replace_current_snapshot(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            await runtime.app.call(
                ADD_NOTE, AddNote(body=InlineText(text="old marker"), remember=True, after=policy)
            )
            old = await runtime.app.call(REFRESH, RefreshRetrieval(mode="lexical"))
            descriptor = await runtime.wave.describe(old.snapshot)
            await runtime.app.call(
                ADD_NOTE, AddNote(body=InlineText(text="new marker"), remember=True, after=policy)
            )
            original = runtime.wave.build_product
            entered, release = asyncio.Event(), asyncio.Event()

            @asynccontextmanager
            async def delayed(
                view: CorpusView, profile: ResolvedBuildProfile
            ) -> AsyncGenerator[BuiltProduct]:
                async with original(view, profile) as product:
                    entered.set()
                    await release.wait()
                    yield product

            async with runtime.database.read() as tx:
                before = await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(schema.snapshots)
                )
            with patch.object(runtime.wave, "build_product", new=delayed):
                task = asyncio.create_task(
                    runtime.app.call(REFRESH, RefreshRetrieval(mode="lexical"))
                )
                await asyncio.wait_for(entered.wait(), 15)
                task.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await task
            async with runtime.database.read() as tx:
                after = await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(schema.snapshots)
                )
            assert before == after
            previous = await runtime.wave.current(descriptor.collection)
            assert previous is not None and previous.ref == old.snapshot

            entered.clear()
            with patch.object(runtime.wave, "build_product", new=delayed):
                task = asyncio.create_task(
                    runtime.app.call(REFRESH, RefreshRetrieval(mode="lexical"))
                )
                await asyncio.wait_for(entered.wait(), 15)
                await runtime.app.call(
                    ADD_NOTE,
                    AddNote(body=InlineText(text="concurrent write"), remember=True, after=policy),
                )
                release.set()
                stale = await task
            assert stale.captured_generation is not None
            assert not stale.current and stale.captured_generation < stale.current_generation
            previous = await runtime.wave.current(descriptor.collection)
            assert previous is not None and previous.ref == old.snapshot
            fresh = await runtime.app.call(REFRESH, RefreshRetrieval(mode="lexical"))
            assert fresh.current and fresh.captured_generation == fresh.current_generation

    asyncio.run(scenario())
