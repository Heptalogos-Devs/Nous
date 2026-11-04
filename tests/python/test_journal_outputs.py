"""Explicit no-material output preserves computation without creating written content."""

import asyncio
from pathlib import Path

import pytest

from nous.cognition.contracts import DraftContext
from nous.common.errors import NousError
from nous.content.contracts import InputGroup, InputPart, OutputContent
from nous.models.operations import ProducedTask
from nous.models.tasks import JournalDraftTask
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_no_material_preserves_result_and_rejects_empty_output(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "journal-output"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            note = await runtime.records.record_note("default", "你好。")
            sources = await runtime.records.select_sources("default", [note.revision_ref])
            context = DraftContext(
                namespace=note.namespace,
                selected_sources=sources,
                source_bindings=sources,
                times=await runtime.records.prepare_times(None),
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
                            name="问候",
                            parts=(InputPart(content=sources[0].content_selection),),
                        ),
                    ),
                    outputs={"output": OutputContent(data=text.encode())},
                    request={},
                    parameters={},
                    coverage={},
                    completed_stage_refs=(),
                )

            raw_marker = "\n !NO_MATERIAL!\t"
            outcome = await runtime.journalize.save_produced(produced(raw_marker), context)
            assert outcome.status == "no_material" and outcome.draft is None
            result = await runtime.results.get(outcome.stage_results[-1])
            output = next(item.artifact for item in result.outputs if item.name == "output")
            assert await runtime.artifacts.read_text(output) == raw_marker
            assert await runtime.journalize.status() == {"drafts": 0, "unadopted": 0}
            assert await runtime.records.list("default", kind="journal") == ()

            for empty in ("", " \n\t"):
                with pytest.raises(NousError, match="no task output") as failed:
                    await runtime.journalize.save_produced(produced(empty), context)
                assert failed.value.code == "model.empty_output"
            assert len(await runtime.results.recent()) == 1

            body = "# 标记说明\n\n!NO_MATERIAL! 出现在正常正文中。"
            assert JournalDraftTask.body(OutputContent(data=b"NO_MATERIAL")) == "NO_MATERIAL"
            drafted = await runtime.journalize.save_produced(produced(body), context)
            assert drafted.status == "draft" and drafted.draft is not None
            assert drafted.draft.body == body
            saved = await runtime.journalize.adopt_draft(drafted.draft.ref)
            assert (await runtime.records.read(saved.revision_ref)).body == body

    asyncio.run(scenario())
