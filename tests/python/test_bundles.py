"""Real archive interoperability retains immutable sources and exact vector identity."""

import asyncio
from io import BytesIO
from pathlib import Path
from uuid import uuid4

from nous_sdk.contracts import (
    AdmitFiles,
    AdoptCognition,
    BlobDocument,
    BundleRequest,
    CallContext,
    EntityBody,
    EvidenceLink,
    ImportDocument,
    MediaInfo,
    NamespaceBundle,
)
from nous_sdk.contracts.bundles import BundleAssetOptions, PackageDescriptor
from PIL import Image
from sqlalchemy import select

from nous.application import Application
from nous.cognition.schema import vectors
from nous.data_management.archive_read import decode
from nous.data_management.portable_read import load
from nous.platform.configuration import Settings


def test_external_portable_and_native_vector_roundtrip(tmp_path: Path):
    async def scenario():
        app = Application(Settings(instance=tmp_path))
        authority, content, materials, cognition = (
            app.authority,
            app.content,
            app.material,
            app.cognition,
        )
        imports, bundles, identity = app.imports, app.bundles, app.identity
        await content.initialize()
        await authority.initialize()
        ctx = CallContext(credential_source_ids=None, caller_id="owner", namespace_id=str(uuid4()))
        try:
            await identity.create_namespace(ctx, "namespace", "namespace")
            result = await imports.admit_files(
                ctx,
                AdmitFiles(
                    operation_id="literal",
                    inputs=(ImportDocument(text="Alice chooses coffee.", filename="source.txt"),),
                    llm_enabled=False,
                    vectors="disabled",
                ),
            )
            memory = await cognition.get(ctx, result.outputs[0].cognition[0])
            await cognition.adopt(
                ctx,
                AdoptCognition(
                    operation_id="entity",
                    body=EntityBody(label="Alice", entity_type="person"),
                    evidence=(EvidenceLink(reference=memory.reference),),
                ),
            )
            async with authority.write(
                ctx, "vector", operation_kind="vector-fixture", payload={}
            ) as tx:
                await tx.connection.execute(
                    vectors.insert().values(
                        reference=memory.reference.exact(),
                        space="known-model:2",
                        model="known-model",
                        dimensions=2,
                        vector=[0.25, -0.5],
                        call_id=None,
                        commit_seq=tx.commit_seq,
                    )
                )
                tx.result(None)
            pixels = BytesIO()
            Image.new("RGB", (3, 2), (10, 80, 120)).save(pixels, format="PNG")
            lease = await content.publish_bytes(pixels.getvalue(), "image/png")
            image_document = BlobDocument(filename="image.png", media_type="image/png")
            try:
                async with authority.write(ctx, "image", operation_kind="image", payload={}) as tx:
                    image_material = await materials.put_blob_in(
                        tx,
                        ctx,
                        lease,
                        image_document,
                        MediaInfo(kind="image", media_type="image/png", width=3, height=2),
                    )
                    await materials.observe_in(
                        tx, ctx, image_material["id"], image_document, "contact"
                    )
                    tx.result(None)
            finally:
                await content.release(lease)
            native = await bundles.export_archive(
                ctx, BundleRequest(assets=BundleAssetOptions(include_vectors=True))
            )
            decoded = decode(native.data)
            assert len(decoded.vectors) == 1
            receiver = ctx.model_copy(update={"namespace_id": str(uuid4())})
            await identity.create_namespace(receiver, "receiver", "receiver")
            installed = await bundles.install_zip(receiver, native.data, "native-install")
            copied_ref = "memory:" + installed["mapping"][memory.reference.id] + "@1"
            async with authority.read(receiver) as view:
                copied = (
                    (
                        await view.connection.execute(
                            select(vectors).where(vectors.c.reference == copied_ref)
                        )
                    )
                    .mappings()
                    .one()
                )
                assert copied["space"] == "known-model:2"
                assert copied["vector"] == [0.25, -0.5]
            command = BundleRequest(
                format="amemorix",
                content_level="knowledge",
                package=PackageDescriptor(id="coffee", name="Coffee", version="1"),
            )
            portable = await bundles.export_archive(ctx, command)
            checked = load(portable.data)
            assert "bundle.json" not in checked.members
            assert checked.knowledge["docs"]
            assert checked.manifest["format_version"] == 2
            assert checked.images["assets"][0]["content_hash"] == image_material["content_hash"]
            normalized = decode(portable.data)
            assert (
                NamespaceBundle.model_validate(normalized).cognition[0].body.text
                == "Alice chooses coffee."
            )
            target = ctx.model_copy(update={"namespace_id": str(uuid4())})
            await identity.create_namespace(target, "external", "external")
            adopted = await bundles.install_zip(target, portable.data, "portable-install")
            assert adopted["cognition_count"] == 2
            assert len(await bundles.list_installs(target)) == 1
            assert await bundles.install_zip(target, portable.data, "portable-install") == adopted
            async with authority.read(target) as view:
                for row in normalized.materials:
                    assert (await materials.read_in(view, target, adopted["mapping"][row.id]))[
                        "content_hash"
                    ]
            await bundles.uninstall(target, adopted["installation_id"], "uninstall")
            assert (await bundles.get_installation(target, adopted["installation_id"]))[
                "state"
            ] == "uninstalled"
        finally:
            await app.close()

    asyncio.run(scenario())


