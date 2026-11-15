"""Published snapshots preserve delivered passages, readers, and captured generations."""

import asyncio
import json
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from unittest.mock import patch

import pytest
import sqlalchemy as sa

from nous.application.contracts import EventEnvelope, Notice, Progress, Saved
from nous.cognition.contracts import NamespaceRef, RevisionSelection
from nous.cognition.feedback import RECORD_QUERY_FEEDBACK, QueryFeedbackRequest
from nous.cognition.material_contracts import MaterialSelection, ReadMaterial
from nous.cognition.materials import READ_MATERIAL
from nous.cognition.retrieval.contracts import (
    ContextSelection,
    DeliveryRequest,
    QueryAnchor,
    QueryAtom,
    QueryIntent,
    QueryRequest,
    RefreshRetrieval,
    SaveEvidence,
    Selector,
)
from nous.cognition.retrieval.operations import QUERY, REFRESH, SAVE_EVIDENCE
from nous.cognition.work.contracts import CreateWork, RecordWorkUse, WorkContext, WorkState
from nous.cognition.work.operations import CREATE_WORK, RECORD_WORK_USE
from nous.cognition.writing.contracts import AddNote, AfterWrite, ReadRecord
from nous.cognition.writing.operations import ADD_NOTE, READ_RECORD
from nous.common.errors import NousError
from nous.common.material_refs import RecordRevisionRef, WorkRevisionRef
from nous.content.contracts import (
    DeleteResult,
    DeliveryBudget,
    InlineText,
    ReadSelections,
    ResultOutputRef,
)
from nous.content.operations import DELETE_RESULT, READ_SELECTIONS
from nous.presentation.evidence import format_source_text
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema
from nous.wave.contracts import (
    BuiltProduct,
    CorpusView,
    PortRef,
    ProgramNode,
    ResolvedBuildProfile,
    WaveCleanup,
)
from nous.wave.operations import WAVE_CLEANUP
from nous.wave.program import default_programs


