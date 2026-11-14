"""Current retrieval preserves addressable text without inventing independent witnesses."""

import asyncio
import json
import shutil
import subprocess
import wave
from pathlib import Path

import pypdfium2 as pdfium
import pytest
from PIL import Image

from nous.application.configuration import open_configuration_operations
from nous.application.inputs import FileInput
from nous.cognition.contracts import SourceBinding
from nous.cognition.retrieval.contracts import QueryControls, QueryRequest, Selector
from nous.cognition.retrieval.operations import QUERY
from nous.cognition.writing.contracts import (
    AddNote,
    AfterWrite,
    CreateJournal,
    ListRecords,
    ReadRecord,
    ReplaceRecord,
    SourceChanges,
    SourceChoice,
)
from nous.cognition.writing.operations import (
    ADD_NOTE,
    CREATE_JOURNAL,
    LIST_RECORDS,
    READ_RECORD,
    REPLACE_RECORD,
)
from nous.common.errors import NousError
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    ExternalInput,
    InlineText,
    InputGroup,
    InputPart,
    OriginSlice,
    SourceSelection,
)
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave.contracts import WaveBuild, WaveQuery, WaveRead
from nous.wave.operations import WAVE_BUILD, WAVE_QUERY, WAVE_READ


def test_deep_source_collection_and_cycle_validation(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "deep-sources"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            source = tmp_path / "source.md"
            source.write_text("A recognizable original source.", encoding="utf-8")
            artifact = (await runtime.artifacts.ingest(source)).ref
            documents = [
                {
                    "key": f"d{index}",
                    "input": {"artifact": artifact.identifier},
                    "derived_from": [f"d{index - 1}"] if index else [],
                }
                for index in range(1500)
            ]
            manifest = tmp_path / "collection.json"
            data = {
                "name": "deep sources",
                "documents": documents,
                "candidates": [
                    {"key": "last", "document": "d1499", "content": {"document": "d1499"}}
                ],
            }
            manifest.write_text(json.dumps(data), encoding="utf-8")
            built = await runtime.app.call(
                WAVE_BUILD,
                WaveBuild(manifest=ExternalInput(key="manifest")),
                inputs={"manifest": FileInput(manifest)},
            )
            snapshot = built.snapshot.identifier
            found = await runtime.app.call(
                WAVE_QUERY, WaveQuery(snapshot=snapshot, text="recognizable", program="lexical")
            )
            assert len(found.hits) == 1
            documents[0]["derived_from"] = ["d1499"]
            manifest.write_text(json.dumps(data), encoding="utf-8")
            with pytest.raises(ValueError, match="cycle"):
                await runtime.app.call(
                    WAVE_BUILD,
                    WaveBuild(manifest=ExternalInput(key="manifest")),
                    inputs={"manifest": FileInput(manifest)},
                )

    asyncio.run(scenario())


def test_media_selections_are_checked_before_formal_writes(tmp_path: Path) -> None:
    pdf = tmp_path / "one.pdf"
    document = pdfium.PdfDocument.new()
    page = document.new_page(200, 200)
    page.close()
    document.save(pdf)
    document.close()
    image = tmp_path / "small.png"
    Image.new("RGB", (16, 12), "red").save(image)
    ffmpeg, ffprobe = shutil.which("ffmpeg"), shutil.which("ffprobe")
    audio = tmp_path / "tone.wav"
    video = tmp_path / "video.mp4"
    if ffmpeg and ffprobe:
        with wave.open(str(audio), "wb") as output:
            output.setnchannels(1)
            output.setsampwidth(2)
            output.setframerate(8000)
            output.writeframes(b"\0\0" * 8000)
        subprocess.run(
            [
                ffmpeg,
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=32x24:r=10:d=1",
                "-c:v",
                "mpeg4",
                "-y",
                str(video),
            ],
            check=True,
        )

    async def scenario() -> None:
        instance = tmp_path / "media-sources"
        await initialize_instance(instance, environment={})
        if ffmpeg and ffprobe:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {"media.ffmpeg_path": ffmpeg, "media.ffprobe_path": ffprobe}
                )
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            original = await runtime.app.call(
                CREATE_JOURNAL, CreateJournal(body=InlineText(text="original body"), after=policy)
            )
            pdf_ref = (await runtime.artifacts.ingest(pdf)).ref
            image_ref = (await runtime.artifacts.ingest(image)).ref
            invalid = [
                ContentSelection(artifact=pdf_ref, selection=SourceSelection(pages=(999,))),
                ContentSelection(
                    artifact=image_ref, selection=SourceSelection(crop=(0, 0, 50, 50))
                ),
                ContentSelection(artifact=image_ref, selection=SourceSelection(frame=2)),
            ]
            valid = [
                ContentSelection(artifact=pdf_ref, selection=SourceSelection(pages=(1,))),
                ContentSelection(
                    artifact=image_ref, selection=SourceSelection(crop=(1, 1, 12, 10))
                ),
            ]
            if ffmpeg and ffprobe:
                for path in (audio, video):
                    ref = (await runtime.artifacts.ingest(path)).ref
                    invalid.append(
                        ContentSelection(
                            artifact=ref, selection=SourceSelection(start_ms=0, end_ms=5000)
                        )
                    )
                    valid.append(
                        ContentSelection(
                            artifact=ref, selection=SourceSelection(start_ms=0, end_ms=500)
                        )
                    )
            namespace = original.commit.namespace.ref
            generation = (await runtime.namespaces.read(namespace)).content_generation
            for selection in invalid:
                with pytest.raises(NousError) as error:
                    await runtime.app.call(
                        CREATE_JOURNAL,
                        CreateJournal(
                            body=InlineText(text="must not save"),
                            sources=(SourceChoice(target=selection),),
                            after=policy,
                        ),
                    )
                assert error.value.code == "media.invalid_selection"
            with pytest.raises(NousError) as error:
                await runtime.app.call(
                    REPLACE_RECORD,
                    ReplaceRecord(
                        target=original.commit.revision_ref,
                        body=InlineText(text="must not change"),
                        source_changes=SourceChanges(
                            add=(SourceBinding(key="N1", selections=(invalid[0],)),)
                        ),
                        after=policy,
                    ),
                )
            assert error.value.code == "media.invalid_selection"
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=original.commit.revision_ref))
            ).body == "original body"
            assert (await runtime.namespaces.read(namespace)).content_generation == generation
            assert len(await runtime.app.call(LIST_RECORDS, ListRecords(kind="journal"))) == 1
            saved = await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(
                    body=InlineText(text="valid selections"),
                    sources=tuple(SourceChoice(target=item) for item in valid),
                    after=policy,
                ),
            )
            read = await runtime.app.call(READ_RECORD, ReadRecord(target=saved.commit.revision_ref))
            assert [source.selections[0] for source in read.sources] == valid

    asyncio.run(scenario())


