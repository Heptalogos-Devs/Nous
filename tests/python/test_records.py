"""Canonical directory pages preserve history while honoring live source authority."""

import asyncio

import pytest
from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    DomainError,
    EntityBody,
    EntityMention,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    MemoryChange,
    ReadRecords,
    RecordsQuery,
    SourceCommand,
    TimeFilter,
    TimeRange,
)

from nous.cognition.records import CognitiveRecords
from nous.data_management.sources import SourceService
from nous.identity.service import IdentityService
from nous.platform.persistence import Authority


def test_directory_snapshot_batch_states_and_source_authority(tmp_path):
    async def scenario():
        authority = Authority(tmp_path / "authority.sqlite")
        cognition = CognitiveRecords(authority)
        identity = IdentityService(authority)
        sources = SourceService(authority, cognition)
        await authority.initialize()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await identity.create_namespace(ctx, "Namespace", "namespace")
            await identity.create_subject(ctx, "Subject", "subject", subject_id="s")
            async with authority.write(ctx) as tx:
                source = await cognition.materials.admit_text_in(
                    tx, ctx, ImportDocument(text="Meeting on Tuesday", filename="meeting.txt")
                )
                evidence = (EvidenceLink(observation_id=source.event["id"]),)
                memory = await cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=MemoryBody(text="Meeting on Tuesday"),
                        evidence=evidence,
                    ),
                )
                person = await cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=EntityBody(
                            label="Organizer", mentions=(EntityMention(reference=memory.reference),)
                        ),
                        evidence=evidence,
                    ),
                )
                other = await cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=MemoryBody(text="Agenda is ready"),
                        evidence=evidence,
                    ),
                )
            expected = {record.reference for record in (memory, person, other)}
            first = await cognition.records(ctx, RecordsQuery(limit=1))
            assert first.next_cursor is not None
            frozen = await cognition.change_memory(
                ctx,
                MemoryChange(
                    reference=memory.reference,
                    action="freeze",
                ),
            )
            assert frozen.lifecycle.activation == "frozen"
            async with authority.read(ctx) as view:
                assert await cognition.automatic_in(view, ctx, memory.reference) is None
            seen = {first.items[0].record.reference}
            page = first
            while page.next_cursor is not None:
                page = await cognition.records(ctx, RecordsQuery(cursor=page.next_cursor, limit=1))
                assert page.coverage.snapshot_commit == first.coverage.snapshot_commit
                for entry in page.items:
                    assert entry.record.reference not in seen
                    seen.add(entry.record.reference)
            assert seen == expected
            assert page.coverage.complete
            with pytest.raises(DomainError, match="different query"):
                await cognition.records(
                    ctx, RecordsQuery(cursor=first.next_cursor, kinds=("memory",))
                )
            active = await cognition.records(ctx, RecordsQuery(kinds=("memory",)))
            assert [item.record.reference for item in active.items] == [other.reference]
            management = await cognition.records(
                ctx, RecordsQuery(purpose="management", states=("frozen",))
            )
            assert [item.record.reference for item in management.items] == [frozen.reference]
            filtered = await cognition.records(
                ctx,
                RecordsQuery(
                    entity_ids=(person.reference.id,),
                    source_ids=(source.material["id"],),
                    times=(
                        TimeFilter(
                            axis="recorded",
                            range=TimeRange(
                                start=person.times.recorded.coordinate,
                                end=person.times.recorded.coordinate + 1,
                            ),
                        ),
                    ),
                ),
            )
            assert [item.record.reference for item in filtered.items] == [person.reference]
            batch = await cognition.read_records(
                ctx,
                ReadRecords(
                    references=(memory.reference, person.reference, memory.reference),
                ),
            )
            assert tuple(record.reference for record in batch) == (
                memory.reference,
                person.reference,
                memory.reference,
            )
            restored = await cognition.change_memory(
                ctx,
                MemoryChange(
                    reference=frozen.reference,
                    action="restore",
                ),
            )
            assert restored.lifecycle.activation == "active"
            assert (
                await cognition.record_detail(ctx, memory.reference)
            ).entry.current_reference == restored.reference
            trashed = await cognition.change_memory(
                ctx,
                MemoryChange(
                    reference=restored.reference,
                    action="trash",
                ),
            )
            with pytest.raises(DomainError, match="management"):
                await cognition.record_detail(ctx, trashed.reference)
            assert (
                await cognition.record_detail(ctx, trashed.reference, purpose="management")
            ).entry.state == "deleted"
            denied = ctx.model_copy(update={"allowed_source_ids": ()})
            for purpose in ("current", "exact", "management"):
                assert not (
                    await cognition.records(denied, RecordsQuery(purpose=purpose, view="history"))
                ).items
                with pytest.raises(DomainError):
                    await cognition.read_records(
                        denied, ReadRecords(references=(memory.reference,), purpose=purpose)
                    )
            foreign = ctx.model_copy(update={"subject_id": "foreign"})
            await identity.create_subject(ctx, "Foreign", "foreign", subject_id="foreign")
            assert not (await cognition.records(foreign, RecordsQuery(purpose="management"))).items
            with pytest.raises(DomainError, match="unavailable"):
                await cognition.read_records(foreign, ReadRecords(references=(memory.reference,)))
            await sources.purge(
                ctx, SourceCommand(observation_ids=(source.event["id"],), reason="Erase source")
            )
            for purpose in ("current", "exact", "management"):
                assert not (
                    await cognition.records(ctx, RecordsQuery(purpose=purpose, view="history"))
                ).items
            assert not await cognition.history(ctx, memory.reference.id)
            with pytest.raises(DomainError, match="sources"):
                async with authority.read(ctx) as view:
                    await cognition.get_in(view, ctx, trashed.reference)
        finally:
            await authority.close()

    asyncio.run(scenario())


