"""Published snapshots preserve delivered passages, readers, and captured generations."""

import asyncio
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from unittest.mock import patch

import pytest
import sqlalchemy as sa

from nous.application.contracts import EventEnvelope, Notice, Progress, Saved
from nous.cognition.contracts import RecordRevisionRef, RevisionSelection
from nous.cognition.feedback import RECORD_QUERY_FEEDBACK, QueryFeedbackRequest
from nous.cognition.material_contracts import MaterialSelection, ReadMaterial
from nous.cognition.materials import READ_MATERIAL
from nous.cognition.retrieval.contracts import (
    ContextSelection,
    DeliveryRequest,
    EvidenceDelivery,
    QueryAnchor,
    QueryRequest,
    RefreshRetrieval,
    SaveEvidence,
)
from nous.cognition.retrieval.operations import QUERY, REFRESH, SAVE_EVIDENCE
from nous.cognition.work.contracts import CreateWork, RecordWorkUse, WorkState, WorkView
from nous.cognition.work.operations import CREATE_WORK, RECORD_WORK_USE
from nous.cognition.work.refs import WorkRevisionRef
from nous.cognition.writing.contracts import AddNote, AfterWrite, ReadRecord
from nous.cognition.writing.operations import ADD_NOTE, READ_RECORD
from nous.common.errors import NousError
from nous.content.contracts import InlineText, ReadSelections, ResultOutputRef
from nous.content.operations import READ_SELECTIONS
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema
from nous.wave.contracts import (
    BuiltProduct,
    CorpusView,
    DeliveryBudget,
    ResolvedBuildProfile,
    WaveCleanup,
)
from nous.wave.operations import WAVE_CLEANUP
from nous.wave.program import PortRef, ProgramNode, default_programs


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
            title = await runtime.app.call(QUERY, QueryRequest(text="quasarheading"))
            group = title.evidence.materials[0]
            assert group.material.ref == record.commit.revision_ref
            assert group.basis is not None
            assert group.basis.candidate.snapshot == title.evidence.snapshot.identifier

            found = await runtime.app.call(QUERY, QueryRequest(text="embervault", save=True))
            delivered = [
                fragment.content for item in found.evidence.passages for fragment in item.fragments
            ]
            assert len(delivered) == len(set(delivered))
            passage = found.evidence.passages[0]
            assert "embervault" in "".join(item.text for item in passage.fragments)
            assert (passage.fragments[0].content.selection.start_byte or 0) > 500
            selections = tuple(item.content for item in passage.fragments)
            views = await runtime.app.call(READ_SELECTIONS, ReadSelections(selections=selections))
            assert tuple(view.body_text for view in views) == tuple(
                fragment.text for fragment in passage.fragments
            )
            assert found.evidence_ref is not None
            with pytest.raises(NousError, match="Evidence key is not in this delivery"):
                await runtime.app.call(
                    RECORD_QUERY_FEEDBACK,
                    QueryFeedbackRequest(
                        evidence=found.evidence_ref, category="retrieval", passage_key="E999"
                    ),
                )
            feedback = await runtime.app.call(
                RECORD_QUERY_FEEDBACK,
                QueryFeedbackRequest(
                    evidence=found.evidence_ref,
                    category="retrieval",
                    passage_key=passage.key,
                    value="positive",
                ),
            )
            assert feedback.target.selection == RevisionSelection(
                revision=record.commit.revision_ref,
                selections=passage.selections,
            )
            old_ref = found.evidence.snapshot

            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A second independent note."), remember=True, after=policy
                ),
            )
            second = await runtime.app.call(QUERY, QueryRequest(text="independent"))
            second_ref = second.evidence.snapshot
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
            current = await runtime.app.call(QUERY, QueryRequest(text="third"))
            async with runtime.wave.snapshots.borrow(second_ref):
                await runtime.wave.cleanup()
                assert (await runtime.wave.describe(second_ref)).ref == second_ref
            removed = (await runtime.wave.cleanup()).snapshots
            assert second_ref.identifier in removed
            assert (await runtime.wave.describe(old_ref)).ref == old_ref
            old = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="embervault", anchor=QueryAnchor(snapshot=old_ref, view="anchored")
                ),
            )
            assert "embervault" in old.evidence.passages[0].fragments[0].text
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=record.commit.revision_ref))
            ).body == body
            assert (
                await runtime.wave.describe(current.evidence.snapshot)
            ).ref == current.evidence.snapshot

    asyncio.run(scenario())


