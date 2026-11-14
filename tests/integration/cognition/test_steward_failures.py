"""Saved evidence survives optional and required reading arrangement failures."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path

import pytest

from nous.application.configuration import open_configuration_operations
from nous.application.contracts import EventEnvelope, Progress, Saved
from nous.cognition.contracts import CreateNamespace
from nous.cognition.material_contracts import ReadMaterial
from nous.cognition.materials import READ_MATERIAL
from nous.cognition.retrieval.contracts import (
    DeliveryRequest,
    DeliveryUsage,
    QueryControls,
    QueryRequest,
    SaveEvidence,
    Selector,
)
from nous.cognition.retrieval.operations import QUERY, SAVE_EVIDENCE
from nous.cognition.scope import CREATE_NAMESPACE
from nous.cognition.steward.contracts import ArrangeReading
from nous.cognition.steward.operations import ARRANGE
from nous.cognition.writing.contracts import AddNote, AfterWrite
from nous.cognition.writing.operations import ADD_NOTE
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection, InlineText, ResultRef, SourceSelection
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave.contracts import DeliveredText, DeliveryBudget
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
            if len(self.requests) in (2, 4):
                reading["context_requests"] = [{"key": "E1", "direction": "after"}]
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
                await runtime.app.call(
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
                assert "include_review" in sent

                body = found.evidence.materials[0].material.body
                assert body is not None
                first_text = "The worker exits first."
                first = ContentSelection(
                    artifact=body.artifact,
                    selection=SourceSelection(start_byte=0, end_byte=len(first_text)),
                )
                passage = found.evidence.passages[0].model_copy(
                    update={
                        "key": "E1",
                        "fragments": (DeliveredText(content=first, text=first_text),),
                        "support_context": (),
                        "scored_ranges": (first,),
                        "shared_context": (),
                    }
                )
                material = found.evidence.materials[0].model_copy(update={"passage_keys": ("E1",)})
                evidence = found.evidence.model_copy(
                    update={
                        "materials": (material,),
                        "passages": (passage,),
                        "program_run": None,
                        "budget": DeliveryBudget(amount=200, unit="bytes"),
                        "delivery_usage": DeliveryUsage(unit="bytes", amount=len(first_text)),
                    }
                )
                original = await runtime.app.call(SAVE_EVIDENCE, SaveEvidence(evidence=evidence))
                expanded = await runtime.app.call(ARRANGE, ArrangeReading(evidence=original))
                assert expanded.evidence_ref != original and len(Handler.requests) == 3
                read = await runtime.app.call(
                    READ_MATERIAL, ReadMaterial(delivery=expanded.evidence_ref, key="E1")
                )
                assert "Only then may the handle be released" in "".join(
                    item.body_text or "" for item in read.selections
                )
                assert original in (await runtime.results.get(expanded.evidence_ref)).dependencies
                assert (
                    expanded.evidence_ref
                    in (await runtime.results.get(expanded.reading_ref)).dependencies
                )

                limited = evidence.model_copy(
                    update={"budget": DeliveryBudget(amount=len(first_text), unit="bytes")}
                )
                original = await runtime.app.call(SAVE_EVIDENCE, SaveEvidence(evidence=limited))
                constrained = await runtime.app.call(ARRANGE, ArrangeReading(evidence=original))
                assert constrained.evidence_ref == original and len(Handler.requests) == 4
                assert constrained.issues[0].code == "steward.budget_exhausted"

        asyncio.run(scenario())
