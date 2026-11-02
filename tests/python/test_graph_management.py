"""Manual graph changes can cite original observations directly."""

import asyncio
from pathlib import Path

import httpx
import pytest
from nous_sdk import NousClient, NousError
from nous_sdk.contracts import EvidenceLink
from nous_sdk.contracts.graph_management import GraphChange

from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_graph_change_uses_original_source(tmp_path: Path):
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
                identity = await client.namespaces.create("graph-source")
                namespace = await client.namespace_by_id(identity.id)
                receipt = await namespace.materials.upload(Path("README.md"), media_type="text/markdown")
                observation = await namespace.materials.observe(receipt.result.id)
                source_id = str(observation["id"])
                created = await client.manage_graph(
                    identity.id,
                    GraphChange(
                        action="create_edge",
                        subject="Nous",
                        predicate="uses",
                        object="Wave",
                        evidence=(EvidenceLink(observation_id=source_id, role="source"),),
                    ),
                )
                relation = next(record for record in created.records if record.reference.kind == "association")
                assert source_id in relation.source_roots
                snapshot = await client.graph_snapshot(identity.id)
                assert relation.reference in {record.reference for record in snapshot.records}
                assert (await namespace.read(relation.reference.exact(), sources=True)).sources is not None
                revised = await client.manage_graph(
                    identity.id,
                    GraphChange(action="update_edge_weight", reference=relation.reference, weight=0.5),
                )
                assert revised.records[0].reference.revision == relation.reference.revision + 1
                with pytest.raises(NousError) as missing:
                    await namespace.read("memory:00000000-0000-0000-0000-000000000000@1")
                assert missing.value.status_code == 404
            finally:
                await client.close()

    asyncio.run(scenario())