def test_artifact_contact_inherits_inputs_and_withdrawal(tmp_path: Path):
    async def scenario():
        from nous_sdk.contracts import BlobDocument, MediaReference, SourceCommand

        app = Application(Settings(instance=tmp_path))
        authority, store, materials, cognition = (
            app.authority,
            app.content,
            app.material,
            app.cognition,
        )
        imports, sources, identity = app.imports, app.sources, app.identity
        await store.initialize()
        await authority.initialize()
        ctx = CallContext(credential_source_ids=None, caller_id="owner", namespace_id=str(uuid4()))
        try:
            await identity.create_namespace(ctx, "namespace", "namespace")
            imported = await imports.admit_files(
                ctx,
                AdmitFiles(
                    operation_id="original",
                    inputs=(
                        ImportDocument(text="Retained original source.", filename="original.txt"),
                    ),
                    llm_enabled=False,
                    vectors="disabled",
                ),
            )
            event = imported.outputs[0].observation_id
            original = imported.catalog.units[0].material_id
            lease = await store.publish_bytes(b"Derived excerpt.", "text/plain")
            try:
                async with authority.write(
                    ctx, "artifact", operation_kind="artifact", payload={}
                ) as tx:
                    artifact = await materials.put_blob_in(
                        tx,
                        ctx,
                        lease,
                        BlobDocument(filename="excerpt.txt", media_type="text/plain"),
                        required_source_ids=(f"material:{original}",),
                        required_observation_ids=(event,),
                    )
                    own_contact = await materials.observe_in(
                        tx,
                        ctx,
                        artifact["id"],
                        BlobDocument(filename="excerpt.txt", media_type="text/plain"),
                        "contact",
                    )
                    tx.result(None)
            finally:
                await store.release(lease)
            scoped = ctx.model_copy(update={"allowed_source_ids": (artifact["id"],)})
            async with authority.read(scoped) as view:
                provenance = await materials.provenance_in(
                    view,
                    scoped,
                    MediaReference(
                        kind="material", id=artifact["id"], observation_id=own_contact["id"]
                    ),
                )
                assert provenance.roots == (event,)
                assert own_contact["id"] not in provenance.roots
            from nous_sdk.contracts import MemoryBody

            memory = await cognition.adopt(
                scoped,
                AdoptCognition(
                    operation_id="artifact-memory",
                    body=MemoryBody(text="Derived excerpt."),
                    evidence=(EvidenceLink(observation_id=own_contact["id"]),),
                ),
            )
            assert memory.source_roots == (event,)
            assert memory.body.media[0].observation_id == own_contact["id"]
            assert (await cognition.get(scoped, memory.reference)).body.text == "Derived excerpt."
            await sources.trash(
                ctx,
                SourceCommand(operation_id="withdraw", observation_ids=(event,), reason="withdraw"),
            )
            from nous_sdk.contracts import DomainError

            try:
                await cognition.get(scoped, memory.reference)
            except DomainError as error:
                assert error.code == "source_unavailable"
            else:
                raise AssertionError(
                    "An artifact contact must not become an independent support root"
                )
            await sources.restore(
                ctx,
                SourceCommand(operation_id="restore", observation_ids=(event,), reason="restore"),
            )
            assert (await cognition.get(scoped, memory.reference)).body.text == "Derived excerpt."
        finally:
            await app.close()

    asyncio.run(scenario())


