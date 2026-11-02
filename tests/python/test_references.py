"""Directory recovery preserves saved readable names and current source visibility."""

import asyncio
from pathlib import Path

import pytest
from nous_sdk.contracts import (
    AdoptCognition,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError

from nous.cognition.records import CognitiveRecords
from nous.identity.service import IdentityService
from nous.material.service import MaterialService
from nous.platform.persistence import Authority
from nous.platform.persistence.authority import canonical_json
from nous.references.allocation import targets
from nous.references.schema import addresses
from nous.references.service import ReferenceDirectory


def test_wave_graph_preserves_exact_member_and_endpoint_revisions():
    from nous_sdk.contracts import (
        AssociationBody,
        CognitiveRecord,
        CognitiveRef,
        CognitiveTimes,
        ConceptBody,
        TimePoint,
    )

    from nous.retrieval.assets import VectorAsset
    from nous.retrieval.graph import graph

    point = TimePoint(coordinate=0)
    old = CognitiveRef(kind="memory", id="readme", revision=1)
    current = CognitiveRef(kind="memory", id="readme", revision=2)
    concept_ref = CognitiveRef(kind="concept", id="nous", revision=1)
    common = dict(
        namespace_id="scope",
        subject_id="subject",
        evidence=(EvidenceLink(observation_id="source"),),
        source_roots=("source",),
        times=CognitiveTimes(formation=point, recorded=point),
        commit_seq=1,
    )
    memory = CognitiveRecord(reference=current, body=MemoryBody(text="Nous"), **common)
    concept = CognitiveRecord(
        reference=concept_ref, body=ConceptBody(label="Nous", members=(old,)), **common
    )
    relation = CognitiveRecord(
        reference=CognitiveRef(kind="association", id="link", revision=1),
        body=AssociationBody(
            from_ref=old,
            to_ref=concept_ref,
            relation="member",
            polarity="positive",
            basis_class="source_evidence",
        ),
        **common,
    )
    records = [memory, concept, relation]
    assets = {
        item.reference.exact(): VectorAsset(
            reference=item.reference.exact(),
            space="numeric-fixture",
            model="numeric-fixture",
            dimensions=2,
            vector=[1.0, 0.0],
            call_id=None,
            commit_seq=1,
        )
        for item in records
    }
    compiled = graph(records, [memory], assets, 2, {"source": "original"}, core_references=(old,))
    assert [item["key"] for item in compiled.tags] == [concept_ref.exact()]
    assert compiled.member_indices.size == compiled.edge_indices.size == 0


def test_reconcile_recovers_old_names_only_within_visible_sources(tmp_path: Path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        identity = IdentityService(authority)
        materials = MaterialService(authority)
        cognition = CognitiveRecords(authority, materials=materials)
        directory = ReferenceDirectory(authority, cognition, materials)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None,
            caller_id="owner",
            namespace_id="research",
            subject_id="subject",
        )
        try:
            await identity.create_namespace(ctx, "Research", "namespace")
            await identity.create_subject(ctx, "Researcher", "subject", subject_id=ctx.subject_id)
            async with authority.write(ctx) as tx:
                records = []
                for text in ("可读的独立资料。", "当前范围以外的独立资料。"):
                    document = ImportDocument(text=text)
                    material = await materials.put_in(tx, ctx, document)
                    observation = await materials.observe_in(tx, ctx, material["id"], document)
                    records.append(
                        await cognition.adopt_in(
                            tx,
                            ctx,
                            AdoptCognition(
                                body=MemoryBody(text=text),
                                evidence=(EvidenceLink(observation_id=observation["id"]),),
                            ),
                        )
                    )
            first, second = records
            assert first.addresses is not None and second.addresses is not None
            async with authority.write(ctx) as tx:
                encoded = [
                    canonical_json(target)
                    for record in records
                    for target in targets(record.reference)
                ]
                await tx.connection.execute(
                    addresses.delete().where(addresses.c.target.in_(encoded))
                )
            limited = ctx.model_copy(update={"allowed_source_ids": (first.reference.exact(),)})
            recovered = await directory.reconcile(limited, "limited-recovery")
            assert recovered["cognition"] == 1
            assert await directory.reconcile(limited, "limited-recovery") == recovered
            assert (
                await directory.resolve(limited, first.addresses.exact)
            ).reference.reference == first.reference
            with pytest.raises(DomainError) as missing:
                await directory.resolve(ctx, second.addresses.exact)
            assert missing.value.code == "reference_not_found"
            assert (await directory.reconcile(ctx, "full-recovery"))["cognition"] == 1
            assert (
                await directory.resolve(ctx, second.addresses.exact)
            ).reference.reference == second.reference
        finally:
            await authority.close()

    asyncio.run(scenario())


def test_source_objects_round_trip_through_query_and_public_source_read(tmp_path: Path):
    original = Path(__file__).resolve().parents[2] / "README.md"
    original_text = original.read_text(encoding="utf-8")
    root = tmp_path / "documents"
    root.mkdir()
    document = root / "README.md"
    document.write_bytes(original.read_bytes())

    async def scenario():
        import httpx
        from nous_sdk import NousClient
        from nous_sdk.contracts import AdmitFiles, QueryRequest
        from nous_sdk.contracts.references import EvidenceTarget, ObservationTarget, ResourceTarget
        from nous_sdk.contracts.resources import (
            ReadResource,
            RegisterResource,
            ResourceLocation,
            ResourceRecord,
        )
        from nous_sdk.contracts.source_objects import EvidenceRelationRecord, ObservationRecord

        from nous.platform.configuration import Settings
        from nous.transports.http import create_app

        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            async with NousClient("http://test", credential_file=settings.token_file) as api:
                await api.transport.http.aclose()
                api.transport.http = httpx.AsyncClient(
                    transport=httpx.ASGITransport(app=app), base_url="http://test"
                )
                namespace = await api.create_namespace("Source references")
                subject = await api.create_subject(namespace.id)
                namespace_client = await api.namespace_by_id(namespace.id)
                resource = await api.register_resource(
                    namespace.id,
                    subject.id,
                    RegisterResource(
                        name="Project README",
                        location=ResourceLocation(kind="file", address="README.md", file_root=root),
                    ),
                )
                saved = await api.read_resource(
                    namespace.id,
                    subject.id,
                    ReadResource(resource_id=resource.reference.id, current="required"),
                )
                assert saved.snapshot is not None and saved.text == original_text
                queried = await namespace_client.query(QueryRequest(reference=saved.reference.exact(), materialize=True))
                assert len(queried.hits) == 1 and isinstance(queried.hits[0].object, ResourceRecord)
                resolved = await api.resolve_reference(subject.id, "resource:Project README")
                assert (
                    isinstance(resolved.reference, ResourceTarget)
                    and resolved.reference.reference == saved.reference
                )
                expanded = await api.source(subject.id, queried.hits[0].object.reference)
                assert expanded["resource"]["snapshot"]["source"]["id"] == saved.snapshot.source.id

                imported = await api.admit_documents(
                    namespace.id,
                    subject.id,
                    AdmitFiles(
                        inputs=(ImportDocument(text=original_text, filename="knowledge.md"),),
                        llm_enabled=False,
                        vectors="disabled",
                    ),
                )
                observation = imported.outputs[0].observation_id
                events = await namespace_client.query(QueryRequest(reference=f"observation:{observation}"))
                assert len(events.hits) == 1 and isinstance(
                    events.hits[0].object, ObservationRecord
                )
                assert isinstance(
                    (
                        await api.resolve_reference(subject.id, events.hits[0].object.reference)
                    ).reference,
                    ObservationTarget,
                )
                event_source = await api.source(subject.id, events.hits[0].object.reference)
                assert event_source["observation"]["source"]["observation_id"] == observation

                edges = await namespace_client.query(QueryRequest(domains=("evidence",), limit=100))
                assert edges.hits and all(
                    isinstance(hit.object, EvidenceRelationRecord) for hit in edges.hits
                )
                edge = edges.hits[0].object
                resolved = await api.resolve_reference(subject.id, edge.reference)
                assert isinstance(resolved.reference, EvidenceTarget)
                exact = await namespace_client.query(QueryRequest(reference=edge.reference.exact()))
                assert len(exact.hits) == 1 and exact.hits[0].object.reference == edge.reference
                source = await api.source(subject.id, edge.reference)
                assert source["evidence"]["target"]["id"] == edge.target.id

                document.unlink()
                stale = await namespace_client.query(QueryRequest(text='@resource("Project README")', current="prefer"))
                assert stale.execution and "external_snapshot_stale" in stale.execution.reasons
                assert stale.hits[0].object.snapshot.source == saved.snapshot.source
                assert (
                    await api.resource(
                        namespace.id,
                        subject.id,
                        saved.reference.id,
                        revision=saved.reference.revision,
                    )
                ).snapshot.source == saved.snapshot.source

    asyncio.run(scenario())