def test_actual_contacts_replay_and_knowledge_package_import(tmp_path):
    async def scenario():
        from nous_sdk.contracts import ContactMaterial, InstallBundle, MediaReference

        from nous.application import Application
        from nous.platform.configuration import Settings

        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await app.identity.create_namespace(ctx, "N", "namespace")
            await app.identity.create_subject(ctx, "S", "subject", subject_id="s")
            document = ImportDocument(text="Workshop is Thursday in room 503.")
            lease = await app.content.publish_bytes(document.inline_text().encode(), "text/plain")
            try:
                async with app.authority.write(ctx) as tx:
                    original = await app.material.admit_text_in(tx, ctx, document, lease)
            finally:
                await app.content.release(lease)
            assert not (await app.cognition.records(ctx, RecordsQuery())).items
            command = ContactMaterial(
                source=MediaReference(kind="material", id=original.material["id"]),
                external_event_id="host/read/first",
            )
            first = await app.imports.contact(ctx, command)
            assert (
                await app.imports.contact(
                    ctx, command.model_copy(update={"operation_id": "transport-retry"})
                )
                == first
            )
            second = await app.imports.contact(
                ctx, command.model_copy(update={"external_event_id": "host/read/next-day"})
            )
            assert second.observation_id != first.observation_id
            before = [await app.cognition.get(ctx, item.cognition[0]) for item in (first, second)]
            assert all(item.body.role == "experiential" for item in before)
            assert before[0].support_roots == before[1].support_roots
            assert before[0].source_roots != before[1].source_roots
            bundle = await app.bundles.export(ctx)
            await app.identity.create_subject(ctx, "Reader", "reader", subject_id="reader")
            receiver = ctx.model_copy(update={"subject_id": "reader"})
            await app.bundles.install(receiver, InstallBundle(bundle=bundle))
            imported = await app.cognition.records(receiver, RecordsQuery(kinds=("memory",)))
            assert len(imported.items) == 2
            assert all(
                item.record.body.role == "declarative"
                and item.record.body.contact_observation_id is None
                for item in imported.items
            )
            await app.sources.purge(
                ctx,
                SourceCommand(
                    observation_ids=(first.observation_id,), reason="Erase this encounter"
                ),
            )
            assert (await app.cognition.get(ctx, first.cognition[0])).purged
            assert not (await app.cognition.get(ctx, second.cognition[0])).purged
            with pytest.raises(DomainError) as purged_replay:
                await app.imports.contact(ctx, command)
            assert purged_replay.value.code in {"operation_result_purged", "source_unavailable"}
        finally:
            await app.close()

    asyncio.run(scenario())


def test_derived_contact_observation_uses_the_selected_source_authority(tmp_path):
    async def scenario():
        from nous_sdk.contracts import ContactMaterial, MediaReference, QueryRequest
        from nous_sdk.contracts.derivation import (
            CreateDerivedRegion,
            CreateRepresentation,
            DerivationInput,
            DerivationProducer,
            DerivedSelector,
            TextRepresentation,
        )

        from nous.application import Application
        from nous.material.derived_producer import digest
        from nous.platform.configuration import Settings

        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await app.identity.create_namespace(ctx, "N", "namespace")
            await app.identity.create_subject(ctx, "S", "subject", subject_id="s")
            text = "Private prefix. Shared text."
            async with app.authority.write(ctx) as tx:
                original = await app.material.admit_text_in(tx, ctx, ImportDocument(text=text))
                represented = await app.derivations.adopt_in(
                    tx,
                    ctx,
                    CreateRepresentation(
                        inputs=(
                            DerivationInput(
                                ordinal=0,
                                source=MediaReference(
                                    kind="material",
                                    id=original.material["id"],
                                    observation_id=original.event["id"],
                                ),
                            ),
                        ),
                        kind="extracted_text",
                        strategy="preprocess",
                        payload=TextRepresentation(text=text),
                        producer=DerivationProducer(
                            implementation="literal",
                            signature=digest(text),
                            config_digest=digest({}),
                        ),
                    ),
                )
                region = await app.derivations.adopt_region_in(
                    tx,
                    ctx,
                    CreateDerivedRegion(
                        representation=represented.reference,
                        selector=DerivedSelector(kind="text_span", start=16, end=len(text)),
                    ),
                )
            source = MediaReference(kind="derived_region", id=region.id)
            scoped = ctx.model_copy(update={"allowed_source_ids": (source.exact(),)})
            contact = await app.imports.contact(
                scoped, ContactMaterial(source=source, external_event_id="selected-contact")
            )
            result = await app.retrieval.query(
                scoped,
                QueryRequest(
                    domains=("observation",), reference=f"observation:{contact.observation_id}@1"
                ),
            )
            assert len(result.hits) == 1 and result.hits[0].object.source == source
            assert (
                await app.cognition.get(scoped, contact.cognition[0])
            ).body.text == "Shared text."
            with pytest.raises(DomainError, match="scope"):
                await app.material.read(scoped, original.material["id"])
        finally:
            await app.close()

    asyncio.run(scenario())
