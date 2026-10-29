"""Derived content preserves exact permissions, immutable revisions and physical sources."""

import asyncio
from pathlib import Path

import pytest
from nous_wave.contracts import (
    AdoptCognition,
    BlobDocument,
    CallContext,
    DomainError,
    EvidenceLink,
    InstallBundle,
    MediaReference,
    MemoryBody,
)
from nous_wave.contracts.derivation import (
    CreateDerivedRegion,
    CreateRepresentation,
    DerivationInput,
    DerivationProducer,
    DerivedSelector,
    DeriveMedia,
    MediaTransformation,
    TextRepresentation,
)
from nous_wave.contracts.media import PixelRect
from PIL import Image

from nous.application import Application
from nous.imports.media import MediaImportService
from nous.platform.configuration import Settings


def test_physical_derivation_exact_scope_revisions_and_withdrawal(tmp_path: Path):
    async def scenario():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.authority.initialize()
        await app.content.initialize()
        ctx = CallContext(
            credential_source_ids=None,
            caller_id="owner",
            namespace_id="research",
            subject_id="subject",
        )
        await app.identity.create_namespace(ctx, "Research", "namespace")
        await app.identity.create_subject(ctx, "Owner", "subject", subject_id=ctx.subject_id)
        path = tmp_path / "image.png"
        image = Image.new("RGB", (12, 8), "red")
        image.putpixel((5, 3), (0, 255, 0))
        image.save(path)
        lease = await app.content.publish_file(path, "image/png")
        document = BlobDocument(filename="image.png", media_type="image/png")
        info = await app.media.probe(lease)
        try:
            async with app.authority.write(
                ctx, "capture", operation_kind="capture", payload={}
            ) as tx:
                material = await app.material.put_blob_in(tx, ctx, lease, document, info)
                first = await app.material.observe_in(tx, ctx, material["id"], document)
                second = await app.material.observe_in(tx, ctx, material["id"], document)
                tx.result(material)
        finally:
            await app.content.release(lease)
        service = MediaImportService(
            app.authority,
            app.material,
            app.cognition,
            app.models,
            app.content,
            app.media,
            app.derivations,
        )
        source = MediaReference(kind="material", id=material["id"], observation_id=first["id"])
        command = DeriveMedia(
            operation_id="crop",
            source=source,
            transformation=MediaTransformation(
                action="crop",
                pixel_rect=PixelRect(x=5, y=3, width=2, height=2),
            ),
        )
        (cropped,) = await service.derive(ctx, command)
        assert await service.derive(ctx, command) == (cropped,)
        assert cropped.source_roots == (first["id"],)
        assert cropped.payload is not None and cropped.payload.kind == "artifact"
        with Image.open(app.content.path_for(cropped.payload.content_hash)) as result:
            assert result.size == (2, 2) and result.getpixel((0, 0)) == (0, 255, 0)
        manual = DerivationProducer(
            signature="a" * 64, config_digest="b" * 64, implementation="manual annotation"
        )
        description = await app.derivations.adopt(
            ctx,
            CreateRepresentation(
                operation_id="describe",
                inputs=(DerivationInput(ordinal=0, source=source),),
                kind="image_description",
                payload=TextRepresentation(text="green pixel and red background"),
                producer=manual,
            ),
        )
        selection = await app.derivations.register_region(
            ctx,
            CreateDerivedRegion(
                operation_id="select",
                representation=description.reference,
                selector=DerivedSelector(kind="text_span", start=0, end=11),
            ),
        )
        selected_ctx = ctx.model_copy(
            update={"allowed_source_ids": (f"derived-region:{selection.id}",)}
        )
        selected_ref = MediaReference(kind="derived_region", id=selection.id)
        assert (await service.derived_input(selected_ctx, selected_ref)).data == b"green pixel"
        with pytest.raises(DomainError):
            await app.derivations.read(selected_ctx, description.reference)
        with pytest.raises(DomainError):
            await app.material.read(selected_ctx, material["id"])
        nested = await app.derivations.register_region(
            selected_ctx,
            CreateDerivedRegion(
                operation_id="nested",
                representation=description.reference,
                parent_id=selection.id,
                selector=DerivedSelector(kind="text_span", start=0, end=5),
            ),
        )
        assert nested.parent_id == selection.id
        assert description.addresses is not None
        resolved = await app.references.resolve(ctx, description.addresses.exact)
        assert resolved.reference.reference == description.reference.media()
        revised = await app.derivations.adopt(
            ctx,
            CreateRepresentation(
                operation_id="revise",
                id=description.reference.id,
                expected_revision=1,
                inputs=description.inputs,
                kind="image_description",
                payload=TextRepresentation(text="revised annotation"),
                producer=manual,
            ),
        )
        assert revised.previous == description.reference
        assert (
            await app.derivations.read(ctx, description.reference)
        ).payload == description.payload
        lineage = await app.derivations.lineage(selected_ctx, selected_ref)
        assert lineage.derived_regions == (selection,)
        assert lineage.representations == () and lineage.unavailable_inputs == (
            description.reference.media(),
        )
        memory = await app.cognition.adopt(
            ctx,
            AdoptCognition(
                operation_id="remember-derived",
                evidence=(EvidenceLink(observation_id=first["id"]),),
                body=MemoryBody(
                    text="the selected image and its annotation",
                    media=(cropped.reference.media(), selected_ref),
                ),
            ),
        )
        from nous.data_management.bundles import BundleService

        bundles = BundleService(app.authority, app.cognition, content_store=app.content)
        exported = await bundles.export(ctx)
        recipient = ctx.model_copy(update={"subject_id": "recipient"})
        await app.identity.create_subject(
            recipient, "Recipient", "recipient", subject_id=recipient.subject_id
        )
        installed = await bundles.install(
            recipient, InstallBundle(operation_id="receive", bundle=exported)
        )
        mapping = installed["mapping"]
        received_ref = cropped.reference.model_copy(update={"id": mapping[cropped.reference.id]})
        received = await app.derivations.read(recipient, received_ref)
        assert received.payload is not None and received.payload.kind == "artifact"
        assert received.payload.content_hash == cropped.payload.content_hash
        assert received.source_roots == (mapping[first["id"]],)
        assert (await app.material.read(recipient, received.payload.material_id))[
            "content_hash"
        ] == cropped.payload.content_hash
        received_memory = await app.cognition.get(
            recipient, memory.reference.model_copy(update={"id": mapping[memory.reference.id]})
        )
        assert isinstance(received_memory.body, MemoryBody)
        assert received_memory.body.media[0] == received_ref.media()
        narrowed = ctx.model_copy(update={"allowed_source_ids": ("another-source",)})
        with pytest.raises(DomainError):
            await service.derive(narrowed, command)
        with pytest.raises(DomainError):
            await app.derivations.adopt(
                narrowed,
                CreateRepresentation(
                    operation_id="describe",
                    inputs=(DerivationInput(ordinal=0, source=source),),
                    kind="image_description",
                    payload=description.payload,
                    producer=manual,
                ),
            )
        assert selection.selector is not None
        with pytest.raises(DomainError):
            await app.derivations.register_region(
                narrowed,
                CreateDerivedRegion(
                    operation_id="select",
                    representation=description.reference,
                    selector=selection.selector,
                ),
            )
        from nous.material.service import observations

        async with app.authority.write(
            ctx, "withdraw", operation_kind="withdraw", payload={}
        ) as tx:
            await tx.connection.execute(
                observations.update().where(observations.c.id == first["id"]).values(active=False)
            )
            tx.result(None)
        with pytest.raises(DomainError):
            await app.derivations.read(ctx, cropped.reference)
        with pytest.raises(DomainError):
            await app.material.read(ctx, cropped.payload.material_id)
        # A separate contact keeps the original alive, while its withdrawn derivative stays unavailable.
        assert (await app.material.read(ctx, material["id"]))["id"] == material["id"]
        assert second["id"] != first["id"]
        await app.network.close()
        await app.authority.close()

    asyncio.run(scenario())


