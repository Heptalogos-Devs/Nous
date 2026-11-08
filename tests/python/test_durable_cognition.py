"""Regression coverage for stable identity and durable annotation adoption."""

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
import sqlalchemy as sa

from nous.application.configuration import open_configuration_operations
from nous.cognition import AfterWrite, concept_schema
from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.common.errors import NousError
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_bound_namespace_and_target_preconditions(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "identity"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            await runtime.namespaces.create("research")
            bound = await runtime.cognition("research")
            await runtime.namespaces.rename("research", "science")
            await runtime.namespaces.create("research")
            first = (await bound.records.add_note("first", remember=True)).record
            other = (await bound.records.add_note("other", remember=True)).record
            for action in (
                lambda: bound.records.replace(first.ref, "changed", expected=other.revision_ref),
                lambda: bound.records.archive(first.ref, expected=other.revision_ref),
                lambda: bound.records.restore(first.ref, expected=other.revision_ref),
                lambda: bound.records.set_recall(first.ref, False, expected=other.revision_ref),
            ):
                with pytest.raises(NousError, match="another record") as error:
                    await action()
                assert error.value.code == "cognition.target_mismatch"
            assert (await bound.records.read(first.ref)).body == "first"
            assert (await bound.records.read(other.ref)).body == "other"
            excerpt = await bound.records.read(first.ref, blocks=("B1",))
            assert excerpt.selection.revision == first.revision_ref
            assert (
                await runtime.artifacts.read_selection(excerpt.selection.selections[0])
                == excerpt.body
            )
            with pytest.raises(NousError) as error:
                await bound.concepts.find(namespace="research")
            assert error.value.code == "cognition.namespace_mismatch"
            with pytest.raises(NousError) as error:
                await bound.retrieval.query(CognitiveQuery(namespace="research", mode="lexical"))
            assert error.value.code == "cognition.namespace_mismatch"
            updated = await bound.records.replace(first.ref, "changed", expected=1)
            assert updated.record.revision_ref.ordinal == 2
            assert updated.record.namespace.ref == bound.namespace

    asyncio.run(scenario())


def test_manual_records_survive_unavailable_background_annotation(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "manual-without-model"
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update(
                {
                    "cognition.annotation.on_save": True,
                    "cognition.maintenance.execution": "background",
                }
            )
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            private = await cognition.records.add_note("A private note.", remember=False)
            assert private.status == "success" and not private.jobs
            updated = await cognition.records.replace(
                private.record.revision_ref, "A revised note."
            )
            assert updated.status == "success" and not updated.jobs
            unchanged = await cognition.records.replace(
                updated.record.revision_ref, "A revised note."
            )
            assert unchanged.status == "success" and not unchanged.record.changed
            requested = await cognition.records.replace(
                updated.record.revision_ref,
                "An explicit annotation is requested.",
                after=AfterWrite(annotation="run", execution="background"),
            )
            assert requested.status == "partial" and requested.annotation.state == "failed"
            assert (await cognition.records.read(requested.record.revision_ref)).body.startswith(
                "An explicit annotation"
            )
            recalled = await cognition.records.add_note("A remembered garden.", remember=True)
            assert recalled.status == "partial" and recalled.annotation.state == "failed"
            answer = await cognition.retrieval.query(
                CognitiveQuery(text="garden", mode="lexical", top_k=5)
            )
            assert any(
                group.revision_ref == recalled.record.revision_ref for group in answer.groups
            )

    asyncio.run(scenario())


def test_annotation_adoption_replay_and_refresh_are_atomic(tmp_path: Path) -> None:
    from pydantic import JsonValue

    from nous.cognition.annotation.contracts import AnnotationPlan
    from nous.cognition.maintenance.contracts import MaintenanceOutcome, RefreshRequest
    from nous.content.annotations import AnnotationProposal
    from nous.content.contracts import ResultRef
    from nous.jobs.contracts import JobRef
    from nous.runtime.bootstrap import Runtime
    from nous.runtime.host import run_host
    from nous.storage.database import WriteTx

    requests: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(request)
            answer = json.dumps(
                {
                    "concepts": [
                        {"key": "c", "label": "Durability", "description": "State survives restart"}
                    ],
                    "occurrences": [{"key": "o", "concept": "c", "blocks": ["B1"]}],
                    "paths": [],
                }
            )
            body = json.dumps(
                {
                    "id": "answer",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": request["model"],
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
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    instance = tmp_path / "annotation"

    async def setup() -> None:
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update(
                {
                    "jobs.poll_interval_seconds": 0.1,
                    "providers.local": {
                        "protocol": "openai-chat-completions",
                        "endpoint": f"http://127.0.0.1:{server.server_port}/v1",
                    },
                    "models.reader": {
                        "provider": "local",
                        "model_name": "controlled",
                        "capabilities": ["text", "structured_output"],
                        "output_modes": ["text", "native"],
                    },
                    "tasks.annotate-concepts": {
                        "model": "reader",
                        "prompt": "annotate-concepts",
                        "output_mode": "native",
                    },
                }
            )

    async def application(runtime: Runtime) -> None:
        cognition = await runtime.cognition()
        adopt = runtime.maintenance.adopt_annotation
        fail_after_commit = True

        async def interrupted(
            plan: AnnotationPlan, proposal: ResultRef, refresh: RefreshRequest | None
        ) -> MaintenanceOutcome:
            nonlocal fail_after_commit
            result = await adopt(plan, proposal, refresh)
            if fail_after_commit:
                fail_after_commit = False
                raise NousError("test.after_commit", "Failure after adoption")
            return result

        runtime.maintenance.adopt_annotation = interrupted
        saved = await cognition.records.create_journal(
            "A durable annotation and its retrieval refresh.",
            after=AfterWrite(
                annotation="run",
                retrieval="refresh",
                retrieval_mode="lexical",
                execution="background",
            ),
        )
        original = saved.jobs[0]
        from unittest.mock import patch

        with patch.object(runtime.jobs, "list", side_effect=AssertionError("history scan")):
            registered = await runtime.maintenance.record_annotation_job(saved.record.revision_ref)
        assert registered is not None and registered.ref == original
        with pytest.raises(NousError):
            await runtime.jobs.wait(original, timeout=30)
        failed = await runtime.jobs.read(original)
        assert failed.status == "ERROR" and failed.failed_step is not None
        generation = (await runtime.namespaces.read(cognition.namespace)).content_generation
        async with runtime.database.read() as tx:
            count = await tx.connection.scalar(
                sa.select(sa.func.count()).select_from(concept_schema.annotations)
            )
            concepts = await tx.connection.scalar(
                sa.select(sa.func.count()).select_from(concept_schema.concepts)
            )
        retry, duplicate = await asyncio.gather(
            runtime.jobs.retry_failed(original, failed.failed_step),
            runtime.jobs.retry_failed(original, failed.failed_step),
        )
        assert retry == duplicate
        outcome = MaintenanceOutcome.model_validate(await runtime.jobs.wait(retry, timeout=30))
        assert outcome.state == "ready" and outcome.annotation_ref and outcome.proposal_result
        assert len(outcome.related_jobs) == 1
        assert (await runtime.jobs.wait(outcome.related_jobs[0], timeout=30))["state"] == "ready"
        async with runtime.database.read() as tx:
            assert (
                await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(concept_schema.annotations)
                )
                == count
                == 1
            )
            assert (
                await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(concept_schema.concepts)
                )
                == concepts
                == 1
            )
        assert (await runtime.namespaces.read(cognition.namespace)).content_generation == generation
        assert len(requests) == 1

        note = (
            await cognition.records.add_note(
                "A manual concept in a recalled note.",
                remember=True,
                after=AfterWrite(annotation="skip", execution="inline"),
            )
        ).record
        proposal = await cognition.annotations.propose(
            note.revision_ref,
            proposal=AnnotationProposal.model_validate(
                {
                    "concepts": [{"key": "m", "label": "Manual", "description": "Manual concept"}],
                    "occurrences": [{"key": "o", "concept": "m", "blocks": ["B1"]}],
                    "paths": [],
                }
            ),
        )
        inactive = await cognition.annotations.prepare_adoption(
            note.revision_ref, proposal.result_ref, activate=False
        )
        async with runtime.database.write() as tx:
            first = await cognition.annotations.adopt_in(tx, inactive)
        assert first.activation == "inactive"
        plan = await runtime.maintenance.prepare_after(
            AfterWrite(
                annotation="skip",
                retrieval="refresh",
                retrieval_mode="lexical",
                execution="background",
            ),
            in_recall=True,
        )
        assert plan.refresh is not None
        prepared = await cognition.annotations.prepare_adoption(
            note.revision_ref, proposal.result_ref
        )
        generation = (await runtime.namespaces.read(cognition.namespace)).content_generation
        jobs_before, _ = await runtime.jobs.list()
        with pytest.raises(RuntimeError, match="rollback"):
            async with runtime.database.write() as tx:
                adopted = await cognition.annotations.adopt_in(tx, prepared)
                assert adopted.current_view_changed
                await runtime.maintenance.enqueue_refresh_in(
                    tx, adopted.namespace, adopted.generation, plan.refresh, key="atomic-regression"
                )
                raise RuntimeError("rollback")
        assert not (await cognition.annotations.read(first.annotation)).active
        assert (await runtime.namespaces.read(cognition.namespace)).content_generation == generation
        jobs_after, _ = await runtime.jobs.list()
        assert len(jobs_after) == len(jobs_before)
        async with runtime.database.write() as tx:
            adopted = await cognition.annotations.adopt_in(tx, prepared)
            queued = await runtime.maintenance.enqueue_refresh_in(
                tx, adopted.namespace, adopted.generation, plan.refresh, key="atomic-regression"
            )
        assert adopted.annotation == first.annotation and adopted.current_view_changed
        assert (await runtime.jobs.wait(queued, timeout=30))["state"] == "ready"
        async with runtime.database.write() as tx:
            replayed = await cognition.annotations.adopt_in(tx, prepared)
        assert replayed.annotation == first.annotation and not replayed.current_view_changed
        assert (
            await runtime.namespaces.read(cognition.namespace)
        ).content_generation == generation + 1

        enqueue = runtime.jobs.enqueue_in

        async def fail_enqueue(
            tx: WriteTx, *, kind: str, workflow_id: str, payload: dict[str, JsonValue]
        ) -> JobRef:
            raise NousError("storage.busy", "Queue registration failed")

        before = await cognition.records.list(kind="journal")
        runtime.jobs.enqueue_in = fail_enqueue
        try:
            with pytest.raises(NousError, match="Queue registration failed"):
                await cognition.records.create_journal(
                    "This write must roll back with its requested refresh.",
                    after=AfterWrite(
                        annotation="skip",
                        retrieval="refresh",
                        retrieval_mode="lexical",
                        execution="background",
                    ),
                )
        finally:
            runtime.jobs.enqueue_in = enqueue
        assert await cognition.records.list(kind="journal") == before

    try:
        asyncio.run(setup())
        run_host(instance, application, environment={})
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
