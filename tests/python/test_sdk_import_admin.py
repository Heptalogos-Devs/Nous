"""Administrative retries consume frozen split facts and actual unit attempts."""

import asyncio

import httpx
import pytest
from nous_wave import NousClient, NousError
from nous_wave.amemorix import AMemorixAdapter

from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_import_retry_formation_overrides_and_runtime_reports(tmp_path):
    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            client = NousClient("http://test", credential_file=settings.token_file)
            await client.transport.http.aclose()
            client.transport.http = httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            )
            try:
                namespace = await client.create_namespace("import retry")
                subject = await client.create_subject(namespace.id, name="import host")
                adapter = AMemorixAdapter(client, namespace_id=namespace.id, subject_id=subject.id)
                created = await adapter.memory_import_admin(
                    action="create_paste",
                    name="story.txt",
                    content="A traveler reached the town. Then she returned to the coast.",
                    strategy_override="narrative",
                    llm_enabled=True,
                    vectors="disabled",
                )
                task = created["task"]["task_id"]
                # A fresh instance has no formation model configured; this is a real failed attempt.
                failed = (
                    await client.wait_jobs(
                        (task,), namespace_id=namespace.id, subject_id=subject.id
                    )
                )[0]
                assert failed.status == "ERROR"
                detail = await adapter.memory_import_admin(
                    action="get", task_id=task, include_chunks=True
                )
                assert detail["chunks"][0]["status"] == "failed"
                with pytest.raises(NousError) as changed:
                    await adapter.memory_import_admin(
                        action="retry_failed",
                        task_id=task,
                        overrides={"chunk_size": 3200, "llm_enabled": False},
                    )
                assert changed.value.code == "import_resplit_required"
                retried = await adapter.memory_import_admin(
                    action="retry_failed",
                    task_id=task,
                    overrides={"chunk_size": 2400, "extract_time": False, "llm_enabled": False},
                    vectors="disabled",
                )
                retry_task = retried["task"]["task_id"]
                await client.wait_success(
                    (retry_task,), namespace_id=namespace.id, subject_id=subject.id
                )
                completed = await adapter.memory_import_admin(
                    action="get", task_id=retry_task, include_chunks=True
                )
                assert completed["task"]["done_chunks"] == 1
                assert completed["chunks"][0]["unit_id"] == detail["chunks"][0]["unit_id"]
                report = await adapter.memory_runtime_admin(action="get_config")
                assert report["models"]["provider_probed"] is False
                assert report["embedding_fingerprint"] is None
                assert report["imports"]["visible_outputs"] == 1
                summary = await client.subject_diagnostics(subject.id)
                assert summary["retrieval"]["visible_targets"] >= 1
                assert summary["imports"]["disabled_outputs"] == 1
                await adapter.ingest_text(
                    "second-traveler", "document", "Another traveler remembered the coast."
                )
                protected = await adapter.maintain_memory(
                    "protect", target="traveler", hours=0, operation_id="lexical-selection-once"
                )
                assert protected["count"] == 2
                assert (
                    await adapter.maintain_memory(
                        "protect", target="traveler", hours=0, operation_id="lexical-selection-once"
                    )
                    == protected
                )
                with pytest.raises(NousError):
                    await adapter.with_source_read({"subject_id": subject.id}).maintain_memory(
                        "protect", target="traveler", hours=0, operation_id="lexical-selection-once"
                    )
                assert (
                    await adapter.maintain_memory(
                        "unprotect", target="traveler", operation_id="unprotect-actual-selection"
                    )
                )["count"] == 2
                directory = await client.sources(
                    {"action": "list", "namespace_id": namespace.id, "subject_id": subject.id}
                )
                label = directory["items"][0]["source"]
                preview = await client.sources(
                    {
                        "action": "preview",
                        "namespace_id": namespace.id,
                        "subject_id": subject.id,
                        "selector": [label],
                    }
                )
                assert preview["preview"]["observation_ids"]
            finally:
                await client.close()

    asyncio.run(scenario())