def test_save_completed_delivery_after_snapshot_cleanup(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(
                        text="资源：Wait for the resource to close before deleting it."
                    ),
                    remember=True,
                    after=policy,
                ),
            )
            found = await runtime.app.call(QUERY, QueryRequest(text="resource close"))
            assert found.evidence_ref is None and found.evidence.passages
            passage = found.evidence.passages[0]
            assert isinstance(passage.material, RecordRevisionRef)
            wrong = passage.fragments[0].model_copy(update={"text": "invented"})
            changed = passage.model_copy(update={"fragments": (wrong,)})
            forged = found.evidence.model_copy(update={"passages": (changed,)})
            with pytest.raises(NousError) as invalid:
                await runtime.app.call(SAVE_EVIDENCE, SaveEvidence(evidence=forged))
            assert invalid.value.code == "cognition.invalid_evidence"

            await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text="An unrelated update."), remember=True, after=policy),
            )
            await runtime.app.call(QUERY, QueryRequest(text="unrelated"))
            removed = await runtime.app.call(WAVE_CLEANUP, WaveCleanup())
            assert found.evidence.snapshot.identifier in removed.snapshots
            saved = await runtime.app.call(SAVE_EVIDENCE, SaveEvidence(evidence=found.evidence))
            artifact = await runtime.results.resolve_output(
                ResultOutputRef(result=saved, output="evidence")
            )
            saved_delivery = EvidenceDelivery.model_validate_json(
                await runtime.artifacts.read_text(artifact)
            )
            assert saved_delivery.formed_at == found.evidence.formed_at
            assert saved_delivery.program_run == found.evidence.program_run
            read = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(delivery=saved, key=passage.key)
            )
            assert "resource" in "".join(part.body_text or "" for part in read.selections)
            feedback = await runtime.app.call(
                RECORD_QUERY_FEEDBACK,
                QueryFeedbackRequest(evidence=saved, category="retrieval", passage_key=passage.key),
            )
            assert feedback.target.query is not None
            assert not feedback.target.query.snapshot_available
            work = await runtime.app.call(
                CREATE_WORK,
                CreateWork(state=WorkState(title="Resource handling", objective="Close safely")),
            )
            use = await runtime.app.call(
                RECORD_WORK_USE,
                RecordWorkUse(
                    revision=work.revision_ref,
                    evidence=saved,
                    selections=(
                        MaterialSelection(material=passage.material, selections=passage.selections),
                    ),
                    purpose="Keep the closing condition",
                ),
            )
            assert use.evidence == saved and use.selections[0].material == passage.material
            direct = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text=f'@ref("{passage.material.identifier}")',
                    delivery=DeliveryRequest(budget=DeliveryBudget(amount=8, unit="bytes")),
                ),
            )
            delivered = "".join(
                fragment.text for item in direct.evidence.passages for fragment in item.fragments
            )
            assert delivered == "资源"
            assert direct.evidence.delivery_usage is not None
            assert direct.evidence.delivery_usage.unit == "bytes"
            assert direct.evidence.delivery_usage.amount == len(delivered.encode("utf-8")) == 6
        async with open_instance(instance, environment={}) as runtime:
            again = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(delivery=saved, key=passage.key)
            )
            assert any("resource" in (part.body_text or "") for part in again.selections)

    asyncio.run(scenario())


def test_query_retains_snapshot_while_preparing_work_context(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="Release resources after the connection closes."),
                    remember=True,
                    after=policy,
                ),
            )
            initial = await runtime.app.call(QUERY, QueryRequest(text="resources"))
            work = await runtime.app.call(
                CREATE_WORK,
                CreateWork(
                    state=WorkState(title="Resource handling", objective="Close connection")
                ),
            )
            entered, release = asyncio.Event(), asyncio.Event()
            original = runtime.work.read_revision

            async def delayed(ref: WorkRevisionRef) -> WorkView:
                entered.set()
                await release.wait()
                return await original(ref)

            with patch.object(runtime.work, "read_revision", new=delayed):
                pending = asyncio.create_task(
                    runtime.app.call(
                        QUERY,
                        QueryRequest(
                            text="resources",
                            context=ContextSelection(work=work.revision_ref),
                            save=True,
                        ),
                    )
                )
                try:
                    await asyncio.wait_for(entered.wait(), 15)
                    await runtime.app.call(
                        ADD_NOTE,
                        AddNote(
                            body=InlineText(text="A new document advances the current view."),
                            remember=True,
                            after=policy,
                        ),
                    )
                    current = await runtime.app.call(QUERY, QueryRequest(text="new document"))
                    assert current.evidence.snapshot != initial.evidence.snapshot
                    removed = await runtime.app.call(WAVE_CLEANUP, WaveCleanup())
                    assert initial.evidence.snapshot.identifier not in removed.snapshots
                finally:
                    release.set()
                saved = await pending
            assert saved.evidence.snapshot == initial.evidence.snapshot
            assert saved.evidence_ref is not None and saved.evidence.passages
            removed = await runtime.app.call(WAVE_CLEANUP, WaveCleanup())
            assert initial.evidence.snapshot.identifier in removed.snapshots
            read = await runtime.app.call(
                READ_MATERIAL,
                ReadMaterial(delivery=saved.evidence_ref, key=saved.evidence.passages[0].key),
            )
            assert any("resources" in (part.body_text or "") for part in read.selections)
            anchored = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="new document",
                    anchor=QueryAnchor(snapshot=current.evidence.snapshot, view="anchored"),
                    save=True,
                ),
            )
            await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text="Another update."), remember=True, after=policy),
            )
            await runtime.app.call(QUERY, QueryRequest(text="update"))
            removed = await runtime.app.call(WAVE_CLEANUP, WaveCleanup())
            assert anchored.evidence_ref is not None
            assert anchored.evidence.snapshot.identifier not in removed.snapshots

    asyncio.run(scenario())


