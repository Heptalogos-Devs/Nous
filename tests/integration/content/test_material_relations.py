"""Completed outputs retain their declared sources across reading and reopening."""

import asyncio
from pathlib import Path

import pytest

from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import FollowMaterial, MaterialSelection, ReadMaterial
from nous.cognition.materials import FOLLOW_MATERIAL, READ_MATERIAL
from nous.cognition.work.contracts import CreateWork, WorkState
from nous.cognition.work.operations import CREATE_WORK
from nous.cognition.writing.contracts import (
    AddNote,
    AdoptDraft,
    AfterWrite,
    DraftFromResult,
    ReadDraft,
    ReadRecord,
    ReplaceRecord,
    ReviseDraft,
    SourceChanges,
    SourceChoice,
)
from nous.cognition.writing.generation import DRAFT_FROM_RESULT, READ_DRAFT, REVISE_DRAFT
from nous.cognition.writing.operations import ADD_NOTE, ADOPT_DRAFT, READ_RECORD, REPLACE_RECORD
from nous.common.errors import NousError
from nous.content.contracts import (
    ContentSelection,
    InlineText,
    InputGroup,
    InputPart,
    MaterialSource,
    NamedSelection,
    OutputContent,
    OutputMapping,
    ProgramProducer,
    ReadResult,
    ResultFacts,
    ResultOutputInput,
    ResultOutputRef,
    SelectedInputs,
    SelectedSource,
    SourceSelection,
)
from nous.content.operations import READ_RESULT
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_result_exposes_only_declared_representation_sources(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest("alpha and beta")
            other = await runtime.artifacts.ingest("unrelated input")
            chosen = ContentSelection(
                artifact=original.ref, selection=SourceSelection(start_byte=0, end_byte=5)
            )
            unselected = ContentSelection(
                artifact=original.ref, selection=SourceSelection(start_byte=10, end_byte=14)
            )
            unrelated = ContentSelection(artifact=other.ref)
            inputs = SelectedInputs(
                sources=(
                    SelectedSource(
                        material=original.ref,
                        key="A",
                        selections=(ContentSelection(artifact=original.ref),),
                    ),
                    SelectedSource(material=unrelated.artifact, key="B", selections=(unrelated,)),
                )
            )
            groups = tuple(
                InputGroup(
                    key=source.key,
                    slot_key=source.key,
                    name=source.key,
                    parts=(InputPart(content=source.selections[0]),),
                )
                for source in inputs.sources
            )
            facts = ResultFacts(
                operation="extract",
                producer=ProgramProducer(),
                selected_inputs=inputs,
                inputs=groups,
                selections=(NamedSelection(key="E1", sources=(chosen,)),),
                mappings=(
                    OutputMapping(
                        output_name="description",
                        selection=SourceSelection(start_byte=0, end_byte=11),
                        relation="derived",
                        origins=(chosen,),
                    ),
                ),
                formed_at=runtime.artifacts.clock.utc_now(),
            )
            outputs = {
                "description": OutputContent(data=b"first value", representation_kind="description")
            }
            result = await runtime.results.adopt(facts=facts, outputs=outputs)
            views = await runtime.artifacts.read_view_many((chosen, unselected, unrelated))
            assert len(views[0].available_representations) == 1
            assert views[0].available_representations[0].source_selections == (chosen,)
            assert not views[1].available_representations
            assert not views[2].available_representations

            foreign = await runtime.artifacts.ingest("not in either input")
            bad = facts.model_copy(
                update={
                    "mappings": (
                        facts.mappings[0].model_copy(
                            update={"origins": (ContentSelection(artifact=foreign.ref),)}
                        ),
                    )
                }
            )
            with pytest.raises(NousError, match="outside the task inputs"):
                await runtime.results.adopt(facts=bad, outputs=outputs)
            whole_text = ContentSelection(
                artifact=original.ref, selection=SourceSelection(start_byte=0, end_byte=14)
            )
            prepared = await runtime.results.prepare_outputs(outputs)
            async with runtime.database.write() as tx:
                later = await runtime.results.adopt_in(
                    tx,
                    facts=facts.model_copy(
                        update={
                            "dependencies": (result.ref, other.ref),
                            "material_sources": (
                                MaterialSource(
                                    key="E1", material=original.ref, selections=(chosen,)
                                ),
                            ),
                            "mappings": (
                                facts.mappings[0].model_copy(update={"origins": (whole_text,)}),
                            ),
                        }
                    ),
                    prepared_outputs=prepared,
                )

        async with open_instance(instance, environment={}) as runtime:
            saved = await runtime.results.get(result.ref)
            async with runtime.database.read() as tx:
                batch = await runtime.results.get_many_in(
                    tx, (later.ref, saved.ref.identifier, later.ref)
                )
                views = await runtime.materials.capture_in(
                    tx, (ResultOutputRef(result=later.ref, output="description"),)
                )
                later_snapshot = (await runtime.cognitive_retrieval.capture.index_in(tx, views))[0]
            assert batch[0] == batch[2] and batch[1] == saved
            assert batch[0].dependencies == (saved.ref, other.ref)
            assert [relation.evidence for relation in later_snapshot.index.support_relations] == [
                (chosen,),
                (
                    ContentSelection(
                        artifact=original.ref, selection=SourceSelection(start_byte=5, end_byte=14)
                    ),
                ),
            ]
            output_source = ResultOutputRef(result=later.ref, output="description")
            note = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A note citing a result output."),
                    sources=(SourceChoice(target=output_source),),
                    after=AfterWrite(annotation="skip"),
                ),
            )
            direct = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=note.commit.revision_ref)
            )
            assert any(relation.target == output_source for relation in direct.relations)
            wrapped = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A note citing the selected result output."),
                    sources=(
                        SourceChoice(
                            target=ResultOutputInput(result=later.ref, output="description")
                        ),
                    ),
                    after=AfterWrite(annotation="skip"),
                ),
            )
            wrapped_relations = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=wrapped.commit.revision_ref)
            )
            assert any(relation.target == output_source for relation in wrapped_relations.relations)
            assert saved.selections == (NamedSelection(key="E1", sources=(chosen,)),)
            ref = ResultOutputRef(result=saved.ref, output="description")
            read = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(selection=MaterialSelection(material=ref))
            )
            relations = await runtime.app.call(FOLLOW_MATERIAL, FollowMaterial(ref=ref))
            assert read.selections[0].body_text == "first value"
            assert relations.relations[0].evidence == (chosen,)
            draft = await runtime.app.call(DRAFT_FROM_RESULT, DraftFromResult(result=saved.ref))
            assert draft.draft is not None
            revised = await runtime.app.call(
                REVISE_DRAFT,
                ReviseDraft(
                    draft=draft.draft, markdown=InlineText(text="Edited finding [E1](source:E1).")
                ),
            )
            adopted = await runtime.app.call(
                ADOPT_DRAFT, AdoptDraft(draft=revised.ref, after=AfterWrite(annotation="skip"))
            )
            record = await runtime.app.call(
                READ_RECORD, ReadRecord(target=adopted.commit.revision_ref, sources=True)
            )
            assert record.body == "Edited finding [E1](source:E1)."
            assert record.sources[0].key == "E1"
            assert record.sources[0].selections == (chosen,)

    asyncio.run(scenario())