def test_transformed_media_uses_its_declared_original_selection(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "media-origin"
        await initialize_instance(instance, environment={})
        source_path = tmp_path / "original.png"
        derivative_path = tmp_path / "normalized.png"
        Image.new("RGB", (20, 20), "white").save(source_path)
        Image.new("RGB", (10, 10), "white").save(derivative_path)

        async with open_instance(instance, environment={}) as runtime:
            namespace = await runtime.namespaces.resolve("default")
            source: ArtifactRef = (await runtime.artifacts.ingest(source_path)).ref
            derivative: ArtifactRef = (await runtime.artifacts.ingest(derivative_path)).ref
            selected = ContentSelection(artifact=source)
            resolver = runtime.written_records.sources
            prepared = await resolver.prepare(namespace, [SourceChoice(target=selected)])
            consumed = InputGroup(
                key="S1:I0",
                slot_key="S1",
                name="normalized.png",
                role="source",
                parts=(
                    InputPart(
                        content=ContentSelection(artifact=derivative),
                        representation="normalized_image",
                        media_type="image/png",
                        origins=(OriginSlice(origin=selected),),
                    ),
                ),
            )

            bindings = await resolver.actual_bindings((consumed,), prepared)

            assert bindings[0].selections == (selected,)

            crop = ContentSelection(artifact=source, selection=SourceSelection(crop=(2, 2, 8, 8)))
            prepared_crop = await resolver.prepare(namespace, [SourceChoice(target=crop)])
            crop_bindings = await resolver.actual_bindings((consumed,), prepared_crop)
            assert crop_bindings[0].selections == (crop,)

    asyncio.run(scenario())


def test_inspiration_only_journal_remains_searchable_without_fact_roots(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            note = await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text="A comparison that inspired an idea."), after=policy),
            )
            journal = await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(
                    body=InlineText(text="Speculation about resource ownership."),
                    sources=(SourceChoice(target=note.commit.revision_ref, usage="inspiration"),),
                    after=policy,
                ),
            )
            result = await runtime.app.call(QUERY, QueryRequest(text="ownership"))
            group = next(
                item
                for item in result.evidence.materials
                if item.material.ref == journal.commit.revision_ref
            )
            assert group.material.sources[0].usage == "inspiration"
            passage = next(
                item for item in result.evidence.passages if item.key in group.passage_keys
            )
            assert passage.candidate is not None
            read = await runtime.app.call(
                WAVE_READ,
                WaveRead(
                    snapshot=result.evidence.snapshot.identifier,
                    candidate=passage.candidate.key,
                    sources=True,
                ),
            )
            assert read.sources == ()
            stored = await runtime.app.call(
                READ_RECORD, ReadRecord(target=journal.commit.revision_ref)
            )
            assert stored.sources == group.material.sources

    asyncio.run(scenario())


