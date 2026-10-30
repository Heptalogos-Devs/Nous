"""Import authority, scoped retrieval, logical restore and restart regression."""

import asyncio
from pathlib import Path

from nous_sdk.contracts import (
    AdmitFiles,
    BundleRequest,
    CallContext,
    ContextCommand,
    ContextFeedback,
    ImportDocument,
    InstallBundle,
    QueryRequest,
    SourceCommand,
)

from nous.cognition.service import CognitionService
from nous.context.service import ContextService
from nous.data_management.bundles import BundleService
from nous.data_management.sources import SourceService
from nous.identity.service import IdentityService
from nous.imports.service import ImportService
from nous.platform.persistence import Authority
from nous.retrieval.service import RetrievalService


class UnusedModels:
    async def form(self, *args, **kwargs):
        raise AssertionError("Literal ingestion must not invoke a formation model")

    async def embed(self, *args, **kwargs):
        raise AssertionError("Lexical retrieval must not invoke an embedding model")


def test_import_source_bundle_and_restart(tmp_path: Path):
    async def scenario():
        path = tmp_path / "authority.sqlite"
        authority = Authority(path)
        cognition = CognitionService(authority)
        imports = ImportService(authority, cognition, UnusedModels())
        retrieval = RetrievalService(authority, cognition, UnusedModels(), tmp_path / "index")
        bundles = BundleService(authority, cognition)
        sources = SourceService(authority, cognition)
        contexts = ContextService(authority, cognition)
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None,
            caller_id="owner",
            namespace_id="namespace",
            subject_id="subject",
        )
        try:
            await identity.create_namespace(ctx, "Research", "namespace-create")
            await identity.create_subject(
                ctx, "Researcher", "subject-create", subject_id=ctx.subject_id
            )
            command = AdmitFiles(
                operation_id="file-import",
                inputs=(
                    ImportDocument(
                        text="发布必须保留回滚策略。数据库迁移采用事务。", filename="release.md"
                    ),
                ),
                llm_enabled=False,
                vectors="disabled",
            )
            result = await imports.admit_files(ctx, command)
            assert result.file_successes[0].unit_outputs == {
                result.catalog.units[0].id: result.outputs[0].id
            }
            assert (
                result.outputs[0].observation_id
                in (await cognition.get(ctx, result.outputs[0].cognition[0])).source_roots
            )
            query = await retrieval.query(ctx, QueryRequest(text="回滚 $return(memory)"))
            assert len(query.hits) == 1
            assert query.coverage.complete
            denied = await retrieval.query(
                ctx.model_copy(update={"allowed_source_ids": ()}), QueryRequest(text="回滚")
            )
            assert denied.hits == ()
            admitted = await contexts.admit(
                ctx,
                ContextCommand(references=result.outputs[0].cognition, purpose="release decision"),
            )
            feedback = await contexts.record_feedback(
                ctx,
                ContextFeedback(
                    context_id=admitted["id"],
                    reference=result.outputs[0].cognition[0],
                    kind="actual_use",
                    description="used to select rollback plan",
                ),
            )
            assert feedback["kind"] == "actual_use"
            bundle = await bundles.export(ctx, BundleRequest())
            target = ctx.model_copy(update={"subject_id": "receiver"})
            await identity.create_subject(ctx, "Receiver", "receiver-create", subject_id="receiver")
            installed = await bundles.install(
                target, InstallBundle(operation_id="bundle-install", bundle=bundle)
            )
            assert installed["output_count"] == 1
            assert len((await retrieval.query(target, QueryRequest(text="回滚"))).hits) == 1
            observation_id = result.outputs[0].observation_id
            preview = await sources.preview(ctx, (observation_id,))
            assert len(preview.retiring) == 1
            await sources.trash(
                ctx,
                SourceCommand(
                    operation_id="trash",
                    observation_ids=(observation_id,),
                    reason="withdraw document",
                    expected_preview=preview.preview_token,
                ),
            )
            assert (await retrieval.query(ctx, QueryRequest(text="回滚"))).hits == ()
            assert len((await retrieval.query(target, QueryRequest(text="回滚"))).hits) == 1
            await sources.restore(
                ctx,
                SourceCommand(
                    operation_id="restore",
                    observation_ids=(observation_id,),
                    reason="restore document",
                ),
            )
            assert len((await retrieval.query(ctx, QueryRequest(text="回滚"))).hits) == 1
        finally:
            await retrieval.close()
            await authority.close()
        reopened = Authority(path)
        cognition = CognitionService(reopened)
        retrieval = RetrievalService(reopened, cognition, UnusedModels(), tmp_path / "index")
        await reopened.initialize()
        try:
            found = await retrieval.query(ctx, QueryRequest(text="回滚"))
            assert found.hits[0].object.body.text == "发布必须保留回滚策略。数据库迁移采用事务。"
        finally:
            await retrieval.close()
            await reopened.close()

    asyncio.run(scenario())


