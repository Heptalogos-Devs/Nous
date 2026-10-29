"""Trace actual transactions and queued work without recording source content."""

import asyncio
import contextlib
import io
import json
import logging

import httpx
import pytest
from nous_wave.contracts import CallContext
from opentelemetry import baggage, context
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from opentelemetry.trace import StatusCode

from nous.platform.configuration import Settings
from nous.platform.observability import configure_logging, configure_tracing
from nous.transports.http import create_app


def test_http_transaction_and_durable_worker_share_trace_without_content(tmp_path):
    async def scenario():
        logs = io.StringIO()
        exporter = InMemorySpanExporter()
        provider = configure_tracing()
        provider.add_span_processor(SimpleSpanProcessor(exporter))
        private = "Private original note with a unique sensitive marker."
        with contextlib.redirect_stderr(logs):
            configure_logging("DEBUG")
            settings = Settings(instance=tmp_path / "instance")
            app = create_app(settings)
            async with app.router.lifespan_context(app):
                token = settings.token_file.read_text().strip()
                headers = {"Authorization": "Bearer " + token}
                async with httpx.AsyncClient(
                    transport=httpx.ASGITransport(app=app), base_url="http://local"
                ) as api:
                    namespace = await api.post(
                        "/api/namespaces", json={"name": "Traces"}, headers=headers
                    )
                    assert namespace.status_code == 200, namespace.text
                    ns = namespace.json()["id"]
                    subject = await api.post(
                        f"/api/namespaces/{ns}/subjects",
                        json={"name": "Durable"},
                        headers=headers,
                    )
                    assert subject.status_code == 200, subject.text
                    sub = subject.json()["id"]
                    base = f"/api/namespaces/{ns}/subjects/{sub}"
                    frozen = await api.post(
                        base + "/imports/catalogs",
                        json={"operation_id": "freeze", "inputs": [{"text": private}]},
                        headers=headers,
                    )
                    assert frozen.status_code == 200, frozen.text
                    # An unrelated ambient baggage value must never enter persisted work.
                    attached = context.attach(baggage.set_baggage("private", private))
                    try:
                        submitted = await api.post(
                            base + "/imports/process",
                            json={
                                "operation_id": "process",
                                "catalog_id": frozen.json()["id"],
                                "llm_enabled": False,
                                "vectors": "disabled",
                            },
                            headers=headers,
                        )
                    finally:
                        context.detach(attached)
                    assert submitted.status_code == 200, submitted.text
                    ctx = CallContext(
                        credential_source_ids=None,
                        caller_id="instance-admin",
                        namespace_id=ns,
                        subject_id=sub,
                    )
                    service = app.state.application
                    job_id = submitted.json()["job_id"]
                    await asyncio.wait_for(service.execution.wait(ctx, job_id), timeout=10)
                    status = await service.execution.status(ctx, job_id)
                    assert private not in json.dumps(status.attributes)
                    trace_id = submitted.headers["x-nous-trace-id"]
                    assert len(trace_id) == 32
                    assert submitted.headers["x-nous-request-id"]
                    # Failure must neither consume a commit nor report a committed sequence.
                    with pytest.raises(ValueError, match="sensitive"):
                        async with service.authority.write(
                            ctx, "aborted", operation_kind="observability.rollback", payload={}
                        ):
                            raise ValueError("sensitive exception: " + private)
                    for _ in range(2):
                        async with service.authority.write(
                            ctx, "accepted", operation_kind="observability.replay", payload={}
                        ) as tx:
                            if not tx.replayed:
                                tx.result({"accepted": True})
                    try:
                        raise ValueError(private)
                    except ValueError:
                        logging.getLogger("integration").exception("Untrusted: %s", private)
        configure_logging("INFO")
        spans = exporter.get_finished_spans()
        chain = [span for span in spans if f"{span.context.trace_id:032x}" == trace_id]
        by_id = {span.context.span_id: span for span in chain}
        request = next(span for span in chain if span.name.startswith("POST "))
        submit = next(span for span in chain if span.name == "imports.submit")
        workflow = next(span for span in chain if span.name == "nous.imports.process")
        assert submit.parent.span_id == request.context.span_id
        assert workflow.parent.span_id == submit.context.span_id
        assert workflow.attributes["operationUUID"] == job_id
        assert any(span.name == "imports.adopt" and span.parent.span_id in by_id for span in chain)
        assert submit.attributes["nous.commit_seq"] > 0
        failed = next(span for span in spans if span.name == "observability.rollback")
        assert failed.status.status_code == StatusCode.ERROR
        assert "nous.commit_seq" not in failed.attributes
        replays = [span for span in spans if span.name == "observability.replay"]
        assert [span.attributes["nous.replayed"] for span in replays] == [False, True]
        assert replays[0].attributes["nous.commit_seq"] == replays[1].attributes["nous.commit_seq"]
        assert private not in logs.getvalue()
        assert token not in logs.getvalue()
        entries = [
            json.loads(line) for line in logs.getvalue().splitlines() if line.startswith("{")
        ]
        assert any(
            row.get("event") == "http.request" and row.get("trace_id") == trace_id
            for row in entries
        )

    asyncio.run(scenario())
