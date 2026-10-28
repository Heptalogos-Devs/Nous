"""Credential revocation and scoped delegation across persisted request boundaries."""

import asyncio
import base64
import hashlib
import json
import secrets

import httpx
import pytest
import sqlalchemy as sa
from nous_wave.contracts import DomainError
from nous_wave.contracts.credentials import CreateCredential

from nous.identity.credentials import AuthenticationError, CredentialsService, credentials
from nous.platform.configuration import Settings
from nous.platform.persistence import Authority, OperationConflict
from nous.platform.persistence.schema import operations
from nous.transports.http import create_app


def test_tokens_are_disclosed_once_hashed_at_rest_and_revoked_across_restart(tmp_path):
    async def scenario():
        bootstrap = secrets.token_urlsafe(32)
        token_file = tmp_path / "admin.token"
        token_file.write_text(bootstrap)
        path = tmp_path / "authority.sqlite"
        authority = Authority(path)
        await authority.initialize()
        service = CredentialsService(authority, token_file)
        admin = await service.authenticate(bootstrap)
        command = CreateCredential(
            operation_id="new-key",
            name="Reader",
            namespace_ids=("allowed",),
            capabilities=("read",),
        )
        issued = await service.create(admin, command)
        assert issued.token is not None
        principal = await service.authenticate(issued.token)
        assert principal.caller_id != "instance-admin"
        principal.require("read")
        with pytest.raises(DomainError, match="write"):
            principal.require("write")
        with pytest.raises(DomainError, match="namespace"):
            principal.require_scope("different")
        principal.require_scope("allowed")
        repeated = await service.create(admin, command)
        assert repeated.record.id == issued.record.id and repeated.token is None
        with pytest.raises(OperationConflict):
            await service.create(admin, command.model_copy(update={"name": "Changed"}))
        async with authority.read() as view:
            digest, record = (
                await view.connection.execute(
                    sa.select(credentials.c.token_hash, credentials.c.record)
                )
            ).one()
            assert digest == hashlib.sha256(issued.token.encode()).hexdigest()
            assert issued.token not in str(record)
            receipts = (
                (await view.connection.execute(sa.select(operations.c.result_json))).scalars().all()
            )
            assert all(issued.token not in receipt for receipt in receipts)
        await service.revoke(admin, issued.record.id, "revoke")
        with pytest.raises(AuthenticationError):
            await service.authenticate(issued.token)
        assert (await service.create(admin, command)).record.revoked_at is not None
        await authority.close()
        authority = Authority(path)
        await authority.initialize()
        service = CredentialsService(authority, token_file)
        with pytest.raises(AuthenticationError):
            await service.authenticate(issued.token)
        assert (await service.authenticate(bootstrap)).caller_id == "instance-admin"
        token_file.write_text(secrets.token_urlsafe(32))
        with pytest.raises(AuthenticationError):
            await service.authenticate(bootstrap)
        await authority.close()

    asyncio.run(scenario())


def test_delegated_manager_cannot_expand_namespace_subject_sources_or_capabilities(tmp_path):
    async def scenario():
        token_file = tmp_path / "admin.token"
        token_file.write_text(secrets.token_urlsafe(32))
        authority = Authority(tmp_path / "authority.sqlite")
        await authority.initialize()
        service = CredentialsService(authority, token_file)
        admin = await service.authenticate(token_file.read_text())
        granted = await service.create(
            admin,
            CreateCredential(
                name="Scoped manager",
                namespace_ids=("namespace-a",),
                subject_ids=("subject-a",),
                source_ids=("source-a", "source-b"),
                capabilities=("read", "manage"),
            ),
        )
        assert granted.token is not None
        manager = await service.authenticate(granted.token)
        manager.require_scope("namespace-a", "subject-a")
        for namespace, subject in (
            ("namespace-b", "subject-a"),
            ("namespace-a", "subject-b"),
            ("namespace-a", None),
        ):
            with pytest.raises(DomainError):
                manager.require_scope(namespace, subject)
        assert manager.intersect_sources(("source-a", "ungranted")) == ("source-a",)
        assert manager.intersect_sources(()) == ()
        inherited = await service.create(manager, CreateCredential(name="Inherited"))
        assert inherited.record.namespace_ids == ("namespace-a",)
        assert inherited.record.subject_ids == ("subject-a",)
        assert inherited.record.source_ids == ("source-a", "source-b")
        for command in (
            CreateCredential(name="All namespaces", namespace_ids=None),
            CreateCredential(name="Another namespace", namespace_ids=("namespace-b",)),
            CreateCredential(name="All subjects", subject_ids=None),
            CreateCredential(name="Another subject", subject_ids=("subject-b",)),
            CreateCredential(name="All sources", source_ids=None),
            CreateCredential(name="Another source", source_ids=("source-c",)),
            CreateCredential(name="Write escalation", capabilities=("write",)),
        ):
            with pytest.raises(DomainError):
                await service.create(manager, command)
        foreign = await service.create(
            admin, CreateCredential(name="Other tenant", namespace_ids=("namespace-b",))
        )
        assert foreign.record.id not in {record.id for record in await service.list(manager)}
        with pytest.raises(DomainError):
            await service.revoke(manager, foreign.record.id, "cross-tenant-revoke")
        await service.revoke(manager, inherited.record.id, "owned-revoke")
        reader = await service.authenticate(foreign.token)
        with pytest.raises(DomainError):
            await service.list(reader)
        with pytest.raises(DomainError):
            await service.revoke(admin, "instance-admin", "protect-bootstrap")
        assert (await service.authenticate(token_file.read_text())).caller_id == "instance-admin"
        await authority.close()

    asyncio.run(scenario())


