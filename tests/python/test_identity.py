"""Identity lifecycle retains data and scopes stable host bindings through actual HTTP."""

import asyncio
import base64
import hashlib
import json
import sqlite3
import zipfile
from datetime import UTC, datetime, timedelta
from io import BytesIO
from pathlib import Path

import httpx
import pytest
import sqlalchemy as sa
from nous_sdk import NousClient, NousError
from nous_sdk.contracts import (
    AdmitFiles,
    ContextCommand,
    ImportDocument,
    QueryRequest,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.context import ContextPrepare, ContextUsage
from nous_sdk.contracts.credentials import CreateCredential, RotateCredential
from nous_sdk.contracts.identity import (
    BindSubject,
    ChangeNamespace,
    ChangeSubject,
    IdentityDirectoryQuery,
    PurgeNamespace,
)
from nous_sdk.contracts.namespace_archives import MigrateSubject, RestoreNamespace
from nous_sdk.contracts.namespace_backups import RestoreNamespaceBackup
from nous_sdk.contracts.tuning import CreateTuning, TuningCase

from nous.identity.access import require_subject_in
from nous.identity.service import IdentityService
from nous.material.service import materials
from nous.platform.configuration import Settings
from nous.platform.execution import JobSpec
from nous.platform.models.ledger import calls
from nous.platform.persistence import Authority
from nous.transports.http import create_app


def test_sdk_lifecycle_capabilities_pagination_and_stable_binding(tmp_path):
    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            api = NousClient("http://test", credential_file=settings.token_file)
            await api.transport.http.aclose()
            api.transport.http = httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            )
            try:
                ns = await api.create_namespace("Lifecycle", namespace_id="lifecycle")
                one = await api.create_subject(ns.id, name="One", subject_id="one")
                await api.create_subject(ns.id, name="Two", subject_id="two")
                page = await api.subject_page(ns.id, IdentityDirectoryQuery(limit=1))
                assert [s.id for s in page.items] == ["one"] and page.next_cursor == "one"
                assert (
                    await api.subject_page(
                        ns.id, IdentityDirectoryQuery(limit=1, cursor=page.next_cursor)
                    )
                ).items[0].id == "two"
                capabilities = await api.identity_capabilities(ns.id, one.id)
                assert capabilities.supported == ("memory", "ego", "social", "motivation")
                assert capabilities.configured == capabilities.enabled == ("memory",)
                configured = await api.change_subject(
                    ns.id,
                    one.id,
                    ChangeSubject(action="configure_capabilities", capabilities=("memory", "ego")),
                )
                assert configured.capabilities == ("memory", "ego")
                with pytest.raises(NousError):
                    await api.change_subject(
                        ns.id, one.id, ChangeSubject(action="enable", expected_revision=1)
                    )
                bound = await api.bind_subject(
                    ns.id, BindSubject(external_id="host:room/one", subject_id=one.id)
                )
                assert (
                    await api.bind_subject(ns.id, BindSubject(external_id="host:room/one"))
                ).binding == bound.binding
                await api.change_subject(ns.id, one.id, ChangeSubject(action="disable"))
                with pytest.raises(NousError) as error:
                    await api.subject(one.id)
                assert error.value.code == "subject_disabled"
                assert (await api.subject(one.id, purpose="management")).id == one.id
                assert (
                    await api.identity_capabilities(ns.id, one.id, purpose="management")
                ).enabled == ()
                assert [s.id for s in (await api.subject_page(ns.id)).items] == ["two"]
                owner = app.state.application
                ctx = CallContext(
                    caller_id="instance-admin",
                    namespace_id=ns.id,
                    subject_id=one.id,
                    credential_source_ids=None,
                )
                async with owner.authority.read(ctx) as view:
                    with pytest.raises(DomainError, match="disabled"):
                        await require_subject_in(view, ctx)
                await api.change_subject(ns.id, one.id, ChangeSubject(action="trash"))
                assert (
                    await api.subject_page(
                        ns.id, IdentityDirectoryQuery(purpose="management", states=("trashed",))
                    )
                ).items[0].id == one.id
                restored = await api.change_subject(ns.id, one.id, ChangeSubject(action="restore"))
                assert restored.id == one.id and not restored.enabled
                await api.change_subject(ns.id, one.id, ChangeSubject(action="enable"))
                assert (await api.subject_binding(ns.id, "host:room/one")).subject.id == one.id
                memory = await api.create_subject(ns.id, name="Protocol", subject_id="protocol")
                await api.change_namespace(
                    ns.id, ChangeNamespace(action="bind_memory_subject", subject_id=memory.id)
                )
                assert (
                    await owner.identity.memory_subject(ctx.model_copy(update={"subject_id": None}))
                ).id == memory.id
                with pytest.raises(NousError):
                    await api.change_subject(
                        ns.id,
                        memory.id,
                        ChangeSubject(
                            action="configure_capabilities", capabilities=("memory", "ego")
                        ),
                    )
                await api.change_namespace(ns.id, ChangeNamespace(action="disable"))
                with pytest.raises(NousError) as error:
                    await api.subject(one.id)
                assert error.value.code == "namespace_disabled"
                await api.change_namespace(ns.id, ChangeNamespace(action="trash"))
                assert (await api.namespace(ns.id, purpose="management")).deleted
                await api.change_namespace(ns.id, ChangeNamespace(action="restore"))
                await api.change_namespace(ns.id, ChangeNamespace(action="enable"))
                assert (await api.subject(one.id)).capabilities == ("memory", "ego")
                issued = await api.create_credential(
                    CreateCredential(
                        name="Bound reader",
                        namespace_ids=(ns.id,),
                        subject_ids=(one.id,),
                        capabilities=("read", "manage"),
                    )
                )
                headers = {"Authorization": f"Bearer {issued.token}"}
                scoped = api.transport.http
                visible = await scoped.post(
                    "/api/namespaces/lifecycle/subjects/query", headers=headers, json={}
                )
                assert [s["id"] for s in visible.json()["items"]] == [one.id]
                forbidden = await scoped.get("/api/subjects/two", headers=headers)
                assert forbidden.status_code == 403
                host = await scoped.post(
                    "/api/namespaces/lifecycle/subject-bindings",
                    headers=headers,
                    json={"external_id": "host:stable", "subject_id": one.id},
                )
                assert host.status_code == 200
                rotated = await api.rotate_credential(
                    issued.record.id, RotateCredential(expected_revision=1)
                )
                repeated = await scoped.post(
                    "/api/namespaces/lifecycle/subject-bindings/read",
                    headers={"Authorization": f"Bearer {rotated.token}"},
                    params={"subject_id": one.id},
                    json={"external_id": "host:stable"},
                )
                assert repeated.status_code == 200 and repeated.json()["subject"]["id"] == one.id
            finally:
                await api.close()

    asyncio.run(scenario())


