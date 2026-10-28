"""Real SDK/HTTP/authority boundaries for exact source selection and host admission."""

import asyncio
from datetime import UTC, datetime

import httpx
import pytest
from nous_wave import HostInputMeasurement, NousClient, NousError, SourceReadScope
from nous_wave.contracts import (
    AdmitFiles,
    AdoptCognition,
    CallContext,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    QueryRequest,
    SourceRegion,
)
from nous_wave.contracts.context import CognitiveTarget, ContextUsage, ContextUse
from nous_wave.source_scope import CognitionSource, MediaSource, OriginalMedia

from nous.platform.configuration import Settings
from nous.transports.http import create_app
from nous.transports.http.sdk_routes import router as sdk_router


def test_sdk_scopes_and_managed_real_http(tmp_path):
    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        if not any(
            getattr(route, "path", "") == "/api/subjects/{subject_id}/query" for route in app.routes
        ):
            app.include_router(sdk_router)
        async with app.router.lifespan_context(app):
            client = NousClient("http://test", credential_file=settings.token_file)
            await client.transport.http.aclose()
            client.transport.http = httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            )
            try:
                namespace = await client.create_namespace("SDK research")
                subject = await client.create_subject(namespace.id, name="SDK host")
                result = await client.admit_documents(
                    namespace.id,
                    subject.id,
                    AdmitFiles(
                        inputs=(
                            ImportDocument(
                                text="限定片段。其余原件秘密。",
                                filename="source.txt",
                                origin={"conversation_ref": "chat:one"},
                            ),
                        ),
                        llm_enabled=False,
                        vectors="disabled",
                    ),
                )
                ref = result.outputs[0].cognition[0]
                material = result.catalog.units[0].material_id
                observation = result.outputs[0].observation_id
                service = app.state.application
                ctx = CallContext(
                    caller_id="instance-admin", namespace_id=namespace.id, subject_id=subject.id
                )
                async with service.authority.write(
                    ctx,
                    "region-create",
                    operation_kind="region.create",
                    scope=subject.id,
                    payload={"material": material},
                ) as tx:
                    region = await service.material.register_region_in(
                        tx,
                        ctx,
                        SourceRegion(
                            id="region:precise", material_id=material, text_start=0, text_end=5
                        ),
                    )
                    tx.result(region)
                region_record = await service.cognition.adopt(
                    ctx,
                    AdoptCognition(
                        body=MemoryBody(text="限定片段。"),
                        evidence=(EvidenceLink(observation_id=observation, region_id=region.id),),
                    ),
                )
                whole = client.with_source_read(
                    SourceReadScope(
                        subject_id=subject.id,
                        source_refs=(MediaSource(reference=OriginalMedia(material_id=material)),),
                    )
                )
                single = whole.with_source_read(
                    SourceReadScope(
                        subject_id=subject.id, source_refs=(CognitionSource(reference=ref),)
                    )
                )
                selected = await single.query_cognition(
                    namespace.id, subject.id, QueryRequest(text="原件")
                )
                assert [hit.object.reference for hit in selected.hits] == [ref]
                with pytest.raises(NousError):
                    await single.material(material)
                record = await client.cognitive_revision(namespace.id, subject.id, ref)
                assert record.addresses is not None
                readable = whole.with_source_read(
                    SourceReadScope(
                        subject_id=subject.id,
                        source_refs=(
                            CognitionSource(
                                reference=record.addresses.object or record.addresses.exact
                            ),
                        ),
                    )
                )
                assert [
                    hit.object.reference
                    for hit in (
                        await readable.query_cognition(
                            namespace.id, subject.id, QueryRequest(text="原件")
                        )
                    ).hits
                ] == [ref]
                with pytest.raises(NousError):
                    await whole.with_source_read(
                        SourceReadScope(
                            subject_id=subject.id,
                            source_refs=(MediaSource(reference=record.addresses.exact),),
                        )
                    ).query_cognition(namespace.id, subject.id, QueryRequest(text="原件"))
                assert (await whole.material(material)).text == "限定片段。其余原件秘密。"
                region_client = whole.with_source_read(
                    {
                        "subject_id": subject.id,
                        "source_refs": [
                            {
                                "kind": "media",
                                "reference": {"kind": "source_region", "id": region.id},
                            }
                        ],
                    }
                )
                limited = await region_client.query_cognition(
                    namespace.id, subject.id, QueryRequest(text="限定")
                )
                assert [hit.object.reference for hit in limited.hits] == [region_record.reference]
                with pytest.raises(NousError):
                    await region_client.material(material)
                scoped_empty = client.with_source_read(SourceReadScope(subject_id=subject.id))
                assert (
                    await scoped_empty.query_cognition(
                        namespace.id, subject.id, QueryRequest(text="限定")
                    )
                ).hits == ()
                from nous_wave.contracts.context import ContextCommand

                await single.admit_context(
                    namespace.id,
                    subject.id,
                    ContextCommand(
                        kind="resident", purpose="unbound historical context", references=(ref,)
                    ),
                )
                from nous_wave.contracts.context import ContextPrepare

                unbound = await single.prepare_context(
                    namespace.id,
                    subject.id,
                    ContextPrepare(
                        session_id="unbound-session",
                        consumer_id="reply",
                        instance_id="scope-check",
                        usage=ContextUsage(purpose="reply"),
                    ),
                )
                assert unbound.response.projection.segments == ()
                await single.cancel_context_preparation(
                    namespace.id, subject.id, unbound.id, "cancel-unbound"
                )
                binding = single.managed(
                    namespace_id=namespace.id,
                    subject_id=subject.id,
                    session_id="actual-session",
                    consumer_id="reply",
                    instance_id="worker:1",
                )
                prepared = await binding.prepare(
                    usage=ContextUsage(purpose="reply", intent="use admitted memory"),
                    max_items=4,
                    max_text_bytes=4096,
                    current_sources=(CognitiveTarget(reference=ref),),
                )
                applied = binding.apply(
                    "Question",
                    prepared,
                    lambda base, segments, usage, view: (
                        base + "\n" + "\n".join(segment.text for segment in segments)
                    ),
                    lambda value: value.encode(),
                )
                admission = await binding.admit(
                    applied,
                    invocation_id="actual-invocation",
                    history_generation="history:1",
                    measurement=HostInputMeasurement(
                        input_tokens=len(applied.input.encode()),
                        reserved_output_tokens=16,
                        context_window_tokens=4096,
                        source="host UTF-8 token upper bound",
                    ),
                )
                assert admission.rendered_digest == applied.rendered_digest
                outcome = await binding.report(
                    applied,
                    invocation_id="actual-invocation",
                    outcome="completed",
                    occurred_at=datetime.now(UTC),
                    uses=(
                        ContextUse(
                            event_id="use:1",
                            target=CognitiveTarget(reference=ref),
                            kind="referenced",
                        ),
                    ),
                )
                assert outcome.admission_id == admission.id
                appended = await binding.prepare(
                    usage=ContextUsage(purpose="reply", intent="use admitted memory"),
                    max_items=4,
                    max_text_bytes=4096,
                    current_sources=(CognitiveTarget(reference=ref),),
                )
                assert appended.response.kind == "append"
                assert appended.response.projection.segments == ()
                await binding.close()
                from datetime import timedelta

                from nous_wave.contracts.context import (
                    ChangeResidents,
                    ChangeWorkContext,
                    ContextPrepare,
                    ContextResident,
                    OpenContextSession,
                    WorkContextContent,
                )

                session = await single.open_context_session(
                    namespace.id, subject.id, OpenContextSession(session_id="resident-session")
                )
                target = CognitiveTarget(reference=ref)
                resident = ContextResident(
                    target=target,
                    entry_reason="explicit host pin",
                    entered_at=datetime.now(UTC) - timedelta(days=2),
                )
                revised = await single.change_context_residents(
                    namespace.id,
                    subject.id,
                    ChangeResidents(
                        session_id=session.id,
                        expected_revision=session.revision,
                        action="admit",
                        residents=(resident,),
                    ),
                )
                assert revised.residents[0].last_meaningful_use_at is None
                with pytest.raises(NousError):
                    await single.change_context_residents(
                        namespace.id,
                        subject.id,
                        ChangeResidents(
                            session_id=session.id,
                            expected_revision=session.revision,
                            action="clear",
                        ),
                    )
                work = await single.change_work_context(
                    namespace.id,
                    subject.id,
                    ChangeWorkContext(
                        action="create",
                        content=WorkContextContent(
                            purpose="answer source",
                            context_text="required task directive",
                            anchors=(target,),
                        ),
                    ),
                )
                await single.change_work_context(
                    namespace.id,
                    subject.id,
                    ChangeWorkContext(
                        action="pin",
                        work_context_id=work.id,
                        expected_revision=work.revision,
                        session_id=session.id,
                    ),
                )
                with pytest.raises(NousError):
                    await single.prepare_context(
                        namespace.id,
                        subject.id,
                        ContextPrepare(
                            session_id=session.id,
                            consumer_id="reply",
                            instance_id="budget",
                            usage=ContextUsage(purpose="reply"),
                            max_items=4,
                            max_text_bytes=1,
                        ),
                    )
                assert (
                    await single.context_session(namespace.id, subject.id, session.id)
                ).active_work_context_id == work.id
                maintained = await service.contexts.maintain_session(
                    ctx, session.id, "resident-expiry"
                )
                assert maintained.residents[0].state == "evicted"
                await single.close_context_session(
                    namespace.id, subject.id, session.id, "close-resident"
                )
                from nous_wave.amemorix import AMemorixAdapter
                from nous_wave.amemorix.contracts import AMemorixPolicy

                from nous.transports.http.amemorix import router as amemorix_router

                if not any(
                    getattr(route, "path", "") == "/api/integrations/amemorix"
                    for route in app.routes
                ):
                    app.include_router(amemorix_router)
                await service.configuration.set(
                    ctx,
                    "integrations.amemorix",
                    AMemorixPolicy(search_mode="lexical").model_dump(mode="json"),
                    "amemorix-policy",
                    "subject",
                )
                adapter = AMemorixAdapter(
                    client,
                    namespace_id=namespace.id,
                    subject_id=subject.id,
                    transfer_root=tmp_path / "transfer",
                )
                stored = await adapter.ingest_text(
                    external_id="person-fact:1",
                    source_type="person_fact",
                    text="Alice喜欢喝绿茶。",
                    chat_id="chat:one",
                    person_ids=("person:alice",),
                    metadata={"person_name": "Alice"},
                    tags=("饮食",),
                )
                assert stored["success"]
                again = await adapter.ingest_text(
                    external_id="person-fact:1",
                    source_type="person_fact",
                    text="Alice喜欢喝绿茶。",
                    chat_id="chat:one",
                    person_ids=("person:alice",),
                    metadata={"person_name": "Alice"},
                    tags=("饮食",),
                )
                assert again["stored_ids"] == stored["stored_ids"]
                assert (await adapter.search_memory("绿茶", chat_id="chat:one"))["hits"]
                profile = await adapter.get_person_profile(person_id="person:alice")
                assert any("绿茶" in trait for trait in profile["traits"])
                assert (await adapter.memory_stats())["paragraphs"] >= 2
                memory_id = stored["stored_ids"][0]
                await adapter.maintain_memory(action="freeze", target=memory_id)
                await adapter.maintain_memory(action="restore", target=memory_id)
                assert (await adapter.memory_source(memory_id))["success"]
                directory = await adapter.memory_source_admin(action="list")
                assert directory["sources"]
                exported = await adapter.memory_bundle_admin(action="export")
                assert exported["material_id"]
                inspection = await adapter.memory_bundle_admin(
                    action="inspect", material_id=exported["material_id"]
                )
                assert inspection["manifest"]["cognition"] >= 2
                import json

                transfer = adapter.managed_files.root
                await asyncio.to_thread(transfer.mkdir, parents=True, exist_ok=True)
                path = transfer / "records.json"
                content = json.dumps(
                    {
                        "records": [
                            {
                                "external_id": "json-record:1",
                                "text": "上传JSON原件保留记录。",
                                "tags": ["actual"],
                            }
                        ]
                    },
                    ensure_ascii=False,
                )
                await asyncio.to_thread(path.write_text, content)
                created = await adapter.memory_import_admin(
                    action="create_upload",
                    input_mode="json",
                    staged_files=[
                        {
                            "filename": path.name,
                            "staged_path": str(path),
                            "size": len(content.encode()),
                        }
                    ],
                    llm_enabled=False,
                    vectors="disabled",
                    dedupe_policy="content_hash",
                    scope_type="global",
                )
                task_id = created["task"]["task_id"]
                await client.wait_success(
                    (task_id,), namespace_id=namespace.id, subject_id=subject.id
                )
                detail = await adapter.memory_import_admin(
                    action="get", task_id=task_id, include_chunks=True
                )
                assert detail["task"]["done_chunks"] == 1
                assert detail["files"][0]["input_mode"] == "json"
                assert detail["chunks"][0]["native_job_id"] == task_id
                assert (await adapter.memory_import_admin(action="list"))["items"]
                page = await adapter.memory_import_admin(
                    action="get_chunks",
                    task_id=task_id,
                    file_id=detail["files"][0]["file_id"],
                    offset=0,
                    limit=10,
                )
                assert len(page["chunks"]) == 1
                skipped = await adapter.memory_import_admin(
                    action="create_upload",
                    input_mode="json",
                    staged_files=[
                        {
                            "filename": path.name,
                            "staged_path": str(path),
                            "size": len(content.encode()),
                        }
                    ],
                    llm_enabled=False,
                    vectors="disabled",
                    dedupe_policy="content_hash",
                    scope_type="global",
                )
                await client.wait_success(
                    (skipped["task"]["task_id"],), namespace_id=namespace.id, subject_id=subject.id
                )
                assert skipped["task"]["task_id"] != task_id

            finally:
                await client.close()

    asyncio.run(scenario())