def test_namespace_bundle_preserves_paid_call_custody(tmp_path: Path):
    async def scenario():
        from nous_sdk.contracts import DomainError, MemoryBody

        app = Application(Settings(instance=tmp_path))
        authority, materials, cognition = app.authority, app.material, app.cognition
        ledger, bundles = app.models.ledger, app.bundles
        await app.content.initialize()
        await authority.initialize()
        source = CallContext(
            caller_id="author", namespace_id=str(uuid4()), credential_source_ids=None
        )
        receiver = source.model_copy(update={"namespace_id": str(uuid4()), "caller_id": "receiver"})
        try:
            identity = app.identity
            await identity.create_namespace(source, "source", "create-source")
            await identity.create_namespace(receiver, "receiver", "create-receiver")
            async with authority.write(
                source, "admit", operation_kind="material.admit", payload={}
            ) as tx:
                admitted = await materials.admit_text_in(
                    tx, source, ImportDocument(text="Original evidence Ω.", filename="evidence.txt")
                )
                tx.result(admitted.material["id"])
            request = {
                "source_ids": [admitted.material["id"]],
                "body": '{"input":"Ω\\noriginal","namespace_id":"source"}',
            }
            response = {
                "raw_text": "Paid output Ω\n  exact spacing.",
                "raw_json": {"provider": "retained", "opaque": "AAECAw=="},
            }
            call, _ = await ledger.reserve(
                source, "formation", request, 1, invocation_id="retained-paid"
            )
            await ledger.finish(source, call, response, {"input_tokens": 7, "output_tokens": 3})
            memory = await cognition.adopt(
                source,
                AdoptCognition(
                    operation_id="adopt-paid",
                    body=MemoryBody(text="Retained paid output."),
                    evidence=(EvidenceLink(observation_id=admitted.event["id"]),),
                    producer_call_id=call,
                ),
            )
            exported = await bundles.export_zip(source)
            original = decode(exported)
            assert original.calls[0].request["body"] == request["body"]
            assert original.calls[0].result == response
            copied = await bundles.install_zip(receiver, exported, "copy")
            copied_call = copied["mapping"][call]
            async with authority.read(receiver) as view:
                retained = (await ledger.export_in(view, receiver, (copied_call,)))[0]
                assert retained.request == original.calls[0].request and retained.result == response
                assert retained.origin.namespace_id == source.namespace_id
                assert retained.source_ids == (copied["mapping"][admitted.material["id"]],)
                bindings = await ledger.export_invocations_in(view, source)
            async with authority.write(
                receiver, "custody", operation_kind="model.custody", payload={}
            ) as tx:
                report = await ledger.restore_invocations_in(
                    tx,
                    receiver,
                    bindings,
                    namespace_mapping={source.namespace_id: receiver.namespace_id},
                    caller_mapping={source.caller_id: receiver.caller_id},
                    call_mapping={call: copied_call},
                )
                tx.result(report)
            assert report.frozen == 1
            try:
                await ledger.reserve(
                    receiver, "formation", request, 1, invocation_id="retained-paid"
                )
            except DomainError as error:
                assert error.code == "model_invocation_unavailable"
            else:
                raise AssertionError("Transferred invocation must preserve its retired custody")
            copied_memory = await cognition.get(
                receiver,
                memory.reference.model_copy(update={"id": copied["mapping"][memory.reference.id]}),
            )
            assert copied_memory.producer_call_id == copied_call
        finally:
            await app.close()

    asyncio.run(scenario())
