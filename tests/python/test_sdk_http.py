"""Real SDK/HTTP/authority boundaries for exact source selection and host admission."""
import asyncio
from datetime import UTC, datetime

import httpx
import pytest
from nous_wave import HostInputMeasurement, NousClient, NousError, SourceReadScope
from nous_wave.contracts import AdmitFiles, AdoptCognition, EvidenceLink, ImportDocument, MemoryBody, QueryRequest, SourceRegion
from nous_wave.contracts.context import CognitiveTarget, ContextUsage, ContextUse
from nous_wave.source_scope import CognitionSource, MediaSource, OriginalMedia

from nous.platform.configuration import Settings
from nous.transports.http import create_app
from nous.transports.http.sdk_routes import router as sdk_router
from nous_wave.contracts import CallContext


def test_sdk_scopes_and_managed_real_http(tmp_path):
    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        if not any(getattr(route, "path", "") == "/api/subjects/{subject_id}/query" for route in app.routes):
            app.include_router(sdk_router)
        async with app.router.lifespan_context(app):
            client = NousClient("http://test", credential_file=settings.token_file)
            await client.transport.http.aclose()
            client.transport.http = httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://test")
            try:
                namespace = await client.create_namespace("SDK research")
                subject = await client.create_subject(namespace.id, name="SDK host")
                result = await client.admit_documents(namespace.id, subject.id, AdmitFiles(inputs=(ImportDocument(text="限定片段。其余原件秘密。", filename="source.txt", origin={"conversation_ref": "chat:one"}),), llm_enabled=False, vectors="disabled"))
                ref = result.outputs[0].cognition[0]
                material = result.catalog.units[0].material_id
                observation = result.outputs[0].observation_id
                service = app.state.application
                ctx = CallContext(caller_id="instance-admin", namespace_id=namespace.id, subject_id=subject.id)
                async with service.authority.write(ctx, "region-create", operation_kind="region.create", scope=subject.id, payload={"material": material}) as tx:
                    region = await service.material.register_region_in(tx, ctx, SourceRegion(id="region:precise", material_id=material, text_start=0, text_end=5))
                    tx.result(region)
                region_record = await service.cognition.adopt(ctx, AdoptCognition(body=MemoryBody(text="限定片段。"), evidence=(EvidenceLink(observation_id=observation, region_id=region.id),)))
                whole = client.with_source_read(SourceReadScope(subject_id=subject.id, source_refs=(MediaSource(reference=OriginalMedia(material_id=material)),)))
                single = whole.with_source_read(SourceReadScope(subject_id=subject.id, source_refs=(CognitionSource(reference=ref),)))
                selected = await single.query_cognition(namespace.id, subject.id, QueryRequest(text="原件"))
                assert [hit.object.reference for hit in selected.hits] == [ref]
                with pytest.raises(NousError):
                    await single.material(material)
                assert (await whole.material(material)).text == "限定片段。其余原件秘密。"
                region_client = whole.with_source_read({"subject_id": subject.id, "source_refs": [{"kind": "media", "reference": {"kind": "source_region", "id": region.id}}]})
                limited = await region_client.query_cognition(namespace.id, subject.id, QueryRequest(text="限定"))
                assert [hit.object.reference for hit in limited.hits] == [region_record.reference]
                with pytest.raises(NousError):
                    await region_client.material(material)
                scoped_empty = client.with_source_read(SourceReadScope(subject_id=subject.id))
                assert (await scoped_empty.query_cognition(namespace.id, subject.id, QueryRequest(text="限定"))).hits == ()
                binding = single.managed(namespace_id=namespace.id, subject_id=subject.id, session_id="actual-session", consumer_id="reply", instance_id="worker:1")
                prepared = await binding.prepare(usage=ContextUsage(purpose="reply", intent="use admitted memory"), max_items=4, max_text_bytes=4096, current_sources=(CognitiveTarget(reference=ref),))
                applied = binding.apply("Question", prepared, lambda base, segments, usage, view: base + "\n" + "\n".join(segment.text for segment in segments), lambda value: value.encode())
                admission = await binding.admit(applied, invocation_id="actual-invocation", history_generation="history:1", measurement=HostInputMeasurement(input_tokens=len(applied.input.encode()), reserved_output_tokens=16, context_window_tokens=4096, source="host UTF-8 token upper bound"))
                assert admission.rendered_digest == applied.rendered_digest
                outcome = await binding.report(applied, invocation_id="actual-invocation", outcome="completed", occurred_at=datetime.now(UTC), uses=(ContextUse(event_id="use:1", target=CognitiveTarget(reference=ref), kind="referenced"),))
                assert outcome.admission_id == admission.id
                appended = await binding.prepare(usage=ContextUsage(purpose="reply", intent="use admitted memory"), max_items=4, max_text_bytes=4096, current_sources=(CognitiveTarget(reference=ref),))
                assert appended.response.kind == "append"
                assert appended.response.projection.segments == ()
                await binding.close()
            finally:
                await client.close()
    asyncio.run(scenario())
