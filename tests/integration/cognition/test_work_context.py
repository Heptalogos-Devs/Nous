"""Continue a real work item from its exact evidence in a new session."""

import asyncio
from pathlib import Path

import pytest

from nous.cognition.material_contracts import FollowMaterial, MaterialSelection, ReadMaterial
from nous.cognition.materials import FOLLOW_MATERIAL, READ_MATERIAL
from nous.cognition.retrieval.contracts import ContextSelection, QueryRequest
from nous.cognition.retrieval.operations import QUERY
from nous.cognition.work.contracts import (
    CreateWork,
    ListWorkUses,
    ReadWork,
    RecordWorkUse,
    ReviseWork,
    WorkDecision,
    WorkState,
)
from nous.cognition.work.operations import (
    CREATE_WORK,
    LIST_WORK_USES,
    READ_WORK,
    RECORD_WORK_USE,
    REVISE_WORK,
)
from nous.cognition.writing.contracts import AddNote, AfterWrite, SourceChoice
from nous.cognition.writing.operations import ADD_NOTE
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection, InlineText
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave.contracts import WaveRead
from nous.wave.operations import WAVE_READ


def test_work_short_question_records_used_evidence_across_sessions(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest(
                "原始讨论：取消时应等待连接退出，并在能够重新计算的条件下释放缓存。"
            )
            record = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    title="取消与资源释放",
                    body=InlineText(
                        text="取消任务时，必须等待连接退出；只有工作能从已提交结果重新计算，才可以释放缓存。"
                    ),
                    remember=True,
                    sources=(SourceChoice(target=ContentSelection(artifact=original.ref)),),
                    after=AfterWrite(annotation="skip"),
                ),
            )
            work = await runtime.app.call(
                CREATE_WORK,
                CreateWork(
                    state=WorkState(
                        title="取消任务决策",
                        objective="确认取消时释放资源的条件",
                        decisions=(
                            WorkDecision(
                                key="release",
                                statement="等待连接退出后再释放取消任务的资源。",
                                basis=(MaterialSelection(material=record.commit.revision_ref),),
                            ),
                        ),
                    )
                ),
            )

        async with open_instance(instance, environment={}) as runtime:
            current = await runtime.app.call(READ_WORK, ReadWork(ref=work.ref.identifier))
            assert current.revision_ref == work.revision_ref
            answer = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="为什么？", context=ContextSelection(work=current.revision_ref), save=True
                ),
            )
            assert answer.evidence_ref is not None
            passage = next(
                part
                for part in answer.evidence.passages
                if part.material == record.commit.revision_ref
            )
            read = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(delivery=answer.evidence_ref, key=passage.key)
            )
            text = "".join(item.body_text or "" for item in read.selections)
            assert "等待连接退出" in text
            assert "已提交结果重新计算" in text
            used = MaterialSelection(
                material=passage.material, selections=tuple(item.source for item in read.selections)
            )
            followed = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=passage.material, relation="reference")
            )
            origin_ref = next(
                item.target for item in followed.relations if item.target == original.ref
            )
            origin = await runtime.app.call(READ_MATERIAL, ReadMaterial(ref=origin_ref))
            assert "原始讨论" in (origin.selections[0].body_text or "")
            supplementary = MaterialSelection(
                material=origin_ref, selections=tuple(item.source for item in origin.selections)
            )
            use = await runtime.app.call(
                RECORD_WORK_USE,
                RecordWorkUse(
                    revision=current.revision_ref,
                    evidence=answer.evidence_ref,
                    selections=(used, supplementary),
                    purpose="说明资源释放条件",
                    decision_key="release",
                ),
            )
            revised_state = current.state.model_copy(
                update={
                    "decisions": (
                        current.state.decisions[0].model_copy(
                            update={
                                "basis": (used, supplementary),
                                "statement": "等待连接退出并保留重算条件。",
                            }
                        ),
                    )
                }
            )
            revised = await runtime.app.call(
                REVISE_WORK,
                ReviseWork(
                    target=current.ref,
                    expected=current.revision_ref,
                    state=revised_state,
                    body=current.body,
                ),
            )
            assert revised.revision_ref.ordinal == current.revision_ref.ordinal + 1
            assert use.selections == (used, supplementary)
            assert use in await runtime.app.call(LIST_WORK_USES, ListWorkUses(ref=current.ref))
            await runtime.configuration.update({"cognition.search.include_work": True})
            projected = await runtime.app.call(QUERY, QueryRequest(text="取消"))
            note_hit = next(
                item
                for item in projected.evidence.passages
                if item.material == record.commit.revision_ref
            )
            work_hit = next(
                item
                for item in projected.evidence.passages
                if item.material == revised.revision_ref
            )
            assert note_hit.candidate is not None and work_hit.candidate is not None
            note_sources = await runtime.app.call(
                WAVE_READ,
                WaveRead(
                    snapshot=projected.evidence.snapshot.identifier,
                    candidate=note_hit.candidate.key,
                    sources=True,
                ),
            )
            work_sources = await runtime.app.call(
                WAVE_READ,
                WaveRead(
                    snapshot=projected.evidence.snapshot.identifier,
                    candidate=work_hit.candidate.key,
                    sources=True,
                ),
            )
            assert note_sources.sources == work_sources.sources
            assert note_sources.sources is not None and len(note_sources.sources) == 1
            with pytest.raises(NousError) as conflict:
                await runtime.app.call(
                    REVISE_WORK,
                    ReviseWork(
                        target=current.ref,
                        expected=current.revision_ref,
                        state=revised_state,
                        body=current.body,
                    ),
                )
            assert conflict.value.code == "cognition.revision_conflict"

    asyncio.run(scenario())