def test_purge_clears_history_receipts_and_blocks_late_adoption(tmp_path: Path):
    async def scenario():
        from nous_sdk.contracts import DomainError, MemoryChange
        from sqlalchemy import select

        from nous.cognition.schema import revisions
        from nous.material.service import MaterialService
        from nous.platform.content import ContentStore

        authority = Authority(tmp_path / "authority.sqlite")
        content = ContentStore(authority, tmp_path / "content")
        materials = MaterialService(authority, content)
        cognition = CognitionService(authority)
        imports = ImportService(authority, cognition, UnusedModels(), materials=materials)
        retrieval = RetrievalService(authority, cognition, UnusedModels(), tmp_path / "index")
        sources = SourceService(
            authority, cognition, content_store=content, invalidate=retrieval.invalidate
        )
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None,
            caller_id="owner",
            namespace_id="namespace",
            subject_id="subject",
        )
        try:
            await identity.create_namespace(ctx, "Research", "namespace-create")
            await identity.create_subject(
                ctx, "Researcher", "subject-create", subject_id=ctx.subject_id
            )
            command = AdmitFiles(
                operation_id="private-import",
                inputs=(ImportDocument(text="secret launch key alpha", filename="private.txt"),),
                llm_enabled=False,
                vectors="disabled",
            )
            result = await imports.admit_files(ctx, command)
            memory = result.outputs[0].cognition[0]
            frozen = await cognition.change_memory(
                ctx, MemoryChange(operation_id="freeze", reference=memory, action="freeze")
            )
            assert frozen.lifecycle.activation == "frozen"
            assert (await retrieval.query(ctx, QueryRequest(text="launch"))).hits == ()
            assert (
                len(
                    (
                        await retrieval.query(
                            ctx, QueryRequest(text="launch", include_inactive=True)
                        )
                    ).hits
                )
                == 1
            )
            restored = await cognition.change_memory(
                ctx,
                MemoryChange(operation_id="revive", reference=frozen.reference, action="restore"),
            )
            assert restored.lifecycle.activation == "active"
            unit = result.catalog.units[0]
            material = await materials.read(ctx, unit.material_id)
            object_path = content.path_for(material["content_hash"])
            assert object_path.is_file()
            await sources.purge(
                ctx,
                SourceCommand(
                    operation_id="purge-source",
                    observation_ids=(result.outputs[0].observation_id,),
                    reason="remove sensitive source",
                ),
            )
            assert not object_path.exists()
            async with authority.read(ctx) as view:
                history = [
                    value
                    for value in (
                        await view.connection.execute(select(revisions.c.record))
                    ).scalars()
                ]
                assert all(value["body"] is None and value["purged"] for value in history)
            assert (await imports.read_catalog(ctx, result.catalog.id)).units[0].text == ""
            try:
                await imports.admit_files(ctx, command)
            except DomainError as error:
                assert error.code == "operation_result_purged"
            else:
                raise AssertionError("A purged receipt must not replay source text")
            try:
                async with authority.write(
                    ctx,
                    "late-formation",
                    operation_kind="late.adoption",
                    scope=ctx.require_subject(),
                    payload={"material_id": unit.material_id},
                ) as tx:
                    await materials.observe_in(tx, ctx, unit.material_id, command.inputs[0])
                    tx.result({})
            except DomainError as error:
                assert error.code == "source_purged"
            else:
                raise AssertionError("A late model result must not revive a purged source")
        finally:
            await retrieval.close()
            await authority.close()

    asyncio.run(scenario())