def test_work_source_survives_markdown_draft_and_adoption(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            work = await runtime.app.call(
                CREATE_WORK,
                CreateWork(
                    state=WorkState(title="Research", objective="Track the decision"),
                    body="The work records the decision and its constraints.",
                ),
            )
            source = ContentSelection(
                artifact=work.body_artifact,
                selection=SourceSelection(start_byte=0, end_byte=len(work.body.encode("utf-8"))),
            )
            note = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="A note citing the work."),
                    sources=(
                        SourceChoice(
                            target=MaterialSelection(
                                material=work.revision_ref, selections=(source,)
                            )
                        ),
                    ),
                    after=AfterWrite(annotation="skip"),
                ),
            )
            direct_work = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=note.commit.revision_ref)
            )
            assert any(
                relation.target == work.revision_ref and relation.evidence == (source,)
                for relation in direct_work.relations
            )
            content = b"The decision is recorded [E1](source:E1)."
            facts = ResultFacts(
                operation="reading.example",
                producer=ProgramProducer(),
                selected_inputs=SelectedInputs(
                    sources=(
                        SelectedSource(material=source.artifact, key="E1", selections=(source,)),
                    )
                ),
                inputs=(
                    InputGroup(
                        key="E1", slot_key="E1", name="Research", parts=(InputPart(content=source),)
                    ),
                ),
                selections=(NamedSelection(key="E1", sources=(source,)),),
                material_sources=(
                    MaterialSource(
                        key="E1", material=work.revision_ref, selections=(source,), name="Research"
                    ),
                ),
                mappings=(
                    OutputMapping(
                        output_name="markdown",
                        selection=SourceSelection(start_byte=0, end_byte=len(content)),
                        relation="derived",
                        origins=(source,),
                    ),
                ),
                formed_at=runtime.artifacts.clock.utc_now(),
            )
            prepared = await runtime.results.prepare_outputs(
                {
                    "markdown": OutputContent(data=content),
                    "other": OutputContent(data=b"other output", media_type="text/plain"),
                }
            )
            async with runtime.database.write() as tx:
                await runtime.materials.validate_sources_in(
                    tx,
                    facts.material_sources,
                    namespace=NamespaceRef(
                        instance_id=work.ref.instance_id, id=work.ref.namespace_id
                    ),
                )
                product = await runtime.results.adopt_in(
                    tx,
                    facts=facts,
                    prepared_outputs=prepared,
                    output_sources={
                        "other": (
                            MaterialSource(
                                key="E1", material=source.artifact, selections=(source,)
                            ),
                        )
                    },
                )
            draft = await runtime.app.call(DRAFT_FROM_RESULT, DraftFromResult(result=product.ref))
            assert draft.draft is not None
            revised = await runtime.app.call(
                REVISE_DRAFT,
                ReviseDraft(
                    draft=draft.draft,
                    markdown=InlineText(text="Reviewed decision [E1](source:E1)."),
                ),
            )
            assert revised.context.source_bindings[0].material == work.revision_ref
            adopted = await runtime.app.call(
                ADOPT_DRAFT, AdoptDraft(draft=revised.ref, after=AfterWrite(annotation="skip"))
            )
            followed = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=adopted.commit.revision_ref)
            )
            assert any(
                relation.target == work.revision_ref and relation.evidence == (source,)
                for relation in followed.relations
            )
            assert not any(relation.target == source.artifact for relation in followed.relations)
            markdown = ResultOutputRef(result=product.ref, output="markdown")
            refs = (work.revision_ref, adopted.commit.revision_ref, markdown, source.artifact)
            async with runtime.database.read() as tx:
                views = await runtime.materials.capture_in(tx, refs)
                snapshots = await runtime.cognitive_retrieval.capture.index_in(tx, views)
                current = await runtime.materials.current_in(
                    tx,
                    NamespaceRef(instance_id=work.ref.instance_id, id=work.ref.namespace_id),
                )
            assert tuple(item.view.ref for item in snapshots) == refs
            assert all(item.body_digest for item in snapshots)
            assert all(item.view.sections is None for item in snapshots)
            assert len(current) == 3
            assert all(any(item.ref == ref for item in current) for ref in refs[:2])
            assert snapshots[1].view.material_sources[0].material == work.revision_ref
            reads = await runtime.materials.read_many(
                tuple(MaterialSelection(material=ref) for ref in (*refs, refs[0])),
                structure=True,
            )
            assert tuple(item.material.ref for item in reads) == (*refs, refs[0])
            assert reads[0].selections[0].body_text and reads[-1].material.sections
            inspired = await runtime.app.call(
                DRAFT_FROM_RESULT,
                DraftFromResult(
                    result=product.ref,
                    sources=(
                        MaterialSource(
                            key="E1",
                            material=work.revision_ref,
                            selections=(source,),
                            kind="inspiration",
                        ),
                    ),
                ),
            )
            assert inspired.draft is not None
            inspired_record = await runtime.app.call(
                ADOPT_DRAFT, AdoptDraft(draft=inspired.draft, after=AfterWrite(annotation="skip"))
            )
            async with runtime.database.read() as tx:
                views = await runtime.materials.capture_in(
                    tx, (inspired_record.commit.revision_ref,)
                )
                inspired_snapshot = (await runtime.cognitive_retrieval.capture.index_in(tx, views))[
                    0
                ]
            assert inspired_snapshot.view.material_sources[0].material == work.revision_ref
            assert inspired_snapshot.view.material_sources[0].kind == "inspiration"
            assert not inspired_snapshot.index.support_relations
            changed_body = "Revised decision [E1](source:E1)."
            edited = await runtime.app.call(
                REPLACE_RECORD,
                ReplaceRecord(
                    target=adopted.commit.ref,
                    expected=adopted.commit.revision_ref,
                    body=InlineText(text=changed_body),
                    after=AfterWrite(annotation="skip"),
                ),
            )
            retained = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=edited.commit.revision_ref)
            )
            assert any(relation.target == work.revision_ref for relation in retained.relations)
            changed_identity = await runtime.app.call(
                REPLACE_RECORD,
                ReplaceRecord(
                    target=edited.commit.ref,
                    expected=edited.commit.revision_ref,
                    body=InlineText(text=changed_body),
                    after=AfterWrite(annotation="skip"),
                    source_changes=SourceChanges(
                        remove=("E1",),
                        add=(
                            MaterialSource(
                                key="E1",
                                material=source.artifact,
                                selections=(source,),
                                kind="reference",
                                name="Research",
                            ),
                        ),
                    ),
                ),
            )
            assert (
                changed_identity.commit.revision_ref.ordinal
                == edited.commit.revision_ref.ordinal + 1
            )
            now = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=changed_identity.commit.revision_ref)
            )
            assert any(relation.target == source.artifact for relation in now.relations)
            assert not any(relation.target == work.revision_ref for relation in now.relations)
            historical = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=edited.commit.revision_ref)
            )
            assert any(relation.target == work.revision_ref for relation in historical.relations)
        async with open_instance(instance, environment={}) as runtime:
            result_view = await runtime.app.call(
                READ_RESULT, ReadResult(ref=product.ref, sources=True)
            )
            outputs = {item.name: item for item in result_view.outputs}
            assert outputs["markdown"].sources is not None
            assert outputs["other"].sources is not None
            assert outputs["markdown"].sources[0].material == work.revision_ref
            assert outputs["other"].sources[0].material == source.artifact
            assert sum(view.source == source for view in result_view.content_views) == 1
            reread_draft = await runtime.app.call(READ_DRAFT, ReadDraft(draft=revised.ref))
            assert reread_draft.context.source_bindings[0].material == work.revision_ref
            assert reread_draft.context.source_bindings[0].selections == (source,)
            historical = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=edited.commit.revision_ref)
            )
            current = await runtime.app.call(
                FOLLOW_MATERIAL, FollowMaterial(ref=changed_identity.commit.revision_ref)
            )
            assert any(relation.target == work.revision_ref for relation in historical.relations)
            assert any(relation.target == source.artifact for relation in current.relations)

    asyncio.run(scenario())