def test_purge_follows_exact_input_contacts_without_erasing_other_revisions(tmp_path: Path):
    async def scenario():
        from nous_wave.contracts import SourceCommand
        from sqlalchemy import select

        from nous.data_management.sources import SourceService
        from nous.material.derived_schema import representations
        from nous.material.service import observations

        app = Application(Settings(instance=tmp_path / "instance"))
        await app.authority.initialize()
        await app.content.initialize()
        ctx = CallContext(
            credential_source_ids=None,
            caller_id="owner",
            namespace_id="research",
            subject_id="subject",
        )
        await app.identity.create_namespace(ctx, "Research", "namespace")
        await app.identity.create_subject(ctx, "Owner", "subject", subject_id=ctx.subject_id)
        path = tmp_path / "image.png"
        Image.new("RGB", (8, 8), "green").save(path)
        lease = await app.content.publish_file(path, "image/png")
        document = BlobDocument(filename="image.png", media_type="image/png")
        try:
            info = await app.media.probe(lease)
            async with app.authority.write(
                ctx, "source", operation_kind="capture", payload={}
            ) as tx:
                material = await app.material.put_blob_in(tx, ctx, lease, document, info)
                source = await app.material.observe_in(tx, ctx, material["id"], document)
                tx.result(None)
        finally:
            await app.content.release(lease)
        media = MediaImportService(
            app.authority,
            app.material,
            app.cognition,
            app.models,
            app.content,
            app.media,
            app.derivations,
        )
        original = MediaReference(kind="material", id=material["id"], observation_id=source["id"])
        from nous_wave.contracts import SourceRegion

        async with app.authority.write(
            ctx, "source-region", operation_kind="material.region", payload={}
        ) as tx:
            selected = await app.material.register_region_in(
                tx,
                ctx,
                SourceRegion(
                    id="image-region",
                    material_id=material["id"],
                    pixel_rect=PixelRect(x=0, y=0, width=2, height=2),
                ),
            )
            tx.result(selected)
        author = DerivationProducer(
            signature="a" * 64, config_digest="b" * 64, implementation="manual annotation"
        )
        full_and_region = await app.derivations.adopt(
            ctx,
            CreateRepresentation(
                operation_id="whole-and-region",
                inputs=(
                    DerivationInput(ordinal=0, source=original),
                    DerivationInput(
                        ordinal=1,
                        source=MediaReference(
                            kind="source_region", id=selected.id, observation_id=source["id"]
                        ),
                    ),
                ),
                kind="image_description",
                payload=TextRepresentation(
                    text="also contains information outside the selected region"
                ),
                producer=author,
            ),
        )
        memory = await app.cognition.adopt(
            ctx,
            AdoptCognition(
                operation_id="whole-derived-memory",
                body=MemoryBody(
                    text="uses the whole input", media=(full_and_region.reference.media(),)
                ),
                evidence=(EvidenceLink(observation_id=source["id"], region_id=selected.id),),
            ),
        )
        narrow = ctx.model_copy(update={"allowed_source_ids": (f"source-region:{selected.id}",)})
        with pytest.raises(DomainError):
            await app.cognition.get(narrow, memory.reference)
        (cropped,) = await media.derive(
            ctx,
            DeriveMedia(
                operation_id="crop",
                source=original,
                transformation=MediaTransformation(
                    action="crop", pixel_rect=PixelRect(x=0, y=0, width=4, height=4)
                ),
            ),
        )
        assert cropped.payload is not None and cropped.payload.kind == "artifact"
        async with app.authority.write(
            ctx, "repeat-contact", operation_kind="contact", payload={}
        ) as tx:
            first = str(
                await tx.connection.scalar(
                    select(observations.c.id).where(
                        observations.c.material_id == cropped.payload.material_id
                    )
                )
            )
            await app.material.observe_in(
                tx,
                ctx,
                cropped.payload.material_id,
                BlobDocument(filename="another delivery", media_type="image/png"),
            )
            tx.result(None)
        producer = DerivationProducer(
            signature="a" * 64, config_digest="b" * 64, implementation="manual annotation"
        )
        old = await app.derivations.adopt(
            ctx,
            CreateRepresentation(
                operation_id="from-contact",
                inputs=(
                    DerivationInput(
                        ordinal=0,
                        source=MediaReference(
                            kind="material", id=cropped.payload.material_id, observation_id=first
                        ),
                    ),
                ),
                kind="image_description",
                payload=TextRepresentation(text="private old description"),
                producer=producer,
            ),
        )
        downstream = await app.derivations.adopt(
            ctx,
            CreateRepresentation(
                operation_id="downstream",
                inputs=(DerivationInput(ordinal=0, source=old.reference.media()),),
                kind="summary",
                payload=TextRepresentation(text="downstream private text"),
                producer=producer,
            ),
        )
        newer = await app.derivations.adopt(
            ctx,
            CreateRepresentation(
                operation_id="from-original",
                id=old.reference.id,
                expected_revision=1,
                inputs=(DerivationInput(ordinal=0, source=original),),
                kind="image_description",
                payload=TextRepresentation(text="independent current description"),
                producer=producer,
            ),
        )
        region = await app.derivations.register_region(
            ctx,
            CreateDerivedRegion(
                operation_id="new-selection",
                representation=newer.reference,
                selector=DerivedSelector(kind="text_span", start=0, end=11),
            ),
        )
        sources = SourceService(
            app.authority,
            app.cognition,
            purge_in=app.derivations.purge_materials_in,
            content_store=app.content,
        )
        await sources.purge(
            ctx,
            SourceCommand(
                operation_id="purge-contact", observation_ids=(first,), reason="erase that delivery"
            ),
        )
        async with app.authority.read(ctx) as view:
            for reference in (old.reference, downstream.reference):
                raw = await view.connection.scalar(
                    select(representations.c.record).where(
                        representations.c.id == reference.id,
                        representations.c.revision == reference.revision,
                    )
                )
                assert raw["purged"] and raw["payload"] is None
        assert (await app.derivations.read(ctx, newer.reference)).payload == newer.payload
        assert not (await app.derivations.read_region(ctx, region.id)).purged
        assert (await app.material.read(ctx, cropped.payload.material_id))[
            "content_hash"
        ] == cropped.payload.content_hash
        from nous.platform.errors import NousError

        with pytest.raises(NousError) as erased:
            await app.models.ledger.reserve(ctx, "formation", {"source_ids": [first]}, 10)
        assert erased.value.code == "SourcePurged"
        await app.network.close()
        await app.authority.close()

    asyncio.run(scenario())
