"""Saved evidence survives optional and required reading arrangement failures."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path

import pytest

from nous.application.configuration import open_configuration_operations
from nous.application.contracts import EventEnvelope, Progress, Saved
from nous.cognition.contracts import CreateNamespace
from nous.cognition.material_contracts import MaterialSelection, ReadMaterial
from nous.cognition.materials import READ_MATERIAL
from nous.cognition.retrieval.contracts import (
    DeliveryRequest,
    QueryControls,
    QueryRequest,
    Selector,
)
from nous.cognition.retrieval.operations import QUERY
from nous.cognition.scope import CREATE_NAMESPACE
from nous.cognition.steward.contracts import ArrangeReading
from nous.cognition.steward.operations import ARRANGE
from nous.cognition.writing.contracts import AddNote, AfterWrite
from nous.cognition.writing.operations import ADD_NOTE
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection, InlineText, ResultRef, SourceSelection
from nous.runtime.bootstrap import initialize_instance, open_instance
from tests.integration.shared.models import configure_models, model_server


def test_unconfigured_steward_preserves_evidence(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(
            instance, environment={}, overrides={"cognition.search.steward": "optional"}
        ) as runtime:
            await runtime.app.call(
                ADD_NOTE,
                AddNote(
                    body=InlineText(
                        text="# Cancellation\n\nWait for workers to exit before releasing files.\n"
                    ),
                    remember=True,
                    after=AfterWrite(annotation="skip"),
                ),
            )
            optional = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="workers exit before releasing files",
                ),
            )
            assert optional.evidence_ref is not None
            assert optional.reading is None and optional.issues[0].code == "model.task_unconfigured"
            preserved = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(delivery=optional.evidence_ref, key="E1")
            )
            assert "workers" in (preserved.selections[0].body_text or "")
            disabled = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="workers exit before releasing files",
                    delivery=DeliveryRequest(steward="off"),
                ),
            )
            assert disabled.reading is None and not disabled.issues
            with pytest.raises(NousError) as failure:
                await runtime.app.call(
                    QUERY,
                    QueryRequest(
                        text="workers exit before releasing files",
                        delivery=DeliveryRequest(steward="required"),
                    ),
                )
            assert failure.value.code == "model.task_unconfigured"
            refs = failure.value.details["completed_refs"]
            assert isinstance(refs, list)
            evidence = next(
                ResultRef.model_validate(item)
                for item in refs
                if isinstance(item, dict) and item.get("kind") == "result"
            )
            preserved = await runtime.app.call(
                READ_MATERIAL, ReadMaterial(delivery=evidence, key="E1")
            )
            assert "workers" in (preserved.selections[0].body_text or "")

    asyncio.run(scenario())


def test_evidence_key_obeys_delivery_namespace_for_shared_artifact(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            other = await runtime.app.call(CREATE_NAMESPACE, CreateNamespace(name="other"))
            artifact = await runtime.artifacts.ingest("Shared original text.")
            result = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="",
                    save=True,
                    controls=QueryControls(
                        selectors=(Selector(kind="ref", value=artifact.ref.identifier),)
                    ),
                    delivery=DeliveryRequest(steward="off"),
                ),
            )
            assert result.evidence_ref is not None
            assert (
                await runtime.app.call(
                    READ_MATERIAL, ReadMaterial(delivery=result.evidence_ref, key="E1")
                )
            ).selections[0].body_text == "Shared original text."
            with pytest.raises(NousError) as failure:
                await runtime.app.call(
                    READ_MATERIAL,
                    ReadMaterial(delivery=result.evidence_ref, key="E1", namespace=other.ref),
                )
            assert failure.value.code == "cognition.namespace_mismatch"

    asyncio.run(scenario())


def test_independent_arrangement_reports_progress_and_committed_result(tmp_path: Path) -> None:
    class Handler(BaseHTTPRequestHandler):
        requests: list[dict[str, object]] = []

        def do_POST(self) -> None:
            self.requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            reading = {
                "title": "Reading",
                "items": [
                    {
                        "markdown": "The worker exits first.",
                        "evidence_keys": ["E1"],
                        "role": "finding",
                    }
                ],
            }
            body = json.dumps(
                {
                    "id": "reading",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": json.dumps(reading)},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "instance"
    with model_server(Handler) as server:
        configure_models(instance, server, "prompted")

        async def scenario() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "tasks.arrange-reading": {
                            "model": "reader",
                            "prompt": "arrange-reading",
                            "output_mode": "prompted",
                        }
                    }
                )
            async with open_instance(instance, environment={}) as runtime:
                note = await runtime.app.call(
                    ADD_NOTE,
                    AddNote(
                        body=InlineText(
                            text="The worker exits first.\n\nOnly then may the handle be released.\n"
                        ),
                        remember=True,
                        after=AfterWrite(annotation="skip"),
                    ),
                )
                found = await runtime.app.call(
                    QUERY,
                    QueryRequest(
                        text="worker exits first",
                        save=True,
                        aspects=("release condition",),
                        controls=QueryControls(include_review=True),
                        delivery=DeliveryRequest(steward="off"),
                    ),
                )
                assert found.evidence_ref is not None
                events: list[EventEnvelope] = []
                reading = await runtime.app.call(
                    ARRANGE,
                    ArrangeReading(evidence=found.evidence_ref, purpose="compare conditions"),
                    observer=events.append,
                )
                assert [
                    event.event.stage for event in events if isinstance(event.event, Progress)
                ] == ["arrange", "save_reading"]
                assert isinstance(events[-1].event, Saved)
                assert reading.reading_ref in events[-1].event.refs
                assert (await runtime.results.get(reading.reading_ref)).ref == reading.reading_ref
                sent = json.dumps(Handler.requests[0]["messages"], ensure_ascii=False)
                assert "release condition" in sent and "compare conditions" in sent
                assert "include_review" not in sent
                assert len(Handler.requests) == 1
                saved_reading = await runtime.results.get(reading.reading_ref)
                saved_evidence = await runtime.results.get(found.evidence_ref)
                assert found.evidence_ref in saved_reading.dependencies
                assert saved_reading.sources_for("markdown") == saved_evidence.sources_for(
                    "evidence"
                )
                assert any(
                    item.output_name == "markdown" and item.origins
                    for item in saved_reading.mappings
                )
                overridden = await runtime.app.call(
                    ARRANGE,
                    ArrangeReading(
                        evidence=found.evidence_ref,
                        question="Which handle is affected?",
                        aspects=("new aspect",),
                        context=({"role": "user", "text": "Use the revised context."},),
                    ),
                )
                assert overridden.evidence_ref == found.evidence_ref
                override_prompt = json.dumps(Handler.requests[1]["messages"], ensure_ascii=False)
                assert "Which handle is affected?" in override_prompt
                assert (
                    "new aspect" in override_prompt and "release condition" not in override_prompt
                )
                assert "Use the revised context." in override_prompt
                view = await runtime.materials.resolve(note.commit.revision_ref)
                assert view.body is not None
                chosen = ContentSelection(
                    artifact=view.body.artifact,
                    selection=SourceSelection(
                        start_byte=0, end_byte=len("The worker exits first.")
                    ),
                )
                direct = await runtime.app.call(
                    ARRANGE,
                    ArrangeReading(
                        materials=(
                            MaterialSelection(
                                material=note.commit.revision_ref, selections=(chosen,)
                            ),
                        ),
                        question="What happens first?",
                    ),
                )
                assert direct.evidence_ref is None and len(Handler.requests) == 3
                saved_direct = await runtime.results.get(direct.reading_ref)
                source = saved_direct.sources_for("markdown")[0]
                assert source.key == "E1" and source.material == note.commit.revision_ref
                assert source.selections == (chosen,)
                assert any(chosen in item.origins for item in saved_direct.mappings)

        asyncio.run(scenario())