def test_json_original_regions_addresses_attempts_and_artifact_purge(tmp_path: Path):
    async def scenario():
        import json

        from nous_sdk.contracts import BlobDocument, DomainError

        from nous.material.service import MaterialService
        from nous.platform.content import ContentStore

        content_bytes = (
            json.dumps(
                {
                    "external_id": "host-1",
                    "text": "原始记录包含独立来源。",
                    "tags": ["来源"],
                    "metadata": {"person": "李明"},
                },
                ensure_ascii=False,
            )
            + '\n{"external_id":"broken","text":2}\n'
        ).encode()
        authority = Authority(tmp_path / "authority.sqlite")
        content = ContentStore(authority, tmp_path / "content")
        materials = MaterialService(authority, content)
        cognition = CognitionService(authority)
        imports = ImportService(authority, cognition, UnusedModels(), materials=materials)
        retrieval = RetrievalService(
            authority, cognition, UnusedModels(), tmp_path / "index", materials=materials
        )
        sources = SourceService(authority, cognition, content_store=content)
        bundles = BundleService(authority, cognition, content)
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None,
            caller_id="owner",
            namespace_id="namespace",
            subject_id="subject",
        )
        try:
            await identity.create_namespace(ctx, "Research", "namespace-create")
            await identity.create_subject(
                ctx, "Researcher", "subject-create", subject_id=ctx.subject_id
            )
            lease = await content.publish_bytes(content_bytes, "application/x-ndjson")
            blob = BlobDocument(filename="records.jsonl", media_type=lease.media_type)
            async with authority.write(
                ctx,
                "upload-json",
                operation_kind="material.upload",
                payload={"hash": lease.content_hash},
            ) as tx:
                material = await materials.put_blob_in(tx, ctx, lease, blob)
                uploaded = await materials.observe_in(tx, ctx, material["id"], blob)
                tx.result({"material_id": material["id"]})
            await content.release(lease)
            result = await imports.admit_files(
                ctx,
                AdmitFiles(
                    operation_id="json-import",
                    inputs=(ImportDocument(material_id=material["id"], format="jsonl"),),
                    llm_enabled=False,
                    vectors="disabled",
                ),
            )
            assert result.catalog.original_record_count == 2
            assert len(result.catalog.errors) == 1
            assert result.file_successes == ()
            assert result.catalog.units[0].material_id == material["id"]
            memory = await cognition.get(ctx, result.outputs[0].cognition[0])
            assert memory.body.text == "原始记录包含独立来源。"
            assert memory.body.metadata == {"person": "李明"}
            attempts = await imports.read_attempts(ctx, result.catalog.id)
            assert (
                len(attempts) == 1
                and attempts[0].status == "succeeded"
                and attempts[0].job_id is None
            )
            unit = result.catalog.units[0]
            region_ctx = ctx.model_copy(update={"allowed_source_ids": (unit.source_region_id,)})
            region = await materials.read_region(region_ctx, unit.source_region_id)
            assert region.line_number == 1 and region.json_pointer == "/text"
            exact = await retrieval.query(
                region_ctx, QueryRequest(reference=f"source-region:{region.id}", domains=("media",))
            )
            assert exact.hits[0].object.text == memory.body.text
            from nous_sdk.contracts import MediaReference, SourceRegion

            selected = ctx.model_copy(
                update={
                    "allowed_source_ids": (region.id, uploaded["id"], memory.reference.exact()),
                    "source_constraints": ((region.id,), (uploaded["id"],)),
                }
            )
            async with authority.read(ctx) as view:
                chosen = await materials.media_source_in(
                    view,
                    selected,
                    MediaReference(
                        kind="source_region", id=region.id, observation_id=uploaded["id"]
                    ),
                )
                assert chosen[2] == uploaded["id"]
                try:
                    await materials.media_source_in(
                        view,
                        selected,
                        MediaReference(
                            kind="source_region", id=region.id, observation_id=unit.observation_id
                        ),
                    )
                except DomainError as error:
                    assert error.code == "source_unavailable"
                else:
                    raise AssertionError(
                        "A selected event cannot authorize another contact of the same bytes"
                    )
            wrong_event = selected.model_copy(
                update={"source_constraints": ((memory.reference.exact(),), (uploaded["id"],))}
            )
            assert (await retrieval.query(wrong_event, QueryRequest(text="独立来源"))).hits == ()
            denied_group = ctx.model_copy(
                update={
                    "source_constraints": ((),),
                    "allowed_source_ids": (memory.reference.exact(),),
                }
            )
            assert (await retrieval.query(denied_group, QueryRequest(text="独立来源"))).hits == ()
            from nous.material.text import source_region_text

            assert (
                source_region_text(
                    "private-prefix",
                    SourceRegion(id="prefix", material_id=material["id"], text_end=7),
                )
                == "private"
            )
            try:
                await materials.read(region_ctx, material["id"])
            except DomainError as error:
                assert error.code == "source_unavailable"
            else:
                raise AssertionError("Region access cannot expose the original JSONL")
            assert (
                await retrieval.query(ctx, QueryRequest(reference=memory.addresses.exact))
            ).hits[0].object.reference == memory.reference
            bundle = await bundles.export(ctx)
            receiver = ctx.model_copy(update={"subject_id": "receiver"})
            await identity.create_subject(ctx, "Receiver", "receiver-create", subject_id="receiver")
            installation = await bundles.install(
                receiver, InstallBundle(operation_id="install-json", bundle=bundle)
            )
            restored = await retrieval.query(
                receiver, QueryRequest(reference=memory.addresses.exact)
            )
            assert (
                restored.hits[0].object.reference.id == installation["mapping"][memory.reference.id]
            )
            assert restored.hits[0].object.addresses.exact != memory.addresses.exact
            scope = ctx.model_copy(update={"allowed_source_ids": (memory.reference.exact(),)})
            artifact_lease = await content.publish_bytes(memory.body.text.encode(), "text/plain")
            artifact_blob = BlobDocument(filename="selection.txt", media_type="text/plain")
            async with authority.write(
                scope,
                "artifact",
                operation_kind="material.artifact",
                payload={"reference": memory.reference.exact()},
            ) as tx:
                artifact = await materials.put_blob_in(
                    tx,
                    scope,
                    artifact_lease,
                    artifact_blob,
                    required_source_ids=(memory.reference.exact(),),
                )
                await materials.observe_in(tx, scope, artifact["id"], artifact_blob)
                tx.result({"material_id": artifact["id"]})
            await content.release(artifact_lease)
            assert (await materials.read(scope, artifact["id"]))["required_source_ids"] == [
                memory.reference.exact()
            ]
            artifact_path = content.path_for(artifact["content_hash"])
            await sources.purge(
                ctx,
                SourceCommand(
                    operation_id="purge-record",
                    observation_ids=(unit.observation_id,),
                    reason="withdraw record",
                ),
            )
            assert not artifact_path.exists()
            try:
                await materials.read(scope, artifact["id"])
            except DomainError as error:
                assert error.code == "source_unavailable"
            else:
                raise AssertionError("An artifact cannot retain its purged cognitive source")
        finally:
            await retrieval.close()
            await authority.close()

    asyncio.run(scenario())