def test_source_projection_keeps_visible_labels_and_literal_markdown() -> None:
    from nous.content.citations import project_citations

    literal = "`[来源 S1](source:S1:C1)`\n\n```md\n[来源 S1](source:S1:C1)\n```\n"
    ordinary = "[普通链接](https://example.com/source:S1)"
    escaped = r"\[来源 S1](source:S1:C1)"
    original = (
        "正文[个人网站作者自述](source:S1:C1)。\n\n"
        "旧标记[来源 S1:C1](source:S1:C1)与[来源 S1](source:S1:C1)。\n\n"
        "[S1:C2的真实说明](source:S1:C1)\n\n" + literal + "\n" + ordinary + "\n\n" + escaped
    )
    expected = (
        "正文个人网站作者自述。\n\n旧标记来源 S1:C1与来源 S1。\n\nS1:C2的真实说明\n\n"
        + literal
        + "\n"
        + ordinary
        + "\n\n"
        + escaped
    )
    assert project_citations(original) == expected
    assert (
        project_citations("[资料 S1](source:S1:C1)[材料](source:S1:C1)[原文](source:S1:C1)")
        == "资料 S1材料原文"
    )
    assert (
        project_citations("[source S1](source:S1:C1) [자료](source:S1:C1) [原資料](source:S1:C1)")
        == "source S1 자료 原資料"
    )
    assert project_citations("<source:S1:C1>") == "source:S1:C1"
    assert project_citations("[个人网站原文](source:S1:C1)") == "个人网站原文"
    assert project_citations("[AT&amp;T](source:S1:C1)") == "AT&T"
    assert project_citations(r"[a\*b](source:S1:C1)") == "a*b"
    assert project_citations("[**text** and *more*](source:S1:C1)") == "text and more"
    assert project_citations("[see `a*b`](source:S1:C1)") == "see a*b"
    assert project_citations("> [甲\n> 乙](source:S1)\n") == "> 甲\n乙\n"
    assert project_citations("> [甲](\n> source:S1)\n") == "> 甲\n"


def test_generated_source_links_preserve_existing_markdown() -> None:
    from nous.content.citations import normalize_source_links, parse_citations

    existing = "[existing ordinary link](N1:C1)\r\n"
    literal = "`[code](N1:C1)`\r\n\r\n```md\r\n[code](N1:C1)\r\n```\r\n"
    body = (
        existing + '[**visible label**](<N1:C1> "title")\r\n\r\n'
        "> [reference][r]\r\n>\r\n> [r]: N1:C1\r\n\r\n"
        "<N1:C1> [external](https://example.com/N1:C1)\r\n\r\n"
        "![image][picture]\r\n\r\n[picture]: N1:C1\r\n[unused]: N1:C1\r\n\r\n" + literal
    )
    expected = (
        existing + '[**visible label**](<source:N1:C1> "title")\r\n\r\n'
        "> [reference][r]\r\n>\r\n> [r]: source:N1:C1\r\n\r\n"
        "[N1:C1](source:N1:C1) [external](https://example.com/N1:C1)\r\n\r\n"
        "![image][picture]\r\n\r\n[picture]: N1:C1\r\n[unused]: N1:C1\r\n\r\n" + literal
    )
    normalized = normalize_source_links(body, ("N1:C1",), preserve=existing)
    assert normalized.text == expected
    assert tuple(item.key for item in parse_citations(normalized.text).citations) == (
        "N1:C1",
        "N1:C1",
        "N1:C1",
    )
    assert not normalize_source_links(normalized.text, ("N1:C1",), preserve=existing).keys


