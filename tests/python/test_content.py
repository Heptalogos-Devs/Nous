"""Committed user content survives contention, transaction failure and reopening."""

import asyncio
import io
import shutil
import sqlite3
import subprocess
import wave
from pathlib import Path
from unittest.mock import patch

import pypdfium2 as pdfium
import pytest
from PIL import Image

from nous.common.errors import NousError
from nous.content.contracts import (
    ContentSelection,
    InputGroup,
    InputPart,
    OutputContent,
    SourceSelection,
)
from nous.media.documents import prepare_pdf_pages
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
                runtime.paths.data / runtime.configuration.snapshot.typed_values().database.filename
            )
            blocker = sqlite3.connect(database_path, timeout=0, autocommit=True)
            blocker.execute("BEGIN IMMEDIATE")
            waiting = asyncio.create_task(
                runtime.results.adopt(
                    operation="convert",
                    producer={"kind": "deterministic", "tool": "integration"},
                    inputs=(
                        InputGroup(
                            key="S1",
                            name="original",
                            parts=(InputPart(content=ContentSelection(artifact=original.ref)),),
                        ),
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
            assert roots[saved.outputs[0].artifact.id] == (original.ref,)
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
        document = pdfium.PdfDocument.new()
        try:
            for width in (90, 120, 150):
                page = document.new_page(width=width, height=90)
                page.close()
            document.save(data)
        finally:
            document.close()
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest(
                data.getvalue(), media_type="application/pdf", display_name="pages.pdf"
            )
            text = await runtime.artifacts.ingest("first line\nsecond line\n")
            settings = runtime.configuration.snapshot.typed_values().media
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
                    assert invalid.value.details["completed_result_refs"] == []
            with patch(
                "nous.media.preparation.prepare_pdf_pages", wraps=prepare_pdf_pages
            ) as render:
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
                assert render.call_count == 3
            by_page: dict[int, set[str]] = {}
            for item in prepared:
                number = item.selection.pages[0]
                by_page.setdefault(number, set()).update(item.completed_result_refs)
                batch = await runtime.media.preparer.materialize(item.batches[0])
                assert batch.inputs[0].parts[0].origins[0].origin.selection.pages == (number,)
                with Image.open(io.BytesIO(batch.sources[0].media[0].data)) as image:
                    assert image.width == (90, 120, 150)[number - 1] * 2
            assert len(by_page) == 3 and all(len(refs) == 1 for refs in by_page.values())
        async with open_instance(instance, environment={}) as runtime:
            settings = runtime.configuration.snapshot.typed_values().media
            with patch(
                "nous.media.preparation.prepare_pdf_pages", wraps=prepare_pdf_pages
            ) as render:
                again = await runtime.media.preparer.prepare(
                    original.ref,
                    SourceSelection(pages=(2,)),
                    settings=settings,
                    document_mode="visual",
                )
                assert render.call_count == 0
            assert set(again.completed_result_refs) == by_page[2]

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
            settings = runtime.configuration.snapshot.typed_values().media
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
                    assert invalid.value.details["completed_result_refs"] == []

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
            assert video_batch.inputs[0].parts[0].origins[0].origin.selection.frame == 0

    asyncio.run(scenario())


def test_result_and_output_references_share_callers_transaction(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            original = await runtime.artifacts.ingest("source")
            prepared = await runtime.results.prepare_outputs(
                {"output": OutputContent(data=b"derived")}
            )
            with pytest.raises(RuntimeError, match="outer owner failed"):
                async with runtime.database.write() as tx:
                    rolled_back = await runtime.results.adopt_in(
                        tx,
                        operation="convert",
                        producer={"kind": "deterministic"},
                        inputs=(
                            InputGroup(
                                key="S1",
                                name="original",
                                parts=(InputPart(content=ContentSelection(artifact=original.ref)),),
                            ),
                        ),
                        prepared_outputs=prepared,
                    )
                    raise RuntimeError("outer owner failed")
            for reference in (rolled_back.ref, rolled_back.outputs[0].artifact):
                with pytest.raises(NousError, match="does not exist"):
                    if reference.address.startswith("res:"):
                        await runtime.results.get(reference.address)
                    else:
                        await runtime.artifacts.get(reference.address)
            async with runtime.database.write() as tx:
                adopted = await runtime.results.adopt_in(
                    tx,
                    operation="convert",
                    producer={"kind": "deterministic"},
                    inputs=(
                        InputGroup(
                            key="S1",
                            name="original",
                            parts=(InputPart(content=ContentSelection(artifact=original.ref)),),
                        ),
                    ),
                    prepared_outputs=prepared,
                )
            assert await runtime.artifacts.read_text(adopted.outputs[0].artifact) == "derived"

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
            settings = runtime.configuration.snapshot.typed_values().media
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
                    assert invalid.value.details["completed_result_refs"] == []

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
            source = (await runtime.media.preparer.materialize(cropped.batches[0])).sources[0]
            with Image.open(io.BytesIO(source.media[0].data)) as prepared_image:
                assert prepared_image.size == (8, 6)
            conversion = await runtime.results.get(cropped.completed_result_refs[0])
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
                assert invalid.value.details["completed_result_refs"] == []

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
            assert interval.batches[0].sources[0].metadata["frame"] == 1
            overlapping = SourceSelection(frame=1, start_ms=50, end_ms=150)
            intersection = await runtime.media.preparer.prepare(
                animation.ref, overlapping, settings=settings
            )
            actual = SourceSelection(frame=1, start_ms=100, end_ms=150)
            assert intersection.selection == overlapping
            assert intersection.batches[0].inputs[0].parts[0].origins[0].origin.selection == actual
            conversion = await runtime.results.get(intersection.completed_result_refs[0])
            assert conversion.inputs[0].parts[0].content.selection == actual
            for disjoint in (
                SourceSelection(frame=1, start_ms=0, end_ms=100),
                SourceSelection(frame=0, start_ms=100, end_ms=200),
                SourceSelection(frame=1, start_ms=400, end_ms=500),
            ):
                with pytest.raises(NousError) as invalid:
                    await runtime.media.preparer.prepare(animation.ref, disjoint, settings=settings)
                assert invalid.value.code == "media.invalid_selection"
                assert invalid.value.details["completed_result_refs"] == []

    asyncio.run(scenario())
