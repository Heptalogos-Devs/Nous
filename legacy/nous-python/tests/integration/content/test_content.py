"""Committed user content survives contention, transaction failure and reopening."""

import asyncio
import io
import shutil
import sqlite3
import subprocess
import wave
from pathlib import Path

import pypdfium2 as pdfium
import pytest
from PIL import Image

from nous.common.errors import NousError
from nous.content.contracts import (
    ArtifactInput,
    ContentSelection,
    InputGroup,
    InputPart,
    OutputContent,
    ProcessingResult,
    ProgramProducer,
    ResultFacts,
    SelectedInputs,
    SelectedSource,
    SourceSelection,
)
from nous.content.operations import READ_ARTIFACT
from nous.models.generation.contracts import PreviewPrompt
from nous.models.generation.operations import PROMPT_PREVIEW
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_content_result_survives_independent_writer_and_reopen(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(
            instance, environment={}, overrides={"database.busy_timeout_ms": 3000}
        ) as runtime:
            original = await runtime.artifacts.ingest("原文\n\nwith exact text\n")
            database_path = (
                runtime.paths.data / runtime.configuration.snapshot.values.database.filename
            )
            blocker = sqlite3.connect(database_path, timeout=0, autocommit=True)
            blocker.execute("BEGIN IMMEDIATE")
            waiting = asyncio.create_task(
                runtime.results.adopt(
                    facts=ResultFacts(
                        operation="convert",
                        producer=ProgramProducer(operation="convert"),
                        selected_inputs=SelectedInputs(
                            sources=(
                                SelectedSource(
                                    slot_key="S1",
                                    selections=(ContentSelection(artifact=original.ref),),
                                ),
                            )
                        ),
                        inputs=(
                            InputGroup(
                                key="S1",
                                slot_key="S1",
                                name="original",
                                parts=(InputPart(content=ContentSelection(artifact=original.ref)),),
                            ),
                        ),
                        formed_at=runtime.artifacts.clock.utc_now(),
                    ),
                    outputs={"output": OutputContent(data="派生结果\n".encode())},
                )
            )
            try:
                with pytest.raises(TimeoutError):
                    await asyncio.wait_for(asyncio.shield(waiting), timeout=0.15)
                blocker.execute("COMMIT")
                result = await asyncio.wait_for(waiting, timeout=5)
                output = result.outputs[0].artifact
                assert (await runtime.artifacts.get(output)).producer_result_ref == result.ref
            finally:
                if blocker.in_transaction:
                    blocker.execute("ROLLBACK")
                blocker.close()
                await asyncio.gather(waiting, return_exceptions=True)
        async with open_instance(instance, environment={}) as runtime:
            saved = await runtime.results.get(result.ref)
            assert await runtime.artifacts.read_text(saved.outputs[0].artifact) == "派生结果\n"
            assert (
                await runtime.artifacts.read_text(saved.inputs[0].parts[0].content.artifact)
                == "原文\n\nwith exact text\n"
            )
            roots = await runtime.results.provenance((saved.outputs[0].artifact,))
            assert roots[saved.outputs[0].artifact.id].terminal_roots == (original.ref,)
            assert roots[saved.outputs[0].artifact.id].complete
            background = await runtime.artifacts.ingest("Unrelated editing context.")
            product = ContentSelection(artifact=saved.outputs[0].artifact)
            context = ContentSelection(artifact=background.ref)
            composition = await runtime.results.adopt(
                facts=ResultFacts(
                    operation="writing.compose",
                    producer=ProgramProducer(operation="writing.compose"),
                    selected_inputs=SelectedInputs(
                        sources=(
                            SelectedSource(
                                slot_key="PRODUCT", role="context", selections=(product,)
                            ),
                            SelectedSource(
                                slot_key="BASE", role="revision_base", selections=(context,)
                            ),
                        )
                    ),
                    inputs=(
                        InputGroup(
                            key="PRODUCT",
                            slot_key="PRODUCT",
                            name="product",
                            role="context",
                            parts=(InputPart(content=product),),
                        ),
                        InputGroup(
                            key="BASE",
                            slot_key="BASE",
                            name="base",
                            role="revision_base",
                            parts=(InputPart(content=context),),
                        ),
                    ),
                    formed_at=runtime.artifacts.clock.utc_now(),
                ),
                outputs={"output": OutputContent(data=b"composed")},
            )
            provenance = await runtime.results.provenance((composition.outputs[0].artifact,))
            support = provenance[composition.outputs[0].artifact.id]
            assert support.support_roots == (original.ref,)
            assert {item.id for item in support.terminal_roots} == {
                original.ref.id,
                background.ref.id,
            }
            exact = ContentSelection(
                artifact=original.ref, selection=SourceSelection(start_line=3, end_line=3)
            )
            assert await runtime.artifacts.read_selection(exact) == "with exact text\n"

    asyncio.run(scenario())


def test_pdf_preparation_reuses_pages_and_rejects_foreign_coordinates(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "pdf-selections"
        await initialize_instance(instance, environment={})
        data = io.BytesIO()
        document: pdfium.PdfDocument = pdfium.PdfDocument.new()
        try:
            for width in (90, 120, 150):
                page: pdfium.PdfPage = document.new_page(width=width, height=90)
                page.close()
            document.save(data)
        finally:
            document.close()
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest(
                data.getvalue(), media_type="application/pdf", display_name="pages.pdf"
            )
            text = await runtime.artifacts.ingest("first line\nsecond line\n")
            image_data = io.BytesIO()
            Image.new("RGB", (32, 32), "red").save(image_data, format="PNG")
            picture = await runtime.artifacts.ingest(image_data.getvalue(), media_type="image/png")
            settings = runtime.configuration.snapshot.values.media
            for artifact, selections in (
                (
                    text,
                    (
                        SourceSelection(pages=(1,)),
                        SourceSelection(frame=0),
                        SourceSelection(start_ms=1),
                        SourceSelection(crop=(0, 0, 1, 1)),
                    ),
                ),
                (
                    original,
                    (
                        SourceSelection(frame=0),
                        SourceSelection(start_line=1),
                        SourceSelection(start_ms=1),
                        SourceSelection(crop=(0, 0, 1, 1)),
                    ),
                ),
            ):
                for selection in selections:
                    with pytest.raises(NousError) as invalid:
                        await runtime.media.preparer.prepare(
                            artifact.ref, selection, settings=settings
                        )
                    assert invalid.value.code == "media.invalid_selection"
            prepared = await asyncio.gather(
                *(
                    runtime.media.preparer.prepare(
                        original.ref,
                        SourceSelection(pages=((index % 3) + 1,)),
                        settings=settings,
                        document_mode="visual",
                    )
                    for index in range(12)
                )
            )
            materialized = await asyncio.gather(
                *(runtime.media.preparer.materialize(item.batches[0]) for item in prepared)
            )
            by_page: dict[int, set[str]] = {}
            for item, batch in zip(prepared, materialized, strict=True):
                number = item.selection.pages[0]
                by_page.setdefault(number, set()).update(
                    ref.identifier for ref in batch.completed_result_refs
                )
                assert batch.inputs[0].parts[0].origins[0].origin.selection.pages == (number,)
                with Image.open(io.BytesIO(batch.sources[0].media[0].data)) as image:
                    assert image.width == (90, 120, 150)[number - 1] * 2
            assert len(by_page) == 3 and all(len(refs) == 1 for refs in by_page.values())
            preview = await runtime.app.call(
                PROMPT_PREVIEW,
                PreviewPrompt(
                    prompt="summarize",
                    sources=(
                        ArtifactInput(
                            artifact=original.ref,
                            selection=SourceSelection(pages=(2,)),
                        ),
                        ArtifactInput(
                            artifact=text.ref,
                            selection=SourceSelection(start_line=2, end_line=2),
                        ),
                        ArtifactInput(
                            artifact=picture.ref,
                            selection=SourceSelection(crop=(2, 3, 20, 21)),
                        ),
                    ),
                ),
            )
            messages = str(preview["messages"])
            assert "源文档共 3 页。" in messages and "本次提供第 2 页。" in messages
            assert "本次提供的是源文本的摘录。" in messages
            assert "源图像尺寸为 32 × 32 像素。" in messages
            assert "源图像像素坐标 (2, 3)–(20, 21) 内的裁剪。" in messages
            assert "second line" in messages and "first line" not in messages
        async with open_instance(instance, environment={}) as runtime:
            settings = runtime.configuration.snapshot.values.media
            again = await runtime.media.preparer.prepare(
                original.ref,
                SourceSelection(pages=(2,)),
                settings=settings,
                document_mode="visual",
            )
            again_batch = await runtime.media.preparer.materialize(again.batches[0])
            assert {ref.identifier for ref in again_batch.completed_result_refs} == by_page[2]

    asyncio.run(scenario())


def test_audio_video_reject_coordinates_they_do_not_apply(tmp_path: Path) -> None:
    ffmpeg, ffprobe = shutil.which("ffmpeg"), shutil.which("ffprobe")
    if not ffmpeg or not ffprobe:
        pytest.skip("FFmpeg and ffprobe are required for audiovisual preparation")
    audio_path = tmp_path / "sound.wav"
    with wave.open(str(audio_path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(16000)
        output.writeframes(b"\0\0" * 16000)
    video_path = tmp_path / "video.mp4"
    subprocess.run(
        [
            ffmpeg,
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=32x32:r=2:d=1",
            "-c:v",
            "mpeg4",
            str(video_path),
        ],
        check=True,
        capture_output=True,
    )

    async def scenario() -> None:
        instance = tmp_path / "av-selections"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            audio = await runtime.artifacts.ingest(audio_path)
            video = await runtime.artifacts.ingest(video_path)
            settings = runtime.configuration.snapshot.values.media
            for artifact, selections in (
                (
                    audio,
                    (
                        SourceSelection(pages=(1,)),
                        SourceSelection(frame=0),
                        SourceSelection(start_line=1),
                    ),
                ),
                (
                    video,
                    (
                        SourceSelection(frame=19),
                        SourceSelection(frame=999),
                        SourceSelection(crop=(0, 0, 2, 2)),
                    ),
                ),
            ):
                for selection in selections:
                    with pytest.raises(NousError) as invalid:
                        await runtime.media.preparer.prepare(
                            artifact.ref, selection, settings=settings
                        )
                    assert invalid.value.code == "media.invalid_selection"

            selected = SourceSelection(start_ms=250, end_ms=750)
            planned_audio = await runtime.media.preparer.prepare(
                audio.ref, selected, settings=settings
            )
            audio_batch = await runtime.media.preparer.materialize(planned_audio.batches[0])
            audio_part = audio_batch.inputs[0].parts[0]
            assert audio_part.content.selection == SourceSelection()
            assert audio_part.origins[0].origin.selection == selected
            with wave.open(io.BytesIO(audio_batch.sources[0].media[0].data)) as segment:
                assert len(segment.readframes(16000)) == 8000 * 2
            planned_video = await runtime.media.preparer.prepare(video.ref, settings=settings)
            video_batch = await runtime.media.preparer.materialize(planned_video.batches[0])
            assert video_batch.sources[0].media[0].data.startswith(b"\x89PNG")
            assert video_batch.inputs[0].parts[0].origins[0].origin.selection.start_ms == 0
            preview = await runtime.app.call(
                PROMPT_PREVIEW,
                PreviewPrompt(
                    prompt="describe-media", sources=(ArtifactInput(artifact=video.ref),)
                ),
            )
            messages = str(preview["messages"])
            assert "源视频时长约 1 秒。" in messages
            assert "本次提供源视频第 0 秒的采样画面。" in messages

    asyncio.run(scenario())


def test_result_and_output_references_share_callers_transaction(tmp_path: Path) -> None:
    from nous.application.preparation import TextPreparation

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest("source")
            prepared = await runtime.results.prepare_outputs(
                {"output": OutputContent(data=b"derived")}
            )
            selected = ContentSelection(artifact=original.ref)
            facts = ResultFacts(
                operation="convert",
                producer=ProgramProducer(operation="convert"),
                selected_inputs=SelectedInputs(
                    sources=(SelectedSource(slot_key="S1", selections=(selected,)),)
                ),
                inputs=(
                    InputGroup(
                        key="S1",
                        slot_key="S1",
                        name="original",
                        parts=(InputPart(content=selected),),
                    ),
                ),
                formed_at=runtime.artifacts.clock.utc_now(),
            )
            rolled_back: ProcessingResult | None = None
            with pytest.raises(RuntimeError, match="outer owner failed"):
                async with runtime.database.write() as tx:
                    rolled_back = await runtime.results.adopt_in(
                        tx,
                        facts=facts,
                        prepared_outputs=prepared,
                    )
                    raise RuntimeError("outer owner failed")
            assert rolled_back is not None
            for reference in (rolled_back.ref, rolled_back.outputs[0].artifact):
                with pytest.raises(NousError, match="does not exist"):
                    if reference.identifier.startswith("res("):
                        await runtime.results.get(reference.identifier)
                    else:
                        await runtime.artifacts.get(reference.identifier)
            async with runtime.database.write() as tx:
                adopted = await runtime.results.adopt_in(
                    tx,
                    facts=facts,
                    prepared_outputs=prepared,
                )
            assert await runtime.artifacts.read_text(adopted.outputs[0].artifact) == "derived"
            cache_facts = facts.model_copy(update={"parameters": {"cache_key": "same"}})
            await runtime.results.adopt(
                facts=cache_facts, outputs={"output": OutputContent(data=b"cached")}
            )
            invalid_ref = original.ref.model_copy(update={"identifier": "art(other)"})
            invalid = cache_facts.model_copy(
                update={
                    "inputs": (
                        InputGroup(
                            key="S1",
                            slot_key="S1",
                            name="original",
                            parts=(InputPart(content=ContentSelection(artifact=invalid_ref)),),
                        ),
                    )
                }
            )
            with pytest.raises(NousError) as error:
                await runtime.results.adopt(
                    facts=invalid, outputs={"output": OutputContent(data=b"cached")}
                )
            assert error.value.code == "content.reference_mismatch"
            preparations = [
                TextPreparation(runtime.artifacts, runtime.media.preparer) for _ in range(4)
            ]
            choices = await asyncio.gather(
                *(item.literal("shared deterministic text", "text.txt") for item in preparations)
            )
            assert len({choice.artifact.id for choice in choices}) == 1

    asyncio.run(scenario())


def test_xml_text_remains_available_across_content_and_model_inputs(tmp_path: Path) -> None:
    import json

    from nous.content.contracts import ReadArtifact
    from nous.models.contracts import ReadingInput
    from nous.models.tasks import builtin_tasks

    async def scenario() -> None:
        instance = tmp_path / "xml-content"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            for media_type in ("application/xml", "application/xhtml+xml"):
                text = "<root>一行</root>\n<next>二行</next>\n"
                artifact = await runtime.artifacts.ingest(text, media_type=media_type)
                selected = ContentSelection(
                    artifact=artifact.ref, selection=SourceSelection(start_line=2, end_line=2)
                )
                view = (await runtime.artifacts.read_view_many((selected,)))[0]
                assert view.body_text == "<next>二行</next>\n"
                assert (
                    await runtime.app.call(READ_ARTIFACT, ReadArtifact(ref=artifact.ref))
                ).body_text == text
                plan = await runtime.media.preparer.prepare(
                    artifact,
                    selected.selection,
                    settings=runtime.configuration.snapshot.values.media,
                )
                source = (await runtime.media.preparer.materialize(plan.batches[0])).sources[0]
                assert source.text == view.body_text
                assert not source.media
                assert source.group is not None
                assert source.group.parts[0].content.artifact == artifact.ref
                task = builtin_tasks()["passage-selection"]
                outputs = await task.outputs(
                    {"keys": [source.key]},
                    ReadingInput("", "", (source,)),
                    runtime.artifacts,
                    structured=True,
                )
                passages = json.loads(outputs["passages"].data)
                assert passages[0]["text"] == view.body_text
                assert passages[0]["content"] == source.group.parts[0].content.model_dump(
                    mode="json"
                )

    asyncio.run(scenario())


def test_image_preparation_validates_selection_before_direct_or_normalized_input(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        instance = tmp_path / "image-selections"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            encoded = io.BytesIO()
            Image.new("RGB", (16, 12), "red").save(encoded, format="PNG")
            original_bytes = encoded.getvalue()
            image = await runtime.artifacts.ingest(
                original_bytes, media_type="image/png", display_name="single.png"
            )
            settings = runtime.configuration.snapshot.values.media
            resized_settings = settings.model_copy(
                update={"image": settings.image.model_copy(update={"max_side": 8})}
            )
            invalid_selections = (
                SourceSelection(frame=99),
                SourceSelection(pages=(99,)),
                SourceSelection(start_line=1),
                SourceSelection(end_line=99),
                SourceSelection(start_ms=0, end_ms=10),
            )
            for options in (settings, resized_settings):
                for selection in invalid_selections:
                    with pytest.raises(NousError) as invalid:
                        await runtime.media.preparer.prepare(image.ref, selection, settings=options)
                    assert invalid.value.code == "media.invalid_selection"

            selected = SourceSelection(frame=0)
            direct = await runtime.media.preparer.prepare(image.ref, selected, settings=settings)
            source = (await runtime.media.preparer.materialize(direct.batches[0])).sources[0]
            assert source.group is not None
            assert (
                source.group.parts[0].content.artifact == image.ref
                and source.media[0].data == original_bytes
            )
            assert direct.selection == source.group.parts[0].content.selection == selected

            crop = SourceSelection(frame=0, crop=(0, 0, 8, 6))
            cropped = await runtime.media.preparer.prepare(image.ref, crop, settings=settings)
            cropped_batch = await runtime.media.preparer.materialize(cropped.batches[0])
            source = cropped_batch.sources[0]
            with Image.open(io.BytesIO(source.media[0].data)) as prepared_image:
                assert prepared_image.size == (8, 6)
            conversion = await runtime.results.get(cropped_batch.completed_result_refs[0])
            assert conversion.inputs[0].parts[0].content.artifact == image.ref
            assert conversion.inputs[0].parts[0].content.selection == crop

            encoded = io.BytesIO()
            Image.new("RGB", (16, 12), "red").save(
                encoded,
                format="GIF",
                save_all=True,
                append_images=[Image.new("RGB", (16, 12), "blue")],
                duration=[100, 200],
                loop=0,
            )
            animation = await runtime.artifacts.ingest(
                encoded.getvalue(), media_type="image/gif", display_name="animation.gif"
            )
            for selection in (SourceSelection(frame=2), SourceSelection(pages=(99,))):
                with pytest.raises(NousError) as invalid:
                    await runtime.media.preparer.prepare(
                        animation.ref, selection, settings=settings
                    )
                assert invalid.value.code == "media.invalid_selection"

            frame = await runtime.media.preparer.prepare(
                animation.ref, SourceSelection(frame=1), settings=settings
            )
            source = (await runtime.media.preparer.materialize(frame.batches[0])).sources[0]
            with Image.open(io.BytesIO(source.media[0].data)) as prepared_image:
                assert prepared_image.convert("RGB").getpixel((0, 0)) == (0, 0, 255)
            assert source.group is not None
            assert source.group.parts[0].origins[0].origin.selection == SourceSelection(frame=1)
            interval = await runtime.media.preparer.prepare(
                animation.ref, SourceSelection(start_ms=100, end_ms=300), settings=settings
            )
            interval_batch = await runtime.media.preparer.materialize(interval.batches[0])
            assert interval_batch.sources[0].metadata["frame"] == 1
            overlapping = SourceSelection(frame=1, start_ms=50, end_ms=150)
            intersection = await runtime.media.preparer.prepare(
                animation.ref, overlapping, settings=settings
            )
            actual = SourceSelection(frame=1, start_ms=100, end_ms=150)
            assert intersection.selection == overlapping
            intersection_batch = await runtime.media.preparer.materialize(intersection.batches[0])
            assert intersection_batch.inputs[0].parts[0].origins[0].origin.selection == actual
            conversion = await runtime.results.get(intersection_batch.completed_result_refs[0])
            assert conversion.inputs[0].parts[0].content.selection == actual
            for disjoint in (
                SourceSelection(frame=1, start_ms=0, end_ms=100),
                SourceSelection(frame=0, start_ms=100, end_ms=200),
                SourceSelection(frame=1, start_ms=400, end_ms=500),
            ):
                with pytest.raises(NousError) as invalid:
                    await runtime.media.preparer.prepare(animation.ref, disjoint, settings=settings)
                assert invalid.value.code == "media.invalid_selection"

    asyncio.run(scenario())


def test_exact_text_selection_and_lossless_edits(tmp_path: Path) -> None:
    from nous.content.edits import ReplaceSelection, prepare_text_edit

    async def scenario() -> None:
        instance = tmp_path / "exact-content"
        await initialize_instance(instance, environment={})
        text = "[label]: source:S1\r\n\r\n中文 x\r\n\r\ntrailing"
        async with open_instance(instance, environment={}) as runtime:
            artifact = await runtime.artifacts.ingest(text)
            view = await runtime.artifacts.text_view(artifact.ref)
            assert "".join(chunk.text for chunk in view.chunks(max_bytes=7)) == text
            assert "".join(chunk.text for chunk in view.complete_chunks(11)) == text
            assert any(
                block.kind == "source_gap" and "[label]:" in block.text for block in view.blocks
            )
            long_paragraph = ("中文正文 " * 80) + "[引用](source:S1)" + ("尾部 " * 80)
            code = "```\n" + ("unchanged code\n" * 30) + "```\n"
            material = long_paragraph + "\n\n" + code
            long_artifact = await runtime.artifacts.ingest(material)
            long_view = await runtime.artifacts.text_view(long_artifact.ref)
            chunks = long_view.complete_chunks(64)
            assert "".join(chunk.text for chunk in chunks) == material
            assert sum("[引用](source:S1)" in chunk.text for chunk in chunks) == 1
            assert any(code in chunk.text for chunk in chunks)
            assert len(chunks) > 3
            start = len(("中文正文 " * 5).encode())
            end = len(long_paragraph.encode()) - len(("尾部 " * 5).encode())
            partial = long_view.slice(SourceSelection(start_byte=start, end_byte=end))
            parts = partial.complete_chunks(64)
            assert len(parts) > 1 and "".join(part.text for part in parts) == partial.text
            assert sum("[引用](source:S1)" in part.text for part in parts) == 1
            assert not partial.blocks
            fence = next(block for block in long_view.structure if block.kind == "fence")
            partial_code = long_view.slice(
                SourceSelection(start_byte=fence.start + 4, end_byte=fence.end - 4)
            )
            assert [chunk.text for chunk in partial_code.complete_chunks(64)] == [partial_code.text]
            assert not partial_code.blocks
            many_blocks = ("first " * 5) + "\n\n" + ("second " * 5) + "\n\n" + ("third " * 5)
            multi_view = await runtime.artifacts.text_view(
                (await runtime.artifacts.ingest(many_blocks)).ref
            )
            packed = multi_view.complete_chunks(64)
            assert len(packed) > 1 and "".join(chunk.text for chunk in packed) == many_blocks
            definition = ContentSelection(
                artifact=artifact.ref, selection=SourceSelection(start_line=1, end_line=1)
            )
            edited = prepare_text_edit(
                view,
                (
                    ReplaceSelection(
                        selection=definition,
                        expected_text="[label]: source:S1\r\n",
                        replacement="[label]: source:S2\r\n",
                    ),
                ),
            )
            assert edited.utf8_bytes == text.replace("source:S1", "source:S2").encode()
            bounds = await runtime.artifacts.normalize_selections((definition,))
            assert bounds[0].selection == SourceSelection(start_byte=0, end_byte=20)
            plan = await runtime.generation.preparer.prepare(
                artifact, bounds[0].selection, settings=runtime.configuration.snapshot.values.media
            )
            batch = await runtime.generation.preparer.materialize(plan.batches[0])
            assert batch.sources[0].text == "[label]: source:S1\r\n"
        async with open_instance(
            instance, environment={}, overrides={"content.text_read_max_bytes": 11}
        ) as runtime:
            selected = ContentSelection(
                artifact=artifact.ref, selection=SourceSelection(start_line=3, end_line=3)
            )
            value = (await runtime.artifacts.read_selections((selected,)))[0]
            assert value.data == "中文 x\r\n".encode()
            assert value.first_line == 3
            byte_value = (await runtime.artifacts.read_selections((value.content,)))[0]
            assert byte_value == value
            local = await runtime.artifacts.text_view(selected)
            assert local.byte_offset == value.start_byte and local.first_line == 3
            assert local.chunks(max_bytes=11)[0].text == value.text
            from nous.application.preparation import TextPreparation

            prefix = "prefix\n" * 8000
            overlapping = await runtime.artifacts.ingest(prefix + "abcdefghijk\n")
            offset = len(prefix.encode())
            selections = tuple(
                ContentSelection(
                    artifact=overlapping.ref,
                    selection=SourceSelection(start_byte=offset + start, end_byte=offset + end),
                )
                for start, end in ((0, 8), (3, 11))
            )
            session = TextPreparation(runtime.artifacts, runtime.generation.preparer)
            prepared = await session.prepare_many(selections)
            assert tuple(item.view.text for item in prepared) == ("abcdefgh", "defghijk")
            assert tuple(item.origin_contents for item in prepared) == tuple(
                (item,) for item in selections
            )
            with pytest.raises(NousError, match="UTF-8"):
                await runtime.artifacts.read_selection(
                    ContentSelection(
                        artifact=artifact.ref,
                        selection=SourceSelection(
                            start_byte=value.start_byte + 1, end_byte=value.start_byte + 2
                        ),
                    )
                )

    asyncio.run(scenario())


def test_adopted_text_projection_maps_exact_regions_without_matching_bytes(tmp_path: Path) -> None:
    from nous.content.projections import semantic_text

    async def scenario() -> None:
        instance = tmp_path / "mapped-projection"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest("before [标签](source:S1) after\n")
            projected = await semantic_text(
                ContentSelection(artifact=original.ref), runtime.results
            )
            assert projected.text == "before 标签 after\n"
            selected = ContentSelection(
                artifact=projected.content.artifact,
                selection=SourceSelection(start_byte=0, end_byte=6),
            )
            mapped = await runtime.artifacts.map_origins(selected, original.ref)
            assert tuple(item.origin.selection for item in mapped) == (
                SourceSelection(start_byte=0, end_byte=6),
            )
            assert (
                mapped[0].transform_ref
                == (await runtime.artifacts.get(projected.content.artifact)).producer_result_ref
            )
            label = ContentSelection(
                artifact=projected.content.artifact,
                selection=SourceSelection(start_byte=7, end_byte=13),
            )
            label_origins = await runtime.artifacts.map_origins(label, original.ref)
            assert (
                await runtime.artifacts.read_selection(label_origins[0].origin)
                == "[标签](source:S1)"
            )
            partial_label = await runtime.artifacts.map_origins(
                ContentSelection(
                    artifact=projected.content.artifact,
                    selection=SourceSelection(start_byte=7, end_byte=10),
                ),
                original.ref,
            )
            assert (
                await runtime.artifacts.read_selection(partial_label[0].origin)
                == "[标签](source:S1)"
            )
            duplicate = await runtime.artifacts.ingest(projected.text)
            with pytest.raises(NousError, match="exact mapping"):
                await runtime.artifacts.map_origins(
                    ContentSelection(artifact=duplicate.ref), original.ref
                )
            whole = await runtime.artifacts.map_origins(projected.content, original.ref)
            assert (
                "".join([await runtime.artifacts.read_selection(item.origin) for item in whole])
                == "before [标签](source:S1) after\n"
            )
        async with open_instance(instance, environment={}) as runtime:
            restored = await runtime.artifacts.map_origins(selected, original.ref)
            assert restored == mapped

    asyncio.run(scenario())


def test_deep_producer_mappings_keep_exact_ranges(tmp_path: Path) -> None:
    """Coordinate recovery follows a long producer chain without Python recursion."""
    from nous.content.contracts import OutputMapping

    async def scenario() -> None:
        instance = tmp_path / "deep-mapping"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest("abc")
            current = ContentSelection(
                artifact=original.ref, selection=SourceSelection(start_byte=0, end_byte=3)
            )
            prepared = await runtime.results.prepare_outputs(
                {
                    "text": OutputContent(
                        data=b"abc", media_type="text/plain", display_name="text.txt"
                    )
                }
            )
            async with runtime.database.write() as tx:
                for _ in range(1050):
                    result = await runtime.results.adopt_in(
                        tx,
                        facts=ResultFacts(
                            operation="projection",
                            producer=ProgramProducer(operation="projection"),
                            selected_inputs=SelectedInputs(
                                sources=(SelectedSource(slot_key="S1", selections=(current,)),)
                            ),
                            inputs=(
                                InputGroup(
                                    key="S1",
                                    slot_key="S1",
                                    name="source",
                                    parts=(InputPart(content=current),),
                                ),
                            ),
                            mappings=(
                                OutputMapping(
                                    output_name="text",
                                    selection=SourceSelection(start_byte=0, end_byte=3),
                                    relation="identity",
                                    origins=(current,),
                                ),
                            ),
                            formed_at=runtime.artifacts.clock.utc_now(),
                        ),
                        prepared_outputs=prepared,
                    )
                    current = ContentSelection(
                        artifact=result.outputs[0].artifact,
                        selection=SourceSelection(start_byte=0, end_byte=3),
                    )
            selected = current.model_copy(
                update={"selection": SourceSelection(start_byte=1, end_byte=2)}
            )
            mapped = await runtime.artifacts.map_origins(selected, original.ref)
            assert len(mapped) == 1
            assert mapped[0].origin.selection == SourceSelection(start_byte=1, end_byte=2)
            assert await runtime.artifacts.read_selection(mapped[0].origin) == "b"

    asyncio.run(scenario())
