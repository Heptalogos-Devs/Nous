"""Committed user content survives contention, transaction failure and reopening."""

import asyncio
import io
import sqlite3
from pathlib import Path

import pytest
from PIL import Image

from nous.common.errors import NousError
from nous.content.contracts import OutputContent, ResultInput, SourceSelection
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
                    inputs=(ResultInput(artifact=original.ref, key="S1"),),
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
                await runtime.artifacts.read_text(saved.inputs[0].artifact)
                == "原文\n\nwith exact text\n"
            )

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
                        inputs=(ResultInput(artifact=original.ref, key="S1"),),
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
                    inputs=(ResultInput(artifact=original.ref, key="S1"),),
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
            source = direct.batches[0].sources[0]
            assert source.artifact == image.ref and source.media[0].data == original_bytes
            assert direct.selection == source.selection == selected

            crop = SourceSelection(frame=0, crop=(0, 0, 8, 6))
            cropped = await runtime.media.preparer.prepare(image.ref, crop, settings=settings)
            source = cropped.batches[0].sources[0]
            with Image.open(io.BytesIO(source.media[0].data)) as prepared_image:
                assert prepared_image.size == (8, 6)
            conversion = await runtime.results.get(cropped.completed_result_refs[0])
            assert conversion.inputs[0].artifact == image.ref
            assert conversion.inputs[0].selection == crop

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
            source = frame.batches[0].sources[0]
            with Image.open(io.BytesIO(source.media[0].data)) as prepared_image:
                assert prepared_image.convert("RGB").getpixel((0, 0)) == (0, 0, 255)
            assert source.selection == SourceSelection(frame=1)
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
            assert intersection.batches[0].sources[0].selection == actual
            conversion = await runtime.results.get(intersection.completed_result_refs[0])
            assert conversion.inputs[0].selection == actual
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