def test_legacy_identity_rows_migrate_in_place(tmp_path):
    database = tmp_path / "authority.sqlite"
    with sqlite3.connect(database) as connection:
        connection.executescript(
            "CREATE TABLE namespaces(id VARCHAR PRIMARY KEY,name VARCHAR NOT NULL); CREATE TABLE subjects(id VARCHAR PRIMARY KEY,namespace_id VARCHAR NOT NULL,name VARCHAR NOT NULL,capabilities JSON NOT NULL,commit_seq INTEGER NOT NULL); INSERT INTO namespaces VALUES('old','Old'); INSERT INTO subjects VALUES('retained','old','Retained','[\"memory\",\"self\"]',7);"
        )

    async def scenario():
        authority = Authority(database)
        await authority.initialize()
        identity = IdentityService(authority)
        ctx = CallContext(
            caller_id="admin", namespace_id="old", subject_id="retained", credential_source_ids=None
        )
        value = await identity.subject(ctx)
        assert (
            value.id == "retained"
            and value.commit_seq == 7
            and value.capabilities == ("memory", "ego")
        )
        await identity.change_subject(ctx, ChangeSubject(action="trash"))
        await authority.close()
        authority = Authority(database)
        await authority.initialize()
        value = await IdentityService(authority).subject(ctx, "management")
        assert value.id == "retained" and value.deleted
        await authority.close()

    asyncio.run(scenario())


