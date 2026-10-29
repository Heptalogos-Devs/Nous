"""Derived media stays exact across SDK downloads, contexts, and source retirement."""

import asyncio
import io

import httpx
import pytest
from nous_wave import NousClient, NousError, SourceReadScope
from nous_wave.contracts import MediaReference, PixelRect, SourceCommand
from nous_wave.contracts.context import (
    ChangeResidents,
    ChangeWorkContext,
    ContextPrepare,
    ContextResidentAdmission,
    ContextUsage,
    MediaTarget,
    OpenContextSession,
    WorkContextContent,
)
from nous_wave.contracts.derivation import (
    CreateRepresentation,
    DerivationInput,
    DerivationProducer,
    DeriveMedia,
    MediaTransformation,
    TextRepresentation,
)
from nous_wave.source_scope import MediaSource
from PIL import Image
from sqlalchemy import select

from nous.context.models import preparations
from nous.material.derived_schema import representations
from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_sdk_derived_content_and_pure_media_context_purge(tmp_path):
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
                namespace = await client.create_namespace("derived HTTP")
                subject = await client.create_subject(namespace.id, name="media host")
                image = tmp_path / "image.png"
                Image.new("RGB", (12, 10), "green").save(image)
                receipt = await client.upload_file(
                    namespace.id, image, owner_subject_id=subject.id, media_type="image/png"
                )
                material = receipt.result.id
                observation = (await client.source_directory(namespace.id, subject.id))[0].id
                source = MediaReference(kind="material", id=material, observation_id=observation)
                command = DeriveMedia(
                    operation_id="actual-crop",
                    source=source,
                    transformation=MediaTransformation(
                        action="crop", pixel_rect=PixelRect(x=1, y=2, width=5, height=4)
                    ),
                )
                crop = (await client.derive_media(namespace.id, subject.id, command))[0]
                assert (await client.derive_media(namespace.id, subject.id, command))[0] == crop
                exact = crop.reference.media()
                scoped = client.with_source_read(
                    SourceReadScope(
                        subject_id=subject.id, source_refs=(MediaSource(reference=exact),)
                    )
                )
                downloaded = await scoped.download_derived(
                    namespace.id, subject.id, exact, tmp_path / "crop.png"
                )
                assert downloaded.reference == exact
                with Image.open(io.BytesIO(downloaded.output.read_bytes())) as result:
                    assert result.size == (5, 4)
                with pytest.raises(NousError):
                    await scoped.download_material(
                        material,
                        tmp_path / "forbidden.png",
                        namespace_id=namespace.id,
                        subject_id=subject.id,
                    )
                lineage = await scoped.media_lineage(namespace.id, subject.id, exact)
                assert lineage.source == exact
                assert lineage.unavailable_inputs
                second = await client.observe(subject.id, material)
                foreign_event = scoped.with_source_read(
                    SourceReadScope(subject_id=subject.id, observation_ids=(str(second["id"]),))
                )
                with pytest.raises(NousError):
                    await foreign_event.download_derived(
                        namespace.id, subject.id, exact, tmp_path / "wrong-event.png"
                    )
                synopsis = await client.adopt_representation(
                    namespace.id,
                    subject.id,
                    CreateRepresentation(
                        operation_id="host-media-synopsis",
                        kind="resource_synopsis",
                        strategy="preprocess",
                        inputs=(DerivationInput(ordinal=0, source=source),),
                        payload=TextRepresentation(text="MEDIA-CACHE-ERASE: exact source synopsis"),
                        producer=DerivationProducer(
                            signature="1" * 64,
                            implementation="host-synopsis",
                            config_digest="2" * 64,
                        ),
                    ),
                )
                synopsis_source = synopsis.reference.media()
                text_client = client.with_source_read(
                    SourceReadScope(
                        subject_id=subject.id,
                        source_refs=(MediaSource(reference=synopsis_source),),
                    )
                )
                work = await text_client.change_work_context(
                    namespace.id,
                    subject.id,
                    ChangeWorkContext(
                        operation_id="cached-work",
                        action="create",
                        content=WorkContextContent(
                            purpose="MEDIA-CACHE-ERASE purpose",
                            context_text="MEDIA-CACHE-ERASE notes",
                            unresolved_questions=("MEDIA-CACHE-ERASE question",),
                            constraints={"secret": "MEDIA-CACHE-ERASE"},
                            anchors=(MediaTarget(reference=synopsis_source),),
                        ),
                    ),
                )
                session = await text_client.open_context_session(
                    namespace.id,
                    subject.id,
                    OpenContextSession(operation_id="actual-session", session_id="host-session"),
                )
                resident_command = ChangeResidents(
                    operation_id="cached-resident",
                    session_id=session.id,
                    expected_revision=session.revision,
                    action="admit",
                    residents=(
                        ContextResidentAdmission(
                            target=MediaTarget(reference=synopsis_source),
                            entry_reason="MEDIA-CACHE-ERASE resident note",
                        ),
                    ),
                )
                await text_client.change_context_residents(
                    namespace.id, subject.id, resident_command
                )
                with pytest.raises(NousError):
                    await client.with_source_read(
                        SourceReadScope(subject_id=subject.id)
                    ).change_context_residents(namespace.id, subject.id, resident_command)
                prepared = await text_client.prepare_context(
                    namespace.id,
                    subject.id,
                    ContextPrepare(
                        operation_id="pure-media-prepare",
                        session_id="host-session",
                        consumer_id="reply",
                        instance_id="host:1",
                        usage=ContextUsage(purpose="reply"),
                        current_sources=(MediaTarget(reference=synopsis_source),),
                    ),
                )
                assert "MEDIA-CACHE-ERASE" in prepared.response.projection.segments[0].text
                command_source = SourceCommand(
                    operation_id="retire-media-contact",
                    observation_ids=(observation,),
                    reason="retire exact contact",
                    requested_by="HTTP integration",
                )
                trashed = await client.trash_sources(namespace.id, subject.id, command_source)
                assert (
                    await client.source_operation(namespace.id, subject.id, trashed.commit_seq)
                )["requested_by"] == "HTTP integration"
                assert (
                    await client.source_operations(
                        namespace.id, subject.id, before_commit_seq=trashed.commit_seq
                    )
                    == ()
                )
                await client.purge_sources(
                    namespace.id,
                    subject.id,
                    command_source.model_copy(update={"operation_id": "purge-media-contact"}),
                )
                with pytest.raises(NousError):
                    await text_client.representation(namespace.id, subject.id, synopsis.reference)
                service = app.state.application
                async with service.authority.read() as view:
                    stored = (
                        (await view.connection.execute(select(preparations.c.record)))
                        .scalars()
                        .all()
                    )
                    retired_rep = (
                        await view.connection.execute(
                            select(representations.c.record).where(
                                representations.c.id == synopsis.reference.id
                            )
                        )
                    ).scalar_one()
                assert retired_rep["purged"] and retired_rep["payload"] is None
                assert stored and "MEDIA-CACHE-ERASE" not in str(stored)
                retired_work = await client.work_context(namespace.id, subject.id, work.id)
                assert retired_work.purged and retired_work.state == "suspended"
                assert "MEDIA-CACHE-ERASE" not in str(retired_work)
                retired_session = await client.context_session(namespace.id, subject.id, session.id)
                assert retired_session.residents[0].state == "evicted"
                assert "MEDIA-CACHE-ERASE" not in str(retired_session)
                # The independent second contact keeps its original available.
                assert (await client.material(material)).id == material
            finally:
                await client.close()

    asyncio.run(scenario())