def test_optional_passage_evaluation_failure_preserves_query_evidence(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            base = default_programs()["text_only"]
            evaluation = ProgramNode(
                id="evaluate",
                operator="passages.evaluate",
                inputs={
                    "passages": PortRef(node="locate", port="passages"),
                    "intent": PortRef(node="query", port="intent"),
                },
                parameters={"method": "rank", "policy": "optional"},
            )
            nodes = tuple(
                node.model_copy(
                    update={
                        "inputs": {
                            **node.inputs,
                            "passages": PortRef(node="evaluate", port="passages"),
                        }
                    }
                )
                if node.id == "select"
                else node
                for node in base.nodes
            )
            configured = base.model_copy(update={"nodes": (*nodes, evaluation)})
            await runtime.configuration.update(
                {
                    "wave.programs.assessed": configured.model_dump(mode="json"),
                    "wave.default_program": "assessed",
                }
            )
            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="Workers exit before resources are released."),
                    remember=True,
                    after=AfterWrite(annotation="skip"),
                ),
            )
            events: list[EventEnvelope] = []
            result = await runtime.app.call(
                QUERY, QueryRequest(text="workers exit", save=True), observer=events.append
            )
            assert result.evidence.passages and result.evidence_ref is not None
            assert result.issues and result.issues[0].stage == "passages.evaluate"
            assert result.issues[0].code == "model.task_unconfigured"
            assert result.evidence.issues == result.issues
            assert any(isinstance(item.event, Saved) for item in events)
            assert any(
                isinstance(item.event, Notice) and item.event.issue == result.issues[0]
                for item in events
            )
            assert {item.event.stage for item in events if isinstance(item.event, Progress)} >= {
                "retrieval",
                "evidence.save",
            }
            output = await runtime.results.resolve_output(
                ResultOutputRef(result=result.evidence_ref, output="evidence")
            )
            saved = EvidenceDelivery.model_validate_json(await runtime.artifacts.read_text(output))
            assert saved.issues == result.issues
            assert saved.passages == result.evidence.passages

    asyncio.run(scenario())


def test_saved_evidence_keys_recover_shared_original_context(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            body = (
                "# Shared heading\n\n"
                + "target first "
                + "a " * 187
                + "must not close early. "
                + "Target second "
                + "b " * 200
                + "second ending.\n"
            )
            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text=body), remember=True, after=AfterWrite(annotation="skip")
                ),
            )
            result = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="target",
                    save=True,
                    delivery=DeliveryRequest(budget=DeliveryBudget(amount=1000, unit="characters")),
                ),
            )
            assert result.evidence_ref is not None
            item = next(part for part in result.evidence.passages if part.shared_context)
            reading = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(delivery=result.evidence_ref, key=item.key)
            )
            actual = "".join(part.body_text or "" for part in reading.selections)
            scored = await runtime.artifacts.read_selections(item.scored_ranges)
            shared = await runtime.artifacts.read_selections(item.shared_context)
            assert all(part.text in actual for part in (*scored, *shared))
            assert any("must not close early" in part.text for part in shared)
            assert any("must not close early" in part.text for part in scored)
            assert "must not close early" in actual
            saved = await runtime.results.get(result.evidence_ref)
            named = next(part for part in saved.selections if part.key == item.key)
            assert named.sources == item.selections
            visible = "".join(
                part.text
                for evidence in result.evidence.passages
                for part in (*evidence.fragments, *evidence.support_context)
            )
            assert visible.count("Shared heading") == 1
            assert visible.count("must not close early") == 1
            assert result.evidence.delivery_usage is not None
            assert result.evidence.delivery_usage.unit == "characters"
            assert result.evidence.delivery_usage.amount == len(visible) <= 1000
            assert result.evidence.program_run is not None
            assert result.evidence.program_run.name == result.evidence.program
            assert result.evidence.program_run.signature and result.evidence.program_run.nodes
            artifact = await runtime.results.resolve_output(
                ResultOutputRef(result=result.evidence_ref, output="evidence")
            )
            restored = EvidenceDelivery.model_validate_json(
                await runtime.artifacts.read_text(artifact)
            )
            assert restored.program_run == result.evidence.program_run
            assert restored.delivery_usage == result.evidence.delivery_usage

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
            old = await runtime.app.call(REFRESH, RefreshRetrieval())
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
                task = asyncio.create_task(runtime.app.call(REFRESH, RefreshRetrieval()))
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
                task = asyncio.create_task(runtime.app.call(REFRESH, RefreshRetrieval()))
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
            fresh = await runtime.app.call(REFRESH, RefreshRetrieval())
            assert fresh.current and fresh.captured_generation == fresh.current_generation

    asyncio.run(scenario())
