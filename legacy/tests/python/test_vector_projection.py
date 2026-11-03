"""A projection changes routes as a complete directory, retaining actual paid batches."""

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
from nous_sdk.contracts import AdmitFiles, ImportDocument
from nous.platform.call_context import CallContext
from nous_sdk.contracts.model_routing import ModelRoutingPolicy
from sqlalchemy import select

from nous.bootstrap import application
from nous.cognition.schema import vectors
from nous.platform.configuration import EmbeddingRoute, Settings
from nous.platform.configuration.models import ModelConfiguration
from nous.platform.models.ledger import calls


@pytest.mark.parametrize("rate_limit_retries,manual_retry", [(0, False), (1, False), (0, True)])
def test_later_batch_rejection_reprojects_the_complete_directory(
    tmp_path: Path, monkeypatch, rate_limit_retries, manual_retry
):
    received: list[str] = []

    class Provider(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            model = request["model"]
            received.append(model)
            rejected = model == "first" and received.count("first") == 2
            response = (
                {"error": {"message": "Rate limited", "type": "rate_limit_error"}}
                if rejected
                else {
                    "model": model + "-resolved",
                    "object": "list",
                    "data": [{"index": 0, "object": "embedding", "embedding": [1.0, 0.0]}],
                    "usage": {"prompt_tokens": 2, "total_tokens": 2},
                }
            )
            body = json.dumps(response).encode()
            self.send_response(429 if rejected else 200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("NOUS_PROJECTION_TEST_KEY", "local-test-key")

    async def scenario():
        settings = Settings(
            instance=tmp_path / "instance",
            embedding=EmbeddingRoute(
                base_url=f"http://127.0.0.1:{server.server_port}/v1",
                model="first",
                credential_env="NOUS_PROJECTION_TEST_KEY",
                dimensions=2,
                max_batch_items=1,
            ),
        )
        async with application(settings) as app:
            ctx = CallContext(
                credential_source_ids=None,
                caller_id="owner",
                namespace_id="namespace",
                subject_id="subject",
            )
            await app.identity.create_namespace(ctx, "Namespace", "namespace")
            await app.identity.create_subject(ctx, "Subject", "subject", subject_id="subject")
            effective = await app.configuration.effective(ctx, "models")
            resources = ModelConfiguration.model_validate(effective["value"])
            resources.catalog["second"] = resources.catalog["embedding"].model_copy(
                update={"name": "second"}
            )
            resources.presets["second"] = resources.presets["embedding"].model_copy(
                update={"model": "second"}
            )
            resources.roles["embedding"] = resources.roles["embedding"].model_copy(
                update={
                    "routes": ("embedding",) if manual_retry else ("embedding", "second"),
                    "fallback": ModelRoutingPolicy(
                        rate_limit_retries=rate_limit_retries,
                        rate_limit_wait_seconds=0,
                    ),
                }
            )
            await app.configuration.set(ctx, "models", resources)
            imported = await app.imports.admit_files(
                ctx,
                AdmitFiles(
                    operation_id="documents",
                    inputs=(
                        ImportDocument(text="First complete project document.", filename="one.txt"),
                        ImportDocument(
                            text="Second complete project document.", filename="two.txt"
                        ),
                    ),
                    llm_enabled=False,
                    vectors="disabled",
                ),
            )
            targets = tuple(output.cognition[0] for output in imported.outputs)
            if manual_retry:
                from nous_sdk.contracts.vector_projection import ProjectionResult, ProjectVectors
                from nous_sdk.contracts.vector_spaces import RetryVectorProjection

                from nous.platform.errors import NousError

                with pytest.raises(NousError) as rejected:
                    await app.vector_projection.project(ctx, targets, "projection")
                assert rejected.value.code == "model_routes_exhausted"
                original = await app.vector_projection.submit(
                    ctx, ProjectVectors(operation_id="projection", targets=targets)
                )
                retry_command = RetryVectorProjection(
                    operation_id="resume-original", request_id=original.request_id
                )
                resumed = await app.vector_spaces.retry(ctx, retry_command)
                result = ProjectionResult.model_validate(
                    await app.execution.wait(ctx, resumed.job_id)
                )
                assert result.request_id == original.request_id
                assert await app.vector_spaces.retry(ctx, retry_command) == resumed
            else:
                result = await app.vector_projection.project(ctx, targets, "projection")
            assert result.references == tuple(target.exact() for target in targets)
            assert len(result.spaces) == 1
            async with app.authority.read(ctx) as view:
                adopted = (await view.connection.execute(select(vectors))).mappings().all()
                physical = (await view.connection.execute(select(calls))).mappings().all()
            assert {row["model"] for row in adopted} == {
                "first-resolved" if rate_limit_retries or manual_retry else "second-resolved"
            }
            assert len(adopted) == 2
            assert [row["status"] for row in physical] == (
                ["succeeded", "failed", "succeeded"]
                if rate_limit_retries or manual_retry
                else ["succeeded", "failed", "succeeded", "succeeded"]
            )
            assert physical[0]["result"]["model"] == "first-resolved"
            inventory = await app.vector_spaces.catalog(ctx)
            assert inventory.configured_space_complete
            resolved = next(
                space for space in inventory.spaces if space.space_id == result.spaces[0]
            )
            assert resolved.configured and resolved.model.endswith("-resolved")
            retrieval_status = await app.retrieval.status(ctx)
            assert retrieval_status.spaces[0].model == resolved.model
            assert retrieval_status.configured_space_complete == (
                retrieval_status.visible_targets == 2
            )
            from nous_sdk.contracts.vector_spaces import RebuildVectors

            missing = await app.vector_spaces.rebuild(
                ctx, RebuildVectors(operation_id="check-complete-space", dry_run=True)
            )
            assert not missing.references and missing.projection is None
            assert (await app.vector_spaces.backfill_status(ctx))["done"] == 2
            # Replaying the same logical request reuses the frozen paid results.
            assert await app.vector_projection.project(ctx, targets, "projection") == result
            assert received == (
                ["first", "first", "first"]
                if rate_limit_retries or manual_retry
                else ["first", "first", "second", "second"]
            )
            if rate_limit_retries:
                from nous.platform.errors import DomainError
                from nous_sdk.contracts.vector_spaces import DeleteVectorSpace, RebuildVectors

                namespace = ctx.model_copy(update={"subject_id": None})
                inventory = await app.vector_spaces.catalog(namespace)
                assert next(
                    space for space in inventory.spaces if space.space_id == result.spaces[0]
                ).can_delete
                with pytest.raises(DomainError) as denied:
                    await app.vector_spaces.delete(
                        ctx, DeleteVectorSpace(space_id=result.spaces[0])
                    )
                assert denied.value.code == "vector_scope_forbidden"
                deleted = await app.vector_spaces.delete(
                    namespace,
                    DeleteVectorSpace(operation_id="delete-space", space_id=result.spaces[0]),
                )
                assert deleted.vectors_deleted == 2
                assert (await app.vector_spaces.catalog(ctx)).configured_space_complete is False
                with pytest.raises(DomainError) as retired:
                    await app.vector_projection.run(result.request_id)
                assert retired.value.code == "vector_space_deleted"
                rebuilt = await app.vector_spaces.rebuild(
                    ctx, RebuildVectors(operation_id="restore-space", mode="all")
                )
                await app.execution.wait(ctx, rebuilt.projection.job_id)
                progress = await app.vector_spaces.progress(ctx, rebuilt.projection.request_id)
                assert progress.status == "SUCCESS" and progress.adopted_references == 2
                assert (await app.vector_spaces.catalog(ctx)).configured_space_complete
                assert len(await app.models.ledger.list(ctx)) == 3
                assert received == ["first", "first", "first"]
                from nous_sdk.contracts.vector_spaces import CheckVectorSpaces

                probe_one = await app.vector_spaces.check(
                    ctx,
                    CheckVectorSpaces(
                        operation_id="probe-one",
                        probe=True,
                        sample_text="First complete project document.",
                    ),
                )
                probe_command = CheckVectorSpaces(
                    operation_id="probe-two",
                    probe=True,
                    sample_text="First complete project document.",
                )
                probe_two = await app.vector_spaces.check(ctx, probe_command)
                assert probe_one.producer_call_id != probe_two.producer_call_id
                assert await app.vector_spaces.check(ctx, probe_command) == probe_two
                assert received == ["first"] * 5

    try:
        asyncio.run(scenario())
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
