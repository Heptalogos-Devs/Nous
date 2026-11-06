"""Regression coverage for stable identity and durable annotation adoption."""

import asyncio
import json
import threading
from collections.abc import Awaitable, Callable, Mapping
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Literal

import pytest
import sqlalchemy as sa
from pydantic import JsonValue

from nous.application.configuration import open_configuration_operations
from nous.cognition import AfterWrite, RevisionSelection, SourceRequest, concept_schema
from nous.cognition.concept_contracts import AnnotationView
from nous.cognition.concepts import ConceptTarget
from nous.cognition.contracts import Namespace
from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.cognition.writing.reading import Target
from nous.common.errors import NousError
from nous.content.contracts import ContentSelection, ResultRef, SourceSelection
from nous.content.projections import semantic_text
from nous.jobs.client import open_job_client
from nous.jobs.contracts import JobRef
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.storage.database import WriteTx


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


def test_annotation_fork_reuses_response_and_committed_adoption(tmp_path: Path) -> None:
    requests: list[dict[str, object]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(request)
            answer = (
                json.dumps({"concepts": [], "occurrences": [], "paths": []})
                if "response_format" in request
                else "# Conclusion\n\nThe selected evidence is retained."
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

    async def scenario() -> None:
        instance = tmp_path / "retry"
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as config:
            await config.update(
                {
                    "logging.level": "ERROR",
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
                    "tasks.journal-draft": {
                        "model": "reader",
                        "prompt": "journal-draft",
                        "output_mode": "text",
                    },
                }
            )
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            dependency = (await cognition.records.add_note("source")).record
            note = (
                await cognition.records.add_note(
                    "before [标签](source:S1) after\n",
                    source_requests=[SourceRequest(target=dependency.revision_ref)],
                )
            ).record
            view = await cognition.records.read(note.revision_ref)
            representation = await semantic_text(
                ContentSelection(artifact=view.body_artifact), runtime.results
            )
            selected = ContentSelection(
                artifact=representation.content.artifact,
                selection=SourceSelection(start_byte=0, end_byte=6),
            )
            revision_selection = RevisionSelection(
                revision=note.revision_ref, selections=(selected,)
            )
            binding = (
                await cognition.records.select_sources([revision_selection], usage="current_basis")
            )[0]
            assert binding.owner_revision == note.revision_ref
            assert await runtime.artifacts.read_selection(binding.selections[0]) == "before"
            partial_label = RevisionSelection(
                revision=note.revision_ref,
                selections=(
                    ContentSelection(
                        artifact=representation.content.artifact,
                        selection=SourceSelection(start_byte=7, end_byte=10),
                    ),
                ),
            )
            with pytest.raises(NousError) as unmapped:
                await cognition.records.select_sources([partial_label], usage="current_basis")
            assert unmapped.value.code == "content.origin_unmapped"
            draft = await cognition.writing.journalize(
                [SourceRequest(target=revision_selection, usage="current_basis")]
            )
            assert draft.draft is not None
            result = await runtime.results.get(draft.draft.result_ref)
            part = result.inputs[0].parts[0]
            assert part.content.artifact == selected.artifact
            assert any(
                origin.origin == binding.selections[0] and origin.transform_ref
                for origin in part.origins
            )
            assert draft.draft.context.source_bindings[0].selections == binding.selections
            saved = await cognition.records.create_journal(
                "One reproducible annotation.",
                after=AfterWrite(annotation="run", retrieval="refresh", execution="background"),
            )
            original, namespace = saved.jobs[0], cognition.namespace
        async with open_instance(instance, environment={}, workers=True) as runtime:
            adopt = runtime.annotations.adopt
            stage = runtime.maintenance.adopt_annotation
            fail_before, fail_after = True, True
            unchanged = object()

            async def before(
                target: Target,
                proposal_result: str | ResultRef,
                *,
                namespace: str | None = None,
                bindings: Mapping[str, ConceptTarget | None] | None = None,
                expected_active_annotation: str | None | object = unchanged,
                activate: bool = True,
                activate_for_current: bool = True,
                require_recall: bool = False,
                on_activation: Callable[[WriteTx, Namespace, int], Awaitable[None]] | None = None,
            ) -> AnnotationView:
                nonlocal fail_before
                if fail_before:
                    fail_before = False
                    raise NousError("test.before", "Failure before annotation adoption")
                if expected_active_annotation is unchanged:
                    return await adopt(
                        target,
                        proposal_result,
                        namespace=namespace,
                        bindings=bindings,
                        activate=activate,
                        activate_for_current=activate_for_current,
                        require_recall=require_recall,
                        on_activation=on_activation,
                    )
                return await adopt(
                    target,
                    proposal_result,
                    namespace=namespace,
                    bindings=bindings,
                    activate=activate,
                    activate_for_current=activate_for_current,
                    require_recall=require_recall,
                    on_activation=on_activation,
                    expected_active_annotation=expected_active_annotation,
                )

            async def after(value: dict[str, JsonValue]) -> dict[str, JsonValue]:
                nonlocal fail_after
                result = await stage(value)
                if fail_after:
                    fail_after = False
                    raise NousError("test.after", "Failure after commit before checkpoint")
                return result

            runtime.annotations.adopt = before
            runtime.maintenance.adopt_annotation = after
            with pytest.raises(NousError, match="Failure before annotation adoption"):
                await runtime.jobs.wait(original, timeout=60)
            first = await runtime.jobs.read(original)
            assert first.status == "ERROR" and first.failed_step is not None
            queue = first.queue_name
            retry = await runtime.jobs.retry_failed(original, first.failed_step)
            with pytest.raises(NousError, match="Failure after commit before checkpoint"):
                await runtime.jobs.wait(retry, timeout=60)
            second = await runtime.jobs.read(retry)
            assert second.status == "ERROR" and second.failed_step is not None
            generation = (await runtime.namespaces.read(namespace)).content_generation
            async with runtime.database.read() as tx:
                count = await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(concept_schema.annotations)
                )
            resumed = await runtime.jobs.retry_failed(retry, second.failed_step)
            outcome = await runtime.jobs.wait(resumed, timeout=60)
            assert outcome.state == "ready" and len(outcome.related_jobs) == 1
            assert outcome.proposal_result and outcome.annotation_ref
            assert outcome.activation_state == "active"
            assert (await runtime.jobs.wait(outcome.related_jobs[0], timeout=60)).state == "ready"
            async with runtime.database.read() as tx:
                repeated = await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(concept_schema.annotations)
                )
            assert count == repeated == 1
            assert (await runtime.namespaces.read(namespace)).content_generation == generation
            assert len(requests) == 2
            assert sum("response_format" in request for request in requests) == 1
            assert (await runtime.jobs.read(original)).status == "ERROR"
            assert (await runtime.jobs.read(retry)).status == "ERROR"
            resumed_status = await runtime.jobs.read(resumed)
            assert resumed_status.queue_name == queue

            cognition = await runtime.cognition()
            generation = (await runtime.namespaces.read(namespace)).content_generation
            background = await cognition.records.add_note(
                "An explicitly annotated private note.",
                remember=False,
                after=AfterWrite(annotation="run", execution="background"),
            )
            assert len(background.jobs) == 1
            assert background.annotation.job == background.jobs[0]
            assert background.retrieval.state == "not_requested"
            background_result = await runtime.jobs.wait(background.jobs[0], timeout=60)
            assert background_result.state == "ready"
            assert background_result.activation_state == "active"
            assert not background_result.related_jobs
            inline = await cognition.records.add_note(
                "Another explicitly annotated private note.",
                remember=False,
                after=AfterWrite(annotation="run", execution="inline"),
            )
            assert inline.annotation.state == "ready"
            assert (await runtime.namespaces.read(namespace)).content_generation == generation

            await runtime.configuration.update({"cognition.annotation.on_save": True})
            automatic_private = await cognition.records.add_note(
                "An ordinary private note.",
                remember=False,
                after=AfterWrite(execution="background"),
            )
            assert not automatic_private.jobs
            assert automatic_private.annotation.state == "not_requested"
            entered, release = asyncio.Event(), asyncio.Event()
            generate = runtime.maintenance.generate_annotation

            async def pause_before_adoption(plan: dict[str, JsonValue]) -> dict[str, JsonValue]:
                result = await generate(plan)
                entered.set()
                await release.wait()
                return result

            runtime.maintenance.generate_annotation = pause_before_adoption
            automatic = await cognition.records.add_note(
                "A recalled note that becomes private during annotation.",
                remember=True,
                after=AfterWrite(execution="background"),
            )
            assert automatic.annotation.job == automatic.jobs[0]
            try:
                await asyncio.wait_for(entered.wait(), timeout=60)
                await cognition.records.set_recall(
                    automatic.record.revision_ref,
                    False,
                    after=AfterWrite(annotation="skip", execution="inline"),
                )
                generation = (await runtime.namespaces.read(namespace)).content_generation
            finally:
                release.set()
            superseded = await runtime.jobs.wait(automatic.jobs[0], timeout=60)
            assert superseded.state == "superseded"
            assert superseded.annotation_ref is not None
            assert superseded.activation_state == "inactive"
            assert not superseded.related_jobs
            assert (await runtime.namespaces.read(namespace)).content_generation == generation

            attempts = 0

            async def fail_twice(plan: dict[str, JsonValue]) -> dict[str, JsonValue]:
                nonlocal attempts
                attempts += 1
                if attempts <= 2:
                    raise NousError("test.generate", "Temporary annotation failure")
                return await generate(plan)

            runtime.maintenance.generate_annotation = fail_twice
            count_before = len(requests)
            failed = await cognition.records.add_note(
                "A note with a retried annotation.",
                after=AfterWrite(annotation="run", execution="background"),
            )
            original = failed.jobs[0]
            with pytest.raises(NousError, match="Temporary annotation failure"):
                await runtime.jobs.wait(original, timeout=60)
            first = await runtime.jobs.read(original)
            assert first.failed_step is not None
            async with (
                open_job_client(instance, environment={}) as left,
                open_job_client(instance, environment={}) as right,
            ):
                first_retry, duplicate = await asyncio.gather(
                    left.retry_failed(original, first.failed_step),
                    right.retry_failed(original, first.failed_step),
                )
                assert first_retry == duplicate
                with pytest.raises(NousError, match="Temporary annotation failure"):
                    await runtime.jobs.wait(first_retry, timeout=60)
                assert await left.retry_failed(original, first.failed_step) == first_retry
                second = await runtime.jobs.read(first_retry)
                assert second.failed_step is not None
                second_retry = await right.retry_failed(first_retry, second.failed_step)
                assert second_retry != first_retry
                assert (await runtime.jobs.wait(second_retry, timeout=60)).state == "ready"
            assert attempts == 3 and len(requests) == count_before + 1

            enqueue = runtime.jobs.enqueue_in

            async def fail_enqueue(
                tx: WriteTx,
                *,
                kind: Literal["annotation", "refresh"],
                workflow_id: str,
                payload: dict[str, JsonValue],
            ) -> JobRef:
                raise NousError("storage.busy", "Queue registration failed")

            notes_before = await cognition.records.list(kind="note")
            runtime.jobs.enqueue_in = fail_enqueue
            try:
                with pytest.raises(NousError, match="Queue registration failed"):
                    await cognition.records.add_note(
                        "A record whose work could not be enqueued.",
                        remember=True,
                        after=AfterWrite(annotation="run", execution="background"),
                    )
            finally:
                runtime.jobs.enqueue_in = enqueue
            assert await cognition.records.list(kind="note") == notes_before

        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            assert first.failed_step is not None
            assert await runtime.jobs.retry_failed(original, first.failed_step) == first_retry
            record = await cognition.records.add_note("A note for explicit maintenance.")
            count_before = len(requests)
            earlier = await runtime.maintenance.request_annotation(
                record.record.revision_ref, namespace=cognition.namespace.id
            )
            prompt_dir = runtime.prompts.editable_directory("annotate-concepts")
            prompt = prompt_dir / "system.md.j2"
            prompt.write_text(
                prompt.read_text(encoding="utf-8") + "\n\n<!-- revised task instructions -->\n",
                encoding="utf-8",
            )
            later = await runtime.maintenance.request_annotation(
                record.record.revision_ref,
                namespace=cognition.namespace.id,
                task="annotate-concepts",
            )
            stale = await cognition.records.add_note("A note that will change while queued.")
            stale_job = await runtime.maintenance.request_annotation(
                stale.record.revision_ref, namespace=cognition.namespace.id
            )
            await cognition.records.replace(
                stale.record.revision_ref,
                "The note has changed before its annotation starts.",
                after=AfterWrite(annotation="skip", execution="inline"),
            )
            assert earlier != later
            assert (await runtime.jobs.read(earlier)).status == "ENQUEUED"
            assert (await runtime.jobs.read(later)).status == "ENQUEUED"
            assert len(await cognition.records.history(record.record.ref)) == 1
        async with open_instance(instance, environment={}, workers=True) as runtime:
            first, second, skipped = await asyncio.gather(
                runtime.jobs.wait(earlier, timeout=60),
                runtime.jobs.wait(later, timeout=60),
                runtime.jobs.wait(stale_job, timeout=60),
            )
            assert first.proposal_result and second.proposal_result
            assert first.state in {"ready", "superseded"}
            assert second.state in {"ready", "superseded"}
            assert skipped.state == "superseded" and skipped.proposal_result is None
            sent = [json.dumps(request) for request in requests[count_before:]]
            assert len(sent) == 2
            assert sum("revised task instructions" in request for request in sent) == 1

    try:
        asyncio.run(scenario())
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