def test_ego_seed_observation_reuse_and_attribute_support(tmp_path: Path):
    async def scenario():
        from nous_sdk.contracts import (
            AdoptCognition,
            CognitiveTimes,
            DomainError,
            EntityAttribute,
            EntityBody,
            EpistemicStance,
            EvidenceLink,
        )
        from nous_sdk.contracts.ego_model import AdoptSeed, SeedDocument, SeedFacet
        from nous_sdk.contracts.imports import FormObservation, TextSplitOptions
        from sqlalchemy import select, update

        from nous.identity.service import subjects
        from nous.material.service import MaterialService, observations
        from nous.platform.content import ContentStore
        from nous.platform.persistence.schema import operations

        authority = Authority(tmp_path / "authority.sqlite")
        content = ContentStore(authority, tmp_path / "content")
        materials = MaterialService(authority, content)
        cognition = CognitionService(authority, materials=materials)
        imports = ImportService(authority, cognition, UnusedModels(), materials=materials)
        sources = SourceService(authority, cognition, content_store=content)
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(
            credential_source_ids=None,
            caller_id="owner",
            namespace_id="namespace",
            subject_id="subject",
        )
        try:
            await identity.create_namespace(ctx, "People", "namespace-create")
            await identity.create_subject(
                ctx,
                "Researcher",
                "subject-create",
                subject_id=ctx.subject_id,
                capabilities=("memory", "ego", "social"),
            )
            seeded = await cognition.adopt_seed(
                ctx,
                AdoptSeed(
                    operation_id="seed",
                    documents=(
                        SeedDocument(
                            document=ImportDocument(text="我喜欢记录观察。", filename="soul.md"),
                            facets=(SeedFacet(facet="preference", description="喜欢记录观察"),),
                        ),
                        SeedDocument(
                            document=ImportDocument(
                                text="我在资料不足时保留疑问。", filename="values.md"
                            ),
                            facets=(
                                SeedFacet(facet="epistemics", description="资料不足时保留疑问"),
                            ),
                        ),
                    ),
                ),
            )
            assert len(seeded.facets) == 2 and len(seeded.memories) == 2
            assert len(await cognition.ego_profile(ctx)) == 2
            # Existing subjects may have persisted the former capability name.
            async with authority.writer.begin() as connection:
                await connection.execute(
                    update(subjects)
                    .where(subjects.c.id == ctx.subject_id)
                    .values(capabilities=["memory", "self", "social"])
                )
            assert len(await cognition.ego_profile(ctx)) == 2
            assert (await identity.list_subjects(ctx))[0]["capabilities"] == [
                "memory", "ego", "social"
            ]
            legacy = await identity.create_subject(
                ctx, "Legacy", "legacy-subject", capabilities=("memory", "self")
            )
            async with authority.writer.begin() as connection:
                import json

                old_result = {**legacy, "capabilities": ["memory", "self"]}
                await connection.execute(
                    update(operations)
                    .where(operations.c.operation_id == "legacy-subject")
                    .values(result_json=json.dumps(old_result))
                )
            replayed = await identity.create_subject(
                ctx, "Legacy", "legacy-subject", capabilities=("memory", "self")
            )
            assert replayed == legacy and legacy["capabilities"] == ["memory", "ego"]
            assert (
                await cognition.get(ctx, seeded.memories[0])
            ).body.epistemic.classification == "simulated"
            preview = await imports.preview_text_import(
                ctx,
                ImportDocument(text="一行\n二行\n三行\n四行\n五行"),
                TextSplitOptions(strategy_override="quote"),
            )
            assert len(preview.units) == 1 and not preview.units[0].requires_llm
            async with authority.read(ctx) as view:
                before = len((await view.connection.execute(select(observations.c.id))).all())
            formed = await imports.form_observation(
                ctx,
                FormObservation(
                    operation_id="form-existing",
                    observation_id=seeded.observation_ids[0],
                    llm_enabled=False,
                ),
            )
            async with authority.read(ctx) as view:
                after = len((await view.connection.execute(select(observations.c.id))).all())
            assert before == after
            assert (await cognition.get(ctx, formed.cognition[0])).source_roots == (
                seeded.observation_ids[0],
            )
            evidence = tuple(EvidenceLink(observation_id=value) for value in seeded.observation_ids)
            times = CognitiveTimes(formation=ctx.clock, recorded=ctx.clock)
            async with authority.write(
                ctx,
                "person",
                operation_kind="person.adopt",
                scope=ctx.require_subject(),
                payload={"observations": list(seeded.observation_ids)},
            ) as tx:
                entity = await cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=EntityBody(
                            entity_type="person",
                            label="李明",
                            attributes=(
                                EntityAttribute(
                                    predicate="private_note",
                                    value="仅第一份材料支持的资料",
                                    epistemic=EpistemicStance(classification="reported"),
                                    evidence=(evidence[0],),
                                ),
                                EntityAttribute(
                                    predicate="public_note",
                                    value="仍有第二份材料支持的资料",
                                    epistemic=EpistemicStance(classification="reported"),
                                    evidence=(evidence[1],),
                                ),
                            ),
                        ),
                        evidence=evidence,
                        times=times,
                    ),
                )
                tx.result(entity)
            await sources.purge(
                ctx,
                SourceCommand(
                    operation_id="purge-one",
                    observation_ids=(seeded.observation_ids[0],),
                    reason="remove first source",
                ),
            )
            retained = await cognition.get(ctx, entity.reference)
            assert retained.body.purged_attribute_count == 1
            assert [item.predicate for item in retained.body.attributes] == ["public_note"]
            try:
                await authority.receipt(
                    ctx,
                    "person",
                    operation_kind="person.adopt",
                    scope=ctx.require_subject(),
                    payload={"observations": list(seeded.observation_ids)},
                )
            except DomainError as error:
                assert error.code == "operation_result_purged"
            else:
                raise AssertionError("A historical receipt cannot retain the removed attribute")
        finally:
            await authority.close()

    asyncio.run(scenario())