def test_http_readonly_key_queries_with_scope_and_revocation(tmp_path):
    async def scenario():
        settings = Settings(instance=tmp_path / "instance")
        app = create_app(settings)
        async with app.router.lifespan_context(app):
            bootstrap = settings.token_file.read_text()
            admin = {"Authorization": f"Bearer {bootstrap}"}
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="http://test"
            ) as client:
                for namespace in ("namespace-a", "namespace-b"):
                    response = await client.post(
                        "/api/namespaces", headers=admin, json={"id": namespace, "name": namespace}
                    )
                    assert response.status_code == 200
                subject_a = (
                    await client.post(
                        "/api/namespaces/namespace-a/subjects",
                        headers=admin,
                        json={"name": "Allowed"},
                    )
                ).json()["id"]
                subject_other = (
                    await client.post(
                        "/api/namespaces/namespace-a/subjects",
                        headers=admin,
                        json={"name": "Unpermitted"},
                    )
                ).json()["id"]
                subject_b = (
                    await client.post(
                        "/api/namespaces/namespace-b/subjects",
                        headers=admin,
                        json={"name": "Other namespace"},
                    )
                ).json()["id"]
                path = f"/api/namespaces/namespace-a/subjects/{subject_a}"
                document = {
                    "inputs": [
                        {
                            "text": "A rollback plan preserves the release.",
                            "filename": "release.txt",
                        }
                    ],
                    "llm_enabled": False,
                    "vectors": "disabled",
                }
                imported = await client.post(path + "/imports/files", headers=admin, json=document)
                assert imported.status_code == 200
                material_a = imported.json()["catalog"]["units"][0]["material_id"]
                observation_a = imported.json()["outputs"][0]["observation_id"]
                created = await client.post(
                    "/api/credentials",
                    headers=admin,
                    json={
                        "name": "Scoped HTTP reader",
                        "namespace_ids": ["namespace-a"],
                        "subject_ids": [subject_a],
                        "capabilities": ["read"],
                    },
                )
                assert created.status_code == 200
                issued = created.json()
                reader = {"Authorization": f"Bearer {issued['token']}"}
                query = {"text": "rollback"}
                response = await client.post(path + "/query", headers=reader, json=query)
                assert response.status_code == 200
                assert len(response.json()["hits"]) == 1
                sdk_response = await client.post(
                    f"/api/subjects/{subject_a}/query", headers=reader, json=query
                )
                assert sdk_response.status_code == 200
                assert len(sdk_response.json()["hits"]) == 1
                assert (
                    await client.post(path + "/imports/files", headers=reader, json=document)
                ).status_code == 403
                assert (await client.get("/api/credentials", headers=reader)).status_code == 403
                foreign = f"/api/namespaces/namespace-b/subjects/{subject_b}/query"
                assert (await client.post(foreign, headers=reader, json=query)).status_code == 403
                other = f"/api/namespaces/namespace-a/subjects/{subject_other}/query"
                assert (await client.post(other, headers=reader, json=query)).status_code == 403
                second = await client.post(
                    path + "/imports/files",
                    headers=admin,
                    json={
                        "inputs": [
                            {
                                "text": "Another rollback rule is independent.",
                                "filename": "other.txt",
                            }
                        ],
                        "llm_enabled": False,
                        "vectors": "disabled",
                    },
                )
                assert second.status_code == 200
                material_b = second.json()["catalog"]["units"][0]["material_id"]
                source_key = (
                    await client.post(
                        "/api/credentials",
                        headers=admin,
                        json={
                            "name": "Source-limited reader",
                            "namespace_ids": ["namespace-a"],
                            "subject_ids": [subject_a],
                            "source_ids": [material_a],
                            "capabilities": ["read"],
                        },
                    )
                ).json()
                source_headers = {"Authorization": f"Bearer {source_key['token']}"}
                source_result = await client.post(
                    path + "/query", headers=source_headers, json=query
                )
                assert source_result.status_code == 200 and len(source_result.json()["hits"]) == 1
                narrowed = (
                    base64.urlsafe_b64encode(
                        json.dumps(
                            [{"subject_id": subject_a, "observation_ids": [observation_a]}]
                        ).encode()
                    )
                    .decode()
                    .rstrip("=")
                )
                source_headers["x-nous-source-read"] = narrowed
                narrowed_result = await client.post(
                    path + "/query", headers=source_headers, json=query
                )
                assert (
                    narrowed_result.status_code == 200 and len(narrowed_result.json()["hits"]) == 1
                )
                expanded = (
                    base64.urlsafe_b64encode(
                        json.dumps(
                            [
                                {
                                    "subject_id": subject_a,
                                    "source_refs": [
                                        {
                                            "kind": "media",
                                            "reference": {
                                                "kind": "original",
                                                "material_id": material_b,
                                            },
                                        }
                                    ],
                                }
                            ]
                        ).encode()
                    )
                    .decode()
                    .rstrip("=")
                )
                source_headers["x-nous-source-read"] = expanded
                refused = await client.post(path + "/query", headers=source_headers, json=query)
                assert refused.status_code == 200 and refused.json()["hits"] == []
                revoked = await client.delete(
                    f"/api/credentials/{issued['record']['id']}", headers=admin
                )
                assert revoked.status_code == 200
                assert (
                    await client.post(path + "/query", headers=reader, json=query)
                ).status_code == 401
                assert (
                    await client.post(path + "/query", headers=admin, json=query)
                ).status_code == 200
                assert settings.token_file.read_text() == bootstrap

    asyncio.run(scenario())
