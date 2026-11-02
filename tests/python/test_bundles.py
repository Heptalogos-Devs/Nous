"""A Namespace bundle preserves original material and cognition over the SDK."""

import asyncio
from hashlib import sha256
from pathlib import Path

import httpx
from nous_sdk import NousClient
from nous_sdk.contracts import CallContext, ModelProvenance, SourceCommand
from sqlalchemy import select

from nous.cognition.schema import vectors
from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_bundle_sdk_roundtrip_between_namespaces(tmp_path: Path):
    text = Path("README.md").read_text(encoding="utf-8")[:400]

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
                source = await client.namespaces.create("bundle-source")
                target = await client.namespaces.create("bundle-target")
                source_ns = await client.namespace_by_id(source.id)
                target_ns = await client.namespace_by_id(target.id)
                memory = await source_ns.remember(text, source="README.md")
                ctx = CallContext(
                    caller_id="bundle-test", namespace_id=source.id, credential_source_ids=None
                )
                service = app.state.application
                async with service.authority.write(ctx) as tx:
                    await tx.connection.execute(
                        vectors.insert().values(
                            namespace_id=source.id,
                            reference=memory.reference.exact(),
                            space="fixture-space",
                            model="fixture-model",
                            dimensions=2,
                            vector=[0.25, -0.5],
                            producer=ModelProvenance(
                                model_id="fixture-model",
                                task="embedding",
                                prompt_digest=sha256(b"embedding").hexdigest(),
                                schema_digest=sha256(b"vector:2").hexdigest(),
                                input_digest=sha256(text.encode()).hexdigest(),
                                output_digest=sha256(b"[0.25,-0.5]").hexdigest(),
                                generated_at=ctx.clock,
                            ).model_dump(mode="json"),
                            commit_seq=tx.commit_seq,
                        )
                    )
                bundle = await source_ns.bundles.export()
                assert bundle.cognition and bundle.materials and bundle.vectors
                installed = await target_ns.bundles.install(bundle)
                installation_id = installed.installation_id
                assert (await target_ns.bundles.installation(installation_id)).state == "active"
                assert len(await target_ns.bundles.installations()) == 1
                copied = await target_ns.read(memory.reference.exact(), sources=True)
                assert copied.text == text and copied.sources is not None
                target_ctx = ctx.model_copy(update={"namespace_id": target.id})
                async with service.authority.read(target_ctx) as view:
                    row = (
                        await view.connection.execute(
                            select(vectors).where(
                                vectors.c.namespace_id == target.id,
                                vectors.c.reference == memory.reference.exact(),
                            )
                        )
                    ).mappings().one()
                    assert row["vector"] == [0.25, -0.5]
                await target_ns.bundles.uninstall(installation_id)
                assert (await target_ns.bundles.installation(installation_id)).state == "uninstalled"
                await client.purge_sources(
                    target.id,
                    SourceCommand(
                        observation_ids=installed.owned_observation_ids,
                        reason="Remove the uninstalled copy",
                        force=True,
                    ),
                )
                assert (await source_ns.read(memory.reference.exact())).text == text
            finally:
                await client.close()

    asyncio.run(scenario())
