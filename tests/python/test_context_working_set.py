"""Working-set supply uses Wave and compression replaces covered source segments."""

import asyncio
from pathlib import Path
from uuid import uuid4

from nous_sdk.contracts import (
    AdoptCognition,
    CognitiveRef,
    DomainKnowledgeBody,
    EgoFacetBody,
    EvidenceLink,
    ImportDocument,
)
from nous.platform.call_context import CallContext
from nous_sdk.contracts.context import ContextPrepare, ContextSegment, ContextUsage
from nous_sdk.contracts.targets import CognitiveTarget

from nous.application import Application
from nous.context.steward import compress_segments
from nous.platform.configuration import Settings


def test_context_compression_retains_exact_source_dependencies():
    one = CognitiveTarget(reference=CognitiveRef(kind="memory", id=str(uuid4()), revision=1))
    first = ContextSegment(
        key="first", text="First account", sources=(one,), authority="recorded_cognition"
    )
    compressed = compress_segments(
        [first], "A bounded synthesis.", "actual-call", "context_compress"
    )
    assert compressed.text == "A bounded synthesis."
    assert compressed.dependencies == (one,)
    assert compressed.provenance["producer_call_id"] == "actual-call"


def test_context_supplies_stable_ego_and_wave_related_knowledge(tmp_path: Path):
    async def scenario():
        app = Application(Settings(instance=tmp_path))
        await app.authority.initialize()
        ctx = CallContext(caller_id="host", namespace_id=str(uuid4()), credential_source_ids=None)
        try:
            await app.identity.create_namespace(ctx, "working-set", "namespace", mode="subject")
            async with app.authority.write(ctx) as tx:
                admitted = await app.material.admit_text_in(
                    tx,
                    ctx,
                    ImportDocument(
                        text="A research observer studies stellar evolution.",
                        filename="observation.txt",
                    ),
                )
            evidence = (EvidenceLink(observation_id=admitted.event["id"]),)
            identity = await app.cognition.adopt(
                ctx,
                AdoptCognition(
                    operation_id="identity",
                    body=EgoFacetBody(
                        facet="identity", category="identity", description="A research observer"
                    ),
                    evidence=evidence,
                ),
            )
            knowledge = await app.cognition.adopt(
                ctx,
                AdoptCognition(
                    operation_id="knowledge",
                    body=DomainKnowledgeBody(
                        domain="astrophysics", understanding="stellar evolution and red giants"
                    ),
                    evidence=evidence,
                ),
            )
            prepared = await app.contexts.prepare(
                ctx,
                ContextPrepare(
                    operation_id="prepare",
                    session_id="session",
                    consumer_id="default",
                    instance_id="host",
                    usage=ContextUsage(purpose="reply", intent="stellar evolution"),
                ),
            )
            segments = prepared.response.projection.segments
            assert any(
                segment.key == identity.reference.exact() and segment.required
                for segment in segments
            )
            assert any(segment.key == knowledge.reference.exact() for segment in segments)
        finally:
            await app.close()

    asyncio.run(scenario())
