"""Import authority, scoped retrieval, logical restore and restart regression."""
import asyncio
from pathlib import Path

from nous_wave.contracts import (
    AdmitFiles, BundleRequest, CallContext, ContextCommand, ContextFeedback,
    InstallBundle, ImportDocument, QueryRequest, SourceCommand,
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
        ctx = CallContext(caller_id="owner", namespace_id="namespace", subject_id="subject")
        try:
            await identity.create_namespace(ctx, "Research", "namespace-create")
            await identity.create_subject(ctx, "Researcher", "subject-create", subject_id=ctx.subject_id)
            command = AdmitFiles(operation_id="file-import", inputs=(ImportDocument(text="发布必须保留回滚策略。数据库迁移采用事务。", filename="release.md"),), llm_enabled=False, vectors="disabled")
            result = await imports.admit_files(ctx, command)
            assert result.file_successes[0].unit_outputs == {result.catalog.units[0].id: result.outputs[0].id}
            assert result.outputs[0].observation_id in (await cognition.get(ctx, result.outputs[0].cognition[0])).source_roots
            query = await retrieval.query(ctx, QueryRequest(text="回滚 $return(memory)"))
            assert len(query.hits) == 1
            assert query.coverage.complete
            denied = await retrieval.query(ctx.model_copy(update={"allowed_source_ids": ()}), QueryRequest(text="回滚"))
            assert denied.hits == ()
            admitted = await contexts.admit(ctx, ContextCommand(references=result.outputs[0].cognition, purpose="release decision"))
            feedback = await contexts.record_feedback(ctx, ContextFeedback(context_id=admitted["id"], reference=result.outputs[0].cognition[0], kind="actual_use", description="used to select rollback plan"))
            assert feedback["kind"] == "actual_use"
            bundle = await bundles.export(ctx, BundleRequest())
            target = ctx.model_copy(update={"subject_id": "receiver"})
            await identity.create_subject(ctx, "Receiver", "receiver-create", subject_id="receiver")
            installed = await bundles.install(target, InstallBundle(operation_id="bundle-install", bundle=bundle))
            assert installed["output_count"] == 1
            assert len((await retrieval.query(target, QueryRequest(text="回滚"))).hits) == 1
            observation_id = result.outputs[0].observation_id
            preview = await sources.preview(ctx, (observation_id,))
            assert len(preview.retiring) == 1
            await sources.trash(ctx, SourceCommand(operation_id="trash", observation_ids=(observation_id,), reason="withdraw document", expected_preview=preview.preview_token))
            assert (await retrieval.query(ctx, QueryRequest(text="回滚"))).hits == ()
            assert len((await retrieval.query(target, QueryRequest(text="回滚"))).hits) == 1
            await sources.restore(ctx, SourceCommand(operation_id="restore", observation_ids=(observation_id,), reason="restore document"))
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
        from nous_wave.contracts import DomainError, MemoryChange
        from nous.material.service import MaterialService
        from nous.platform.content import ContentStore
        from nous.cognition.service import revisions
        from sqlalchemy import select
        authority = Authority(tmp_path / "authority.sqlite")
        content = ContentStore(authority, tmp_path / "content")
        materials = MaterialService(authority, content)
        cognition = CognitionService(authority)
        imports = ImportService(authority, cognition, UnusedModels(), materials=materials)
        retrieval = RetrievalService(authority, cognition, UnusedModels(), tmp_path / "index")
        sources = SourceService(authority, cognition, content_store=content, invalidate=retrieval.invalidate)
        identity = IdentityService(authority)
        await authority.initialize()
        ctx = CallContext(caller_id="owner", namespace_id="namespace", subject_id="subject")
        try:
            await identity.create_namespace(ctx, "Research", "namespace-create")
            await identity.create_subject(ctx, "Researcher", "subject-create", subject_id=ctx.subject_id)
            command = AdmitFiles(operation_id="private-import", inputs=(ImportDocument(text="secret launch key alpha", filename="private.txt"),), llm_enabled=False, vectors="disabled")
            result = await imports.admit_files(ctx, command)
            memory = result.outputs[0].cognition[0]
            frozen = await cognition.change_memory(ctx, MemoryChange(operation_id="freeze", reference=memory, action="freeze"))
            assert frozen.lifecycle.activation == "frozen"
            assert (await retrieval.query(ctx, QueryRequest(text="launch"))).hits == ()
            assert len((await retrieval.query(ctx, QueryRequest(text="launch", include_inactive=True))).hits) == 1
            restored = await cognition.change_memory(ctx, MemoryChange(operation_id="revive", reference=frozen.reference, action="restore"))
            assert restored.lifecycle.activation == "active"
            unit = result.catalog.units[0]
            material = await materials.read(ctx, unit.material_id)
            object_path = content.path_for(material["content_hash"])
            assert object_path.is_file()
            await sources.purge(ctx, SourceCommand(operation_id="purge-source", observation_ids=(result.outputs[0].observation_id,), reason="remove sensitive source"))
            assert not object_path.exists()
            async with authority.read(ctx) as view:
                history = [value for value in (await view.connection.execute(select(revisions.c.record))).scalars()]
                assert all(value["body"] is None and value["purged"] for value in history)
            assert (await imports.read_catalog(ctx, result.catalog.id)).units[0].text == ""
            try:
                await imports.admit_files(ctx, command)
            except DomainError as error:
                assert error.code == "operation_result_purged"
            else:
                raise AssertionError("A purged receipt must not replay source text")
            try:
                async with authority.write(ctx, "late-formation", operation_kind="late.adoption", scope=ctx.require_subject(), payload={"material_id": unit.material_id}) as tx:
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
