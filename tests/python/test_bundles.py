"""Real archive interoperability retains immutable sources and exact vector identity."""

import asyncio
from io import BytesIO
from pathlib import Path

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
    SubjectBundle,
)
from nous_sdk.contracts.bundles import BundleAssetOptions, PackageDescriptor
from PIL import Image
from sqlalchemy import select

from nous.cognition.schema import vectors
from nous.cognition.service import CognitionService
from nous.data_management.archive_read import decode
from nous.data_management.bundles import BundleService
from nous.data_management.portable_read import load
from nous.identity.service import IdentityService
from nous.imports.service import ImportService
from nous.material.service import MaterialService
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority


class LiteralModels:
    async def form(self, *args, **kwargs):
        raise AssertionError("Literal archive setup does not form with a model")

    async def embed(self, *args, **kwargs):
        raise AssertionError("Archive transfer does not replace adopted vector coordinates")


def test_external_portable_and_native_vector_roundtrip(tmp_path: Path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        content = ContentStore(authority, tmp_path / "content")
        materials = MaterialService(authority, content)
        cognition = CognitionService(authority, materials=materials)
        imports = ImportService(authority, cognition, LiteralModels(), materials=materials)
        bundles = BundleService(authority, cognition, content)
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None, caller_id="owner", namespace_id="ns", subject_id="sender"
        )
        try:
            await identity.create_namespace(ctx, "namespace", "namespace")
            await identity.create_subject(ctx, "sender", "sender", subject_id="sender")
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
            receiver = ctx.model_copy(update={"subject_id": "receiver"})
            await identity.create_subject(receiver, "receiver", "receiver", subject_id="receiver")
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
                SubjectBundle.model_validate(normalized).cognition[0].body.text
                == "Alice chooses coffee."
            )
            target = ctx.model_copy(update={"subject_id": "external"})
            await identity.create_subject(target, "external", "external", subject_id="external")
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
            await authority.close()

    asyncio.run(scenario())


def test_artifact_contact_inherits_inputs_and_withdrawal(tmp_path: Path):
    async def scenario():
        from nous_sdk.contracts import BlobDocument, MediaReference, SourceCommand

        from nous.data_management.sources import SourceService

        authority = Authority(tmp_path / "authority.sqlite")
        store = ContentStore(authority, tmp_path / "content")
        materials = MaterialService(authority, store)
        cognition = CognitionService(authority, materials=materials)
        imports = ImportService(authority, cognition, LiteralModels(), materials=materials)
        sources = SourceService(authority, cognition, content_store=store)
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None, caller_id="owner", namespace_id="ns", subject_id="subject"
        )
        try:
            await identity.create_namespace(ctx, "namespace", "namespace")
            await identity.create_subject(ctx, "subject", "subject", subject_id="subject")
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
            await authority.close()

    asyncio.run(scenario())