def test_retrieval_projection_preserves_original_annotation_and_plain_inputs(
    tmp_path: Path,
) -> None:
    from nous.cognition.concept_contracts import AnnotationView
    from nous.content.annotations import AnnotationProposal
    from nous.content.contracts import ContentSelection
    from nous.content.projections import semantic_text

    async def scenario() -> None:
        from nous.cognition.annotation.contracts import AnnotateRecord, ReadAnnotation
        from nous.cognition.annotation.operations import ANNOTATE_RECORD, READ_ANNOTATION

        instance = tmp_path / "projection"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            note = await runtime.app.call(
                ADD_NOTE, AddNote(body=InlineText(text="线程结束后关闭存储。"), after=policy)
            )
            body = "资源释放应等待线程结束。[个人网站作者自述](source:S1)"
            journal = await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(
                    body=InlineText(text=body),
                    sources=(SourceChoice(target=note.commit.revision_ref),),
                    after=policy,
                ),
            )
            raw = await runtime.app.call(
                READ_RECORD, ReadRecord(target=journal.commit.revision_ref)
            )
            chosen = ContentSelection(artifact=raw.body_artifact)
            projected = await semantic_text(chosen, runtime.results)
            assert (await semantic_text(chosen, runtime.results)).content == projected.content
            assert projected.text == "资源释放应等待线程结束。个人网站作者自述"
            provenance = (
                await runtime.results.provenance((projected.content.artifact,), details=True)
            )[projected.content.artifact.id]
            assert any(relation.parent == raw.body_artifact for relation in provenance.relations)
            proposal = AnnotationProposal.model_validate(
                {
                    "concepts": [
                        {
                            "key": "K",
                            "label": "资源释放",
                            "description": "等待工作线程结束后释放其资源",
                        }
                    ],
                    "occurrences": [{"key": "O", "concept": "K", "blocks": ["B1"]}],
                    "paths": [],
                }
            )
            outcome = await runtime.app.call(
                ANNOTATE_RECORD,
                AnnotateRecord(target=journal.commit.revision_ref, proposal=proposal),
            )
            assert outcome.state == "ready"
            annotation = await runtime.app.call(
                READ_ANNOTATION, ReadAnnotation(ref=outcome.annotation_ref)
            )
            assert isinstance(annotation, AnnotationView)
            await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(body=InlineText(text="资源释放需要明确的持有者。"), after=policy),
            )
            query = await runtime.app.call(QUERY, QueryRequest(text="资源释放"))
            assert [group.rank for group in query.evidence.materials] == list(
                range(1, len(query.evidence.materials) + 1)
            )
            hit = next(
                group
                for group in query.evidence.materials
                if group.material.ref == journal.commit.revision_ref
            )
            passage = next(item for item in query.evidence.passages if item.key in hit.passage_keys)
            assert passage.fragments[0].content.artifact == raw.body_artifact
            assert passage.candidate is not None
            candidate = await runtime.app.call(
                WAVE_READ,
                WaveRead(
                    snapshot=query.evidence.snapshot.identifier,
                    candidate=passage.candidate.key,
                ),
            )
            assert candidate.concepts
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=journal.commit.revision_ref))
            ).body == body
            concept = annotation.bindings["K"].ref
            scoped = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="资源释放",
                    controls=QueryControls(
                        selectors=(Selector(kind="tag", value=concept.concept_ref.identifier),)
                    ),
                ),
            )
            assert scoped.evidence.intent.bound_concepts == (concept.concept_ref,)
            assert any(
                group.material.ref == journal.commit.revision_ref
                for group in scoped.evidence.materials
            )
            plain = await runtime.artifacts.ingest("普通文字")
            assert (
                await semantic_text(ContentSelection(artifact=plain.ref), runtime.results)
            ).content.artifact == plain.ref
            only = await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(
                    body=InlineText(text="[](source:S1)"),
                    sources=(SourceChoice(target=note.commit.revision_ref),),
                    after=policy,
                ),
            )
            query = await runtime.app.call(QUERY, QueryRequest(text="S1"))
            assert not query.evidence.materials
            assert (
                await runtime.app.call(READ_RECORD, ReadRecord(target=only.commit.revision_ref))
            ).body == "[](source:S1)"

    asyncio.run(scenario())