def test_cognitive_passages_feedback_and_safe_cleanup(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            body = (
                "".join(
                    f"## Section {index}\n\nOrdinary context for a lengthy note.\n\n"
                    for index in range(220)
                )
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
            assert group.material == record.commit.revision_ref
            assert title.evidence.intent.expression is None
            assert title.evidence.intent.controls is None
            assert len(group.model_dump_json()) < 5000
            assert group.rank == 1 and group.score is not None
            assert title.evidence.snapshot is not None
            direct = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text=f'@ref("{record.commit.revision_ref.identifier}")',
                    delivery=DeliveryRequest(budget=DeliveryBudget(amount=100, unit="characters")),
                ),
            )
            direct_group = direct.evidence.materials[0]
            assert direct_group.material == record.commit.revision_ref
            assert len(direct_group.model_dump_json()) < 5000

            found = await runtime.app.call(QUERY, QueryRequest(text="embervault", save=True))
            passage = found.evidence.materials[0].passages[0]
            assert "embervault" in "".join(item.body_text or "" for item in passage.views)
            assert (passage.views[0].source.selection.start_byte or 0) > 500
            selections = tuple(item.source for item in passage.views)
            views = await runtime.app.call(READ_SELECTIONS, ReadSelections(selections=selections))
            assert tuple(view.body_text for view in views) == tuple(
                fragment.body_text for fragment in passage.views
            )
            assert found.evidence_ref is not None
            saved_found = await runtime.results.get(found.evidence_ref)
            stored_intent = saved_found.request["intent"]
            assert isinstance(stored_intent, dict)
            assert "expression" not in stored_intent and "controls" not in stored_intent
            passage_source = next(
                source
                for source in saved_found.sources_for("evidence")
                if source.key == passage.key
            )
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
                selections=passage_source.selections,
            )
            old_ref = found.evidence.snapshot
            assert old_ref is not None

            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A second independent note."), remember=True, after=policy
                ),
            )
            second = await runtime.app.call(QUERY, QueryRequest(text="independent"))
            second_ref = second.evidence.snapshot
            assert second_ref is not None
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
            assert current.evidence.snapshot is not None
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
            assert "embervault" in (old.evidence.materials[0].passages[0].views[0].body_text or "")
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
            record = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(
                        text="资源：Wait for the resource to close before deleting it."
                    ),
                    remember=True,
                    after=policy,
                ),
            )
            async with runtime.database.read() as tx:
                before = await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(schema.snapshots)
                )
            direct = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text=f'@ref("{record.commit.revision_ref.identifier}")',
                    delivery=DeliveryRequest(budget=DeliveryBudget(amount=8, unit="bytes")),
                    save=True,
                ),
            )
            assert direct.evidence.snapshot is None and direct.evidence_ref is not None
            assert all(item.rank is None for item in direct.evidence.materials)
            assert direct.evidence.program is None
            assert direct.evidence.continuation[0].relation in {"read", "read_remaining"}
            assert direct.evidence.delivery_usage is not None
            assert direct.evidence.delivery_usage.unit == "bytes"
            assert direct.evidence.delivery_usage.amount <= 8
            async with runtime.database.read() as tx:
                after = await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(schema.snapshots)
                )
            assert after == before
            direct_feedback = await runtime.app.call(
                RECORD_QUERY_FEEDBACK,
                QueryFeedbackRequest(evidence=direct.evidence_ref, category="retrieval"),
            )
            assert direct_feedback.target.query is not None
            assert not direct_feedback.target.query.snapshot_available
            with pytest.raises(NousError) as retained:
                await runtime.app.call(DELETE_RESULT, DeleteResult(ref=direct.evidence_ref))
            assert retained.value.code == "content.result_in_use"
            with pytest.raises(NousError) as anchored_direct:
                await runtime.app.call(
                    QUERY,
                    QueryRequest(
                        text=f'@ref("{record.commit.revision_ref.identifier}")',
                        anchor=QueryAnchor(view="anchored"),
                    ),
                )
            assert anchored_direct.value.code == "input.unsupported_query_control"
            found = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="resource close",
                    expression=QueryAtom(
                        text="",
                        selectors=(
                            Selector(kind="ref", value=record.commit.revision_ref.identifier),
                        ),
                    ),
                ),
            )
            assert found.evidence_ref is None and any(
                item.passages for item in found.evidence.materials
            )
            assert found.evidence.snapshot is not None
            material = found.evidence.materials[0]
            passage = material.passages[0]
            assert isinstance(material.material, RecordRevisionRef)
            passage_source = material.sources()[0]
            wrong = passage.views[0].model_copy(update={"body_text": "invented"})
            changed = passage.model_copy(update={"views": (wrong,)})
            forged = found.evidence.model_copy(
                update={"materials": (material.model_copy(update={"passages": (changed,)}),)}
            )
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
            _, artifact = await runtime.results.resolve_output(saved, "evidence")
            saved_delivery = json.loads(await runtime.artifacts.read_text(artifact))
            assert "materials" not in saved_delivery and "intent" not in saved_delivery
            assert "program" not in saved_delivery
            saved_result = await runtime.results.get(saved)
            assert saved_result.parameters["program"] == found.evidence.program
            saved_intent = QueryIntent.model_validate(saved_result.request.get("intent"))
            assert saved_intent.expression == found.evidence.intent.expression
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
                        MaterialSelection(
                            material=material.material, selections=passage_source.selections
                        ),
                    ),
                    purpose="Keep the closing condition",
                ),
            )
            assert use.evidence == saved and use.selections[0].material == material.material
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
            assert initial.evidence.snapshot is not None
            work = await runtime.app.call(
                CREATE_WORK,
                CreateWork(
                    state=WorkState(title="Resource handling", objective="Close connection")
                ),
            )
            entered, release = asyncio.Event(), asyncio.Event()
            original = runtime.work.context

            async def delayed(
                ref: str | WorkRevisionRef, namespace: NamespaceRef, window: int
            ) -> WorkContext:
                if ref == work.revision_ref:
                    entered.set()
                    await release.wait()
                return await original(ref, namespace, window)

            with patch.object(runtime.work, "context", new=delayed):
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
            assert saved.evidence.snapshot is not None
            assert saved.evidence.snapshot == initial.evidence.snapshot
            assert saved.evidence_ref is not None and any(
                item.passages for item in saved.evidence.materials
            )
            removed = await runtime.app.call(WAVE_CLEANUP, WaveCleanup())
            assert initial.evidence.snapshot.identifier in removed.snapshots
            read = await runtime.app.call(
                READ_MATERIAL,
                ReadMaterial(
                    delivery=saved.evidence_ref,
                    key=saved.evidence.materials[0].passages[0].key,
                ),
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
            latest = await runtime.app.call(QUERY, QueryRequest(text="update", save=True))
            removed = await runtime.app.call(WAVE_CLEANUP, WaveCleanup())
            assert anchored.evidence_ref is not None
            assert anchored.evidence.snapshot is not None
            assert anchored.evidence.snapshot.identifier not in removed.snapshots
            assert latest.evidence_ref is not None
            await runtime.app.call(
                CREATE_WORK,
                CreateWork(
                    state=WorkState(
                        title="Protected result",
                        objective="Keep the evidence reference",
                        related=(ResultOutputRef(result=latest.evidence_ref, output="evidence"),),
                    )
                ),
            )
            with pytest.raises(NousError) as in_use:
                await runtime.app.call(DELETE_RESULT, DeleteResult(ref=latest.evidence_ref))
            assert in_use.value.code == "content.result_in_use"
            await runtime.app.call(DELETE_RESULT, DeleteResult(ref=anchored.evidence_ref))
            removed = await runtime.app.call(WAVE_CLEANUP, WaveCleanup())
            assert anchored.evidence.snapshot.identifier in removed.snapshots

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
            assert (
                any(item.passages for item in result.evidence.materials)
                and result.evidence_ref is not None
            )
            assert result.evidence.issues and result.evidence.issues[0].stage == "passages.evaluate"
            assert result.evidence.issues[0].code == "model.task_unconfigured"
            assert not result.issues
            assert any(isinstance(item.event, Saved) for item in events)
            assert any(
                isinstance(item.event, Notice) and item.event.issue == result.evidence.issues[0]
                for item in events
            )
            assert {item.event.stage for item in events if isinstance(item.event, Progress)} >= {
                "retrieval",
                "evidence.save",
            }
            _, output = await runtime.results.resolve_output(result.evidence_ref, "evidence")
            saved = json.loads(await runtime.artifacts.read_text(output))
            assert saved["issues"] == [item.to_dict() for item in result.evidence.issues]

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
            material, item = next(
                (material, part)
                for material in result.evidence.materials
                for part in material.passages
                if part.shared
            )
            reading = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(delivery=result.evidence_ref, key=item.key)
            )
            actual = "".join(part.body_text or "" for part in reading.selections)
            shared = await runtime.artifacts.read_selections(item.shared)
            assert all(part.text in actual for part in shared)
            assert "must not close early" in actual
            saved = await runtime.results.get(result.evidence_ref)
            named = next(part for part in saved.sources_for("evidence") if part.key == item.key)
            assert named == next(source for source in material.sources() if source.key == item.key)
            views = tuple(
                view
                for material in result.evidence.materials
                for part in material.passages
                for view in part.views
            )
            covered: dict[str, list[tuple[int, int]]] = {}
            for view in views:
                start = view.source.selection.start_byte
                end = view.source.selection.end_byte
                assert start is not None and end is not None
                ranges = covered.setdefault(view.source.artifact.id, [])
                assert all(end <= left or start >= right for left, right in ranges)
                ranges.append((start, end))
            delivered = "".join(view.body_text or "" for view in views)
            visible = format_source_text(views, ())
            assert visible.count("Shared heading") == 1
            assert visible.count("must not close early") == 1
            assert delivered.count("Shared heading") == 1
            assert delivered.count("must not close early") == 1
            assert result.evidence.delivery_usage is not None
            assert result.evidence.delivery_usage.unit == "characters"
            assert result.evidence.delivery_usage.amount == len(delivered) <= 1000
            _, artifact = await runtime.results.resolve_output(result.evidence_ref, "evidence")
            restored = json.loads(await runtime.artifacts.read_text(artifact))
            assert "program" not in restored
            assert restored["delivery_usage"] == result.evidence.delivery_usage.model_dump(
                mode="json"
            )

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
