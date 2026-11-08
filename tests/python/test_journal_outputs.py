"""Explicit no-material output preserves computation without creating written content."""

import asyncio
from pathlib import Path

import pytest
import tomlkit

from nous.cognition.writing.contracts import DraftOverride
from nous.common.errors import NousError
from nous.models.contracts import ReadingInput
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


def test_no_material_and_draft_corrections_use_real_writing_path(tmp_path: Path) -> None:
    import json
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    from nous.application.configuration import open_configuration_operations

    answer = ""

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            self.rfile.read(int(self.headers["Content-Length"]))
            payload = json.dumps(
                {
                    "id": "writing",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": answer},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    async def scenario() -> None:
        nonlocal answer
        instance = tmp_path / "journal-output"
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update(
                {
                    "providers.local": {
                        "protocol": "openai-chat-completions",
                        "endpoint": f"http://127.0.0.1:{server.server_port}/v1",
                    },
                    "models.reader": {
                        "provider": "local",
                        "model_name": "local",
                        "capabilities": ["text"],
                        "output_modes": ["text"],
                    },
                    "tasks.journal-draft": {
                        "model": "reader",
                        "prompt": "journal-draft",
                        "output_mode": "text",
                    },
                }
            )
        async with open_instance(instance, environment={}) as runtime:
            c = await runtime.cognition("default")
            note = (await c.records.add_note("你好。")).record
            answer = "\n !NO_MATERIAL!\t"
            outcome = await c.writing.journalize([note.revision_ref], adoption="draft")
            assert outcome.status == "no_material" and outcome.draft is None
            result = await runtime.results.get(outcome.stage_results[-1])
            output = next(item.artifact for item in result.outputs if item.name == "output")
            assert await runtime.artifacts.read_text(output) == answer
            assert await c.writing.drafts.status() == {"drafts": 0, "unadopted": 0}
            assert await c.records.list(kind="journal") == ()

            for invalid_answer in ("", " \n\t"):
                answer = invalid_answer
                with pytest.raises(NousError) as failed:
                    await c.writing.journalize([note.revision_ref], adoption="draft")
                assert failed.value.code == "model.empty_output"

            answer = "# 标记说明\n\n!NO_MATERIAL! 出现在正常正文中。"
            drafted = await c.writing.journalize([note.revision_ref], adoption="draft")
            assert drafted.draft is not None and drafted.draft.body == answer
            saved = await c.records.adopt(drafted.draft.ref)
            assert (await c.records.read(saved.record.revision_ref)).body == answer

            answer = "# 引用待修\n\n[材料](source:missing)"
            invalid = await c.writing.journalize([note.revision_ref], adoption="draft")
            assert invalid.draft is not None
            with pytest.raises(NousError) as failed:
                await c.records.adopt(invalid.draft.ref)
            assert failed.value.code == "cognition.source_unknown"
            source_key = invalid.draft.context.source_bindings[0].key
            corrected = await c.records.adopt(
                invalid.draft.ref,
                override=DraftOverride(markdown=f"# 引用已修\n\n[材料](source:{source_key})"),
            )
            retried = await c.records.adopt(invalid.draft.ref)
            assert retried.record.revision_ref == corrected.record.revision_ref

    try:
        asyncio.run(scenario())
    finally:
        server.shutdown()
        thread.join()
        server.server_close()


def test_task_contract_ignores_nested_schema_help_but_keeps_business_fields() -> None:
    from pydantic import Field, create_model

    from nous.content.contracts import SelectedInputs
    from nous.models.generation import GenerationRequest
    from nous.models.tasks import TaskDefinition

    def task(help_text: str, *, min_length: int = 1) -> TaskDefinition:
        nested = create_model(
            "Nested",
            title=(str, Field(..., description=help_text, min_length=min_length)),
            description=(str, Field(..., description=help_text)),
            examples=(list[str], Field(..., description=help_text)),
        )
        output = create_model("Output", child=(nested, Field(..., description=help_text)))
        return TaskDefinition("nested-contract", output_type=output)

    before = task("Before")
    help_only = task("After")
    changed = task("After", min_length=2)
    assert before.interpretation_contract() == help_only.interpretation_contract()
    assert help_only.output_type is not None
    assert before.supports_schema(help_only.output_type.model_json_schema())
    assert before.interpretation_contract() != changed.interpretation_contract()
    assert before.output_type is not None and changed.output_type is not None
    original_request = GenerationRequest(
        "custom", SelectedInputs(sources=()), schema=before.output_type.model_json_schema()
    )
    help_request = GenerationRequest(
        "custom", SelectedInputs(sources=()), schema=help_only.output_type.model_json_schema()
    )
    changed_request = GenerationRequest(
        "custom", SelectedInputs(sources=()), schema=changed.output_type.model_json_schema()
    )
    assert original_request.identity() != help_request.identity()
    assert original_request.identity() != changed_request.identity()
    schema = before.interpretation_contract()["output_schema"]
    assert isinstance(schema, dict)
    definitions = schema["$defs"]
    assert isinstance(definitions, dict)
    nested_schema = definitions["Nested"]
    assert isinstance(nested_schema, dict)
    properties = nested_schema["properties"]
    assert isinstance(properties, dict)
    assert set(properties) == {"title", "description", "examples"}