def test_expired_credential_is_rejected_at_authentication(tmp_path, monkeypatch):
    import nous.identity.credentials as credential_module

    now = datetime.now(UTC)

    class Later(datetime):
        @classmethod
        def now(cls, tz=None):
            return now + timedelta(days=2)

    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            service = app.state.application.credentials
            admin = await service.authenticate(settings.token_file.read_text())
            expires = now + timedelta(days=1)
            issued = await service.create(
                admin, CreateCredential(name="Expiring", expires_at=expires)
            )
            assert issued.record.expires_at == expires
            await service.authenticate(issued.token)
            monkeypatch.setattr(credential_module, "datetime", Later)
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            ) as client:
                result = await client.get(
                    "/api/credentials/whoami", headers={"Authorization": f"Bearer {issued.token}"}
                )
                assert (
                    result.status_code == 401
                    and result.json()["error"]["code"] == "credential_expired"
                )

    asyncio.run(scenario())


def test_namespace_semantic_archive_and_subject_transfer_use_native_bundles(tmp_path):
    original = (Path(__file__).parents[2] / "README.md").read_text(encoding="utf-8")[:512]

    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            api = NousClient("http://test", credential_file=settings.token_file)
            await api.transport.http.aclose()
            api.transport.http = httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            )
            try:
                ns = await api.create_namespace("Archive source", namespace_id="archive-source")
                destination = await api.create_namespace(
                    "Migration receiver", namespace_id="transfer-target"
                )
                subject = await api.create_subject(ns.id, name="Stored", subject_id="stored")
                other = await api.create_subject(ns.id, name="Inactive", subject_id="inactive")
                admitted = await api.admit_documents(
                    ns.id,
                    subject.id,
                    AdmitFiles(
                        inputs=(ImportDocument(text=original, filename="README.md"),),
                        llm_enabled=False,
                        vectors="disabled",
                    ),
                )
                reference = admitted.outputs[0].cognition[0]
                await api.bind_subject(
                    ns.id, BindSubject(external_id="host:archive", subject_id=subject.id)
                )
                owner = app.state.application
                ns_ctx = CallContext(
                    caller_id="instance-admin", namespace_id=ns.id, credential_source_ids=None
                )
                await owner.configuration.set(
                    ns_ctx, "growth", {"mode": "review"}, "namespace"
                )
                await api.change_namespace(
                    ns.id, ChangeNamespace(action="bind_memory_subject", subject_id=subject.id)
                )
                await api.change_subject(ns.id, other.id, ChangeSubject(action="disable"))
                await api.change_namespace(ns.id, ChangeNamespace(action="trash"))
                archive = await api.export_namespace_archive(ns.id)
                assert archive.namespace.deleted and len(archive.subjects) == 2
                assert all(
                    entry.bundle.authority_commit_seq == archive.authority_commit_seq
                    for entry in archive.subjects
                )
                assert {entry.subject.id for entry in archive.subjects} == {subject.id, other.id}
                command = RestoreNamespace(
                    operation_id="restore-official-contract", archive=archive
                )
                restored = await api.restore_namespace_archive("restored-namespace", command)
                assert (
                    restored.namespace.deleted
                    and restored.namespace.memory_subject_id == restored.subjects[subject.id]
                )
                assert (
                    await api.restore_namespace_archive("restored-namespace", command)
                ) == restored
                inactive = await api.subject(restored.subjects[other.id], purpose="management")
                assert not inactive.enabled
                await api.change_namespace(restored.namespace.id, ChangeNamespace(action="restore"))
                copied_id = restored.installations[subject.id]["mapping"][reference.id]
                copied_ref = reference.model_copy(update={"id": copied_id})
                restored_ctx = ns_ctx.model_copy(
                    update={
                        "namespace_id": restored.namespace.id,
                        "subject_id": restored.subjects[subject.id],
                    }
                )
                copied = await owner.cognition.get(restored_ctx, copied_ref)
                assert copied.body.text == original
                assert (
                    await api.subject_binding(restored.namespace.id, "host:archive")
                ).subject.id == restored.subjects[subject.id]
                assert (await owner.configuration.effective(restored_ctx, "growth"))["value"][
                    "mode"
                ] == "review"
                await api.change_namespace(ns.id, ChangeNamespace(action="restore"))
                migration = MigrateSubject(
                    operation_id="transfer-source", target_namespace_id=destination.id
                )
                migrated = await api.migrate_subject(ns.id, subject.id, migration)
                assert (
                    migrated.subject.id != subject.id
                    and migrated.subject.namespace_id == destination.id
                )
                assert (await api.migrate_subject(ns.id, subject.id, migration)) == migrated
                assert (await api.subject(subject.id)).id == subject.id
                target_ref = reference.model_copy(
                    update={"id": migrated.installation["mapping"][reference.id]}
                )
                target_ctx = ns_ctx.model_copy(
                    update={"namespace_id": destination.id, "subject_id": migrated.subject.id}
                )
                assert (await owner.cognition.get(target_ctx, target_ref)).body.text == original
                # Complete backup additionally retains work state, pending jobs and unobserved uploads.
                source_ctx = ns_ctx.model_copy(update={"subject_id": subject.id})
                prepared_job = await owner.contexts.submit_prepare(
                    source_ctx,
                    ContextPrepare(
                        session_id="backup-session",
                        consumer_id="default",
                        instance_id="backup-host-instance",
                        usage=ContextUsage(purpose="Preserve source context"),
                    ),
                )
                await owner.execution.wait(source_ctx, prepared_job.job_id)
                work = await owner.contexts.admit(
                    source_ctx,
                    ContextCommand(references=(reference,), purpose="Retain active source context"),
                )
                unobserved_document = ImportDocument(text=original, filename="unobserved-README.md")
                lease = await owner.content.publish_bytes(original.encode(), "text/plain")

                try:
                    async with owner.authority.write(source_ctx) as tx:
                        unobserved = await owner.material.put_in(
                            tx, ns_ctx, unobserved_document, lease
                        )
                        waiting = await tx.enqueue(
                            JobSpec(
                                "nous.context.prepare",
                                "default",
                                args=(prepared_job.request_id,),
                                delay_seconds=3600,
                            )
                        )
                finally:
                    await owner.content.release(lease)
                ledger_request = {
                    "source_ids": [admitted.catalog.units[0].material_id],
                    "model": "offline-ledger-fixture",
                }
                ledger_call, _ = await owner.models.ledger.reserve(
                    source_ctx,
                    "embedding",
                    ledger_request,
                    10,
                    invocation_id="preserved-invocation",
                )
                await owner.models.ledger.finish(
                    source_ctx, ledger_call, {"value": "retained ledger result"}, {}
                )
                # Existing exact-reference input from test_tuning; no question text is generated.
                tuning = await api.create_tuning(
                    ns.id,
                    subject.id,
                    CreateTuning(
                        cases=(
                            TuningCase(
                                id="exact-reference",
                                query=QueryRequest(reference=reference.exact()),
                                relevance={reference.exact(): 1},
                            ),
                        ),
                        max_trials=1,
                    ),
                )
                await api.wait_success((tuning.job_id,), namespace_id=ns.id, subject_id=subject.id)
                backup = await api.create_namespace_backup(ns.id)
                assert (await api.namespace_backup(backup.backup_id)) == backup
                assert (await api.namespace_backups(ns.id)).items[0] == backup
                copied = await api.restore_namespace_backup(
                    "complete-copy",
                    RestoreNamespaceBackup(backup_id=backup.backup_id, operation_id="full-copy"),
                )
                assert (
                    copied.namespace.id == "complete-copy"
                    and copied.mapping[waiting.workflow_id] in copied.paused_jobs
                )
                copied_tuning = await api.tuning_task(
                    copied.namespace.id, copied.mapping[subject.id], copied.mapping[tuning.id]
                )
                copied_rounds = await api.tuning_rounds(
                    copied.namespace.id, copied.mapping[subject.id], copied.mapping[tuning.id]
                )
                mapped_reference = reference.model_copy(
                    update={"id": copied.mapping[reference.id]}
                ).exact()
                assert copied_tuning.request is not None and copied_tuning.request.cases[
                    0
                ].relevance == {mapped_reference: 1}
                assert copied_rounds.items[0].queries[0].ranking == (mapped_reference,)
                full_ctx = ns_ctx.model_copy(
                    update={
                        "namespace_id": copied.namespace.id,
                        "subject_id": copied.mapping[subject.id],
                    }
                )
                assert (await owner.contexts.read(full_ctx, copied.mapping[work["id"]]))[
                    "purpose"
                ] == work["purpose"]
                with pytest.raises(DomainError, match="live contact"):
                    await owner.material.read(full_ctx, copied.mapping[unobserved["id"]])
                async with owner.authority.read(full_ctx) as view:
                    unobserved_copy = await view.connection.scalar(
                        sa.select(materials.c.record).where(
                            materials.c.id == copied.mapping[unobserved["id"]],
                            materials.c.namespace_id == full_ctx.namespace_id,
                        )
                    )
                    assert unobserved_copy["text"] == original
                paused_job = await owner.execution.status(
                    full_ctx, copied.mapping[waiting.workflow_id]
                )
                assert (
                    paused_job.status == "CANCELLED"
                    and paused_job.attributes["namespace_restore"]["requires_review"]
                )
                with pytest.raises(DomainError, match="Copied work"):
                    await owner.execution.resume(full_ctx, copied.mapping[waiting.workflow_id])
                assert await owner.namespace_backups.namespace_for_backup(backup.backup_id) == ns.id
                downloaded = bytearray()
                while True:
                    chunk = await api.download_namespace_backup(
                        backup.backup_id, offset=len(downloaded), max_bytes=16384
                    )
                    downloaded.extend(base64.b64decode(chunk.data_base64))
                    if chunk.complete:
                        break
                assert hashlib.sha256(downloaded).hexdigest() == backup.sha256
                with zipfile.ZipFile(BytesIO(downloaded)) as archive_zip:
                    snapshot = json.loads(archive_zip.read("data/namespace.json"))
                    assert (
                        "nous_api_credentials" not in snapshot["tables"]
                        and "nous_credential_events" not in snapshot["tables"]
                    )
                    assert all(
                        row.get("namespace_id", ns.id) == ns.id
                        for rows in snapshot["tables"].values()
                        for row in rows
                    )
                    assert (
                        snapshot["tables"]["cognitive_contexts"]
                        and snapshot["tables"]["workflow_status"]
                    )
                upload = await api.begin_namespace_backup_upload()
                assert (
                    await owner.namespace_backups.namespace_for_upload(upload.upload_id)
                    == "instance"
                )
                partial = await api.upload_namespace_backup_chunk(
                    upload.upload_id, 0, bytes(downloaded[:100])
                )
                with pytest.raises(NousError):
                    await api.upload_namespace_backup_chunk(
                        upload.upload_id, 0, bytes(downloaded[:100])
                    )
                await api.upload_namespace_backup_chunk(
                    upload.upload_id, partial.next_offset, bytes(downloaded[100:])
                )
                with pytest.raises(NousError):
                    await api.complete_namespace_backup_upload(upload.upload_id, "0" * 64)
                assert (
                    await api.complete_namespace_backup_upload(upload.upload_id, backup.sha256)
                    == backup
                )
                abandoned = await api.begin_namespace_backup_upload()
                await api.upload_namespace_backup_chunk(abandoned.upload_id, 0, b"partial archive")
                await api.abort_namespace_backup_upload(abandoned.upload_id)
                purge_command = PurgeNamespace(operation_id="permanent-copy-removal")
                with pytest.raises(NousError, match="recycle bin"):
                    await api.purge_namespace(copied.namespace.id, purge_command)
                copy_backup = await api.create_namespace_backup(copied.namespace.id)
                await api.change_namespace(copied.namespace.id, ChangeNamespace(action="trash"))
                removed = await api.purge_namespace(copied.namespace.id, purge_command)
                assert removed.deleted_rows > 0 and removed.namespace_id == copied.namespace.id
                assert await api.purge_namespace(copied.namespace.id, purge_command) == removed
                with pytest.raises(NousError):
                    await api.namespace(copied.namespace.id, purpose="management")
                with pytest.raises(NousError):
                    await api.namespace_backup(copy_backup.backup_id)
                with pytest.raises(NousError, match="permanently removed"):
                    await api.create_namespace("Reused", namespace_id=copied.namespace.id)
                assert (await owner.cognition.get(source_ctx, reference)).body.text == original
                async with owner.authority.read() as view:
                    assert (
                        await view.connection.exec_driver_sql("PRAGMA foreign_key_check")
                    ).all() == []
            finally:
                await api.close()

        restored_settings = Settings(instance=tmp_path / "restored-instance")
        restored_app = create_app(restored_settings)
        async with restored_app.router.lifespan_context(restored_app):
            restored_api = NousClient("http://test", credential_file=restored_settings.token_file)
            await restored_api.transport.http.aclose()
            restored_api.transport.http = httpx.AsyncClient(
                transport=httpx.ASGITransport(app=restored_app), base_url="http://test"
            )
            try:
                upload = await restored_api.begin_namespace_backup_upload()
                await restored_api.upload_namespace_backup_chunk(
                    upload.upload_id, 0, bytes(downloaded)
                )
                await restored_api.complete_namespace_backup_upload(upload.upload_id, backup.sha256)
                preserved = await restored_api.restore_namespace_backup(
                    ns.id,
                    RestoreNamespaceBackup(
                        backup_id=backup.backup_id,
                        operation_id="preserve-installation-identity",
                        preserve_ids=True,
                    ),
                )
                assert preserved.mapping[subject.id] == subject.id
                restored_owner = restored_app.state.application
                assert (
                    await restored_owner.cognition.get(source_ctx, reference)
                ).body.text == original
                async with restored_owner.authority.read(source_ctx) as view:
                    assert (
                        await view.connection.scalar(
                            sa.select(sa.func.count())
                            .select_from(calls)
                            .where(calls.c.namespace_id == ns.id)
                        )
                        == 0
                    )
                existing_call, cached_result = await restored_owner.models.ledger.reserve(
                    source_ctx,
                    "embedding",
                    ledger_request,
                    10,
                    invocation_id="preserved-invocation",
                )
                assert existing_call == ledger_call and cached_result == {
                    "value": "retained ledger result"
                }
                await restored_owner.execution.resume(source_ctx, waiting.workflow_id)
                await restored_owner.execution.wait(source_ctx, waiting.workflow_id)
                await restored_api.delete_namespace_backup(backup.backup_id)
                assert (await restored_api.namespace_backups(ns.id)).items == ()
            finally:
                await restored_api.close()

    asyncio.run(scenario())
