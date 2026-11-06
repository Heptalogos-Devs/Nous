"""Explicit no-material output preserves computation without creating written content."""

import asyncio
from pathlib import Path

import pytest
import tomlkit

from nous.cognition.writing.contracts import DraftContext, DraftOverride
from nous.cognition.writing.tasks import JournalDraftTask
from nous.common.errors import NousError
from nous.content.contracts import InputGroup, InputPart, OutputContent
from nous.models.contracts import ReadingInput
from nous.models.operations import ProducedTask
from nous.models.prompts import PromptCatalog, PromptCompiler
from nous.runtime.bootstrap import initialize_instance, open_instance, task_catalog


def test_prompt_editing_notes_are_visible_without_changing_the_request(tmp_path: Path) -> None:
    catalog = PromptCatalog(tmp_path / "prompts", task_catalog())
    compiler = PromptCompiler(catalog)
    original = catalog.get("journal-draft")
    request = compiler.compile(original, ReadingInput(question="A writing topic"))
    directory = catalog.editable_directory("journal-draft")
    manifest = directory / "prompt.toml"
    document = tomlkit.parse(manifest.read_text(encoding="utf-8"))
    document["documentation"]["purpose"] = "A clearer description for the editor."
    manifest.write_text(tomlkit.dumps(document), encoding="utf-8")
    revised = catalog.get("journal-draft")
    assert revised.execution_signature == original.execution_signature
    assert revised.documentation_signature != original.documentation_signature
    assert (
        compiler.compile(revised, ReadingInput(question="A writing topic")).preview
        == request.preview
    )
    context = catalog.write_context("journal-draft").read_text(encoding="utf-8")
    assert "A clearer description for the editor." in context
    assert "S1:C1" in context and "output-schema.json" in context


def test_no_material_preserves_result_and_rejects_empty_output(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "journal-output"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            c = await runtime.cognition("default")
            note = (await c.records.add_note("你好。")).record
            sources = await c.records.select_sources([note.revision_ref])
            context = DraftContext(
                namespace=note.namespace,
                intent="new",
                selected_sources=sources,
                source_bindings=sources,
                times=(await c.records.read(note.revision_ref)).times,
                config_version=runtime.configuration.version,
                task="journal-draft",
            )

            def produced(text: str) -> ProducedTask:
                return ProducedTask(
                    operation="journal-draft",
                    producer={"kind": "contract-test"},
                    actual_inputs=(
                        InputGroup(
                            key="S1",
                            slot_key="S1",
                            name="问候",
                            parts=tuple(
                                InputPart(content=selection) for selection in sources[0].selections
                            ),
                        ),
                    ),
                    outputs={"output": OutputContent(data=text.encode())},
                    request={},
                    parameters={},
                    coverage={},
                    completed_stage_refs=(),
                )

            raw_marker = "\n !NO_MATERIAL!\t"
            outcome = await c.writing.drafts.save_produced(produced(raw_marker), context)
            assert outcome.status == "no_material" and outcome.draft is None
            result = await runtime.results.get(outcome.stage_results[-1])
            output = next(item.artifact for item in result.outputs if item.name == "output")
            assert await runtime.artifacts.read_text(output) == raw_marker
            assert await c.writing.drafts.status() == {"drafts": 0, "unadopted": 0}
            assert await c.records.list(kind="journal") == ()

            for empty in ("", " \n\t"):
                with pytest.raises(NousError, match="no task output") as failed:
                    await c.writing.drafts.save_produced(produced(empty), context)
                assert failed.value.code == "model.empty_output"
            assert len(await runtime.results.recent()) == 1

            body = "# 标记说明\n\n!NO_MATERIAL! 出现在正常正文中。"
            assert JournalDraftTask.body(OutputContent(data=b"NO_MATERIAL")) == "NO_MATERIAL"
            drafted = await c.writing.drafts.save_produced(produced(body), context)
            assert drafted.status == "draft" and drafted.draft is not None
            assert drafted.draft.body == body
            saved = await c.records.adopt(drafted.draft.ref)
            assert (await c.records.read(saved.record.revision_ref)).body == body

            invalid = await c.writing.drafts.save_produced(
                produced("# 引用待修\n\n[材料](source:missing)"), context
            )
            assert invalid.draft is not None
            with pytest.raises(NousError) as failed:
                await c.records.adopt(invalid.draft.ref)
            assert failed.value.code == "cognition.source_unknown"
            assert (await c.writing.read_draft(invalid.draft.ref)).body == invalid.draft.body

            unqualified = await c.writing.drafts.save_produced(
                produced("# 引用待修\n\n[材料](S1)"), context
            )
            assert unqualified.draft is not None
            with pytest.raises(NousError) as failed:
                await c.records.adopt(unqualified.draft.ref)
            assert failed.value.code == "cognition.source_link_unqualified"
            assert (
                await c.writing.read_draft(unqualified.draft.ref)
            ).body == unqualified.draft.body
            assert len(await c.records.list(kind="journal")) == 1
            corrected = await c.records.adopt(
                unqualified.draft.ref,
                override=DraftOverride(markdown="# 引用已修\n\n[材料](source:S1)"),
            )
            retried = await c.records.adopt(unqualified.draft.ref)
            assert retried.record.revision_ref == corrected.record.revision_ref
            assert (
                await c.writing.read_draft(unqualified.draft.ref)
            ).body == unqualified.draft.body

    asyncio.run(scenario())
