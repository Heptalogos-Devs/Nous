"""Read exact materials under a shared budget without losing their identities."""

import asyncio
from pathlib import Path

import pytest

from nous.cognition.material_contracts import ReadMaterial
from nous.cognition.materials import READ_MATERIAL
from nous.cognition.writing.contracts import AddNote, AfterWrite
from nous.cognition.writing.operations import ADD_NOTE
from nous.content.contracts import (
    ContentSelection,
    DeliveryBudget,
    InlineText,
    MaterialSource,
    OutputContent,
    ProgramProducer,
    ResultFacts,
    ResultOutputRef,
    SourceSelection,
)
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_budgeted_read_preserves_overlapping_materials_and_unread_ranges(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(
            instance, environment={}, overrides={"content.text_read_max_bytes": 128}
        ) as runtime:
            related = await runtime.artifacts.ingest("相关内容。" * 100)
            source = MaterialSource(
                key="S1",
                material=related.ref,
                selections=(ContentSelection(artifact=related.ref),),
            )
            result = await runtime.results.adopt(
                facts=ResultFacts(
                    operation="test.material",
                    producer=ProgramProducer(),
                    material_sources=(source,),
                    formed_at=runtime.artifacts.clock.utc_now(),
                ),
                outputs={"output": OutputContent(data="甲。乙。".encode("utf-8"))},
            )
            artifact = result.outputs[0].artifact
            output = ResultOutputRef(result=result.ref, output="output")
            read = await runtime.materials.read_budgeted(
                (artifact, output), DeliveryBudget(amount=6, unit="bytes")
            )
            assert len(read.readings) == 2 and read.used == 6
            assert [item.material.ref for item in read.readings] == [artifact, output]
            assert read.readings[0].selections[0].body_text == "甲。"
            assert not read.readings[1].selections
            assert read.readings[1].shared == (read.readings[0].selections[0].source,)
            characters = await runtime.materials.read_budgeted(
                (artifact, output), DeliveryBudget(amount=2, unit="characters")
            )
            assert characters.used == 2
            assert characters.readings[0].selections[0].body_text == "甲。"
            assert characters.readings[1].shared == (characters.readings[0].selections[0].source,)
            assert read.readings[1].material.material_sources == (source,)
            assert read.readings[1].sources == {}
            expected = ContentSelection(
                artifact=artifact, selection=SourceSelection(start_byte=6, end_byte=12)
            )
            assert [item.selections for item in read.remaining] == [(expected,), (expected,)]
            complete = await runtime.materials.read_budgeted((artifact, output), None)
            assert complete.used == 4 and not complete.remaining
            assert complete.readings[0].selections[0].body_text == "甲。乙。"
            assert not complete.readings[1].selections
            assert complete.readings[1].shared == (complete.readings[0].selections[0].source,)

    asyncio.run(scenario())


def test_budgeted_read_uses_configured_tokenizer(tmp_path: Path) -> None:
    tokenizers = pytest.importorskip("tokenizers")
    tokenizer = tokenizers.Tokenizer(
        tokenizers.models.WordLevel({"[UNK]": 0, "Alpha": 1, ".": 2, "Beta": 3}, unk_token="[UNK]")
    )
    tokenizer.pre_tokenizer = tokenizers.pre_tokenizers.Whitespace()
    path = tmp_path / "tokenizer.json"
    tokenizer.save(str(path))

    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(
            instance, environment={}, overrides={"content.tokenizer_path": str(path)}
        ) as runtime:
            artifact = await runtime.artifacts.ingest("Alpha. Beta.")
            whitespace = await runtime.artifacts.ingest("   ")
            read = await runtime.materials.read_budgeted(
                (artifact.ref, whitespace.ref), DeliveryBudget(amount=2, unit="tokens")
            )
            assert read.used == 2 and read.remaining
            assert (read.readings[0].selections[0].body_text or "").startswith("Alpha.")
            assert "Beta" not in (read.readings[0].selections[0].body_text or "")
            assert read.readings[1].selections[0].body_text == "   "

    asyncio.run(scenario())


def test_material_identity_does_not_read_body_and_read_exposes_blocks(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "identity"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            long_note = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="long material " * 100),
                    after=AfterWrite(annotation="skip"),
                ),
            )
            short_note = await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(text="# Title\n\nOne paragraph.\n"),
                    after=AfterWrite(annotation="skip"),
                ),
            )
            artifact = await runtime.artifacts.ingest("standalone " * 100)
        async with open_instance(
            instance, environment={}, overrides={"content.text_read_max_bytes": 128}
        ) as runtime:
            ref = await runtime.materials.resolve_identifier(long_note.commit.ref.identifier)
            assert ref == long_note.commit.revision_ref
            views = await runtime.materials.resolve_many((ref, artifact.ref))
            assert [view.ref for view in views] == [ref, artifact.ref]
            assert all(view.sections is None for view in views)
            plain = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(ref=short_note.commit.revision_ref)
            )
            assert plain.material.sections is None
            full = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(ref=short_note.commit.revision_ref, structure=True)
            )
            assert full.material.sections
            block = full.material.sections[-1]
            selected = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(ref=short_note.commit.revision_ref, blocks=(block.key,))
            )
            assert selected.material.sections is None
            assert selected.selections[0].source == block.selection
            assert "One paragraph." in (selected.selections[0].body_text or "")

    asyncio.run(scenario())
