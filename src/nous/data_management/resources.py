"""Versioned external files and HTTP documents use the normal content and network owners."""

from __future__ import annotations

import mimetypes
import os
from datetime import UTC, datetime
from hashlib import sha256
from io import BytesIO
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import urlsplit
from uuid import uuid4

import httpx2
from nous_sdk.contracts import (
    BlobDocument,
    CallContext,
    CognitiveTimes,
    DomainError,
    MediaReference,
    TimePoint,
)
from nous_sdk.contracts.resources import (
    ChangeResource,
    ReadResource,
    RegisterResource,
    ResourceLocation,
    ResourcePage,
    ResourceRecord,
    ResourceReference,
    ResourceSnapshot,
    ResourceTextMethod,
)
from pydantic import JsonValue
from sqlalchemy import JSON, Boolean, Column, Integer, String, Table, select

from nous.identity.access import require_namespace_in
from nous.platform.concurrency import owned_thread
from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

if TYPE_CHECKING:
    from nous.material.service import MaterialService
    from nous.platform.network.service import NetworkService


resources = Table(
    "external_resources",
    metadata,
    Column("id", String, primary_key=True),
    Column("revision", Integer, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    
    Column("head", Boolean, nullable=False, index=True),
    Column("location", JSON, nullable=False),
    Column("max_bytes", Integer, nullable=False),
    Column("record", JSON, nullable=False),
)


def _now() -> str:
    return datetime.now(UTC).isoformat()


class ResourceService:
    def __init__(
        self, authority: Authority, materials: MaterialService, network: NetworkService
    ) -> None:
        self.authority, self.materials, self.network = authority, materials, network

    @staticmethod
    def _validate_location(location: ResourceLocation) -> None:
        if location.kind == "http":
            parsed = urlsplit(location.address)
            if (
                parsed.scheme not in {"http", "https"}
                or not parsed.netloc
                or parsed.username
                or parsed.password
            ):
                raise DomainError(
                    "resource_location_invalid",
                    "HTTP resources require an HTTP(S) URL without embedded credentials",
                )
        elif location.file_root is None or not location.file_root.is_absolute():
            raise DomainError(
                "resource_root_required", "File resources require an absolute managed file root"
            )
        if location.credential_file and location.credential_env:
            raise DomainError("resource_credential_ambiguous", "Choose one credential reference")

    async def _row_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        resource_id: str,
        revision: int | None = None,
    ):
        await require_namespace_in(view, ctx)
        statement = select(resources).where(
            resources.c.id == resource_id,
            resources.c.namespace_id == ctx.namespace_id,
            
        )
        statement = (
            statement.where(resources.c.head.is_(True))
            if revision is None
            else statement.where(resources.c.revision == revision)
        )
        row = (await view.connection.execute(statement)).mappings().first()
        if row is None:
            raise DomainError("resource_not_found", "Resource is unavailable in this subject")
        return row

    async def _visible_in(
        self, view: ReadView | WriteTx, ctx: CallContext, record: ResourceRecord
    ) -> ResourceRecord:
        if ctx.source_groups() and record.snapshot is None:
            raise DomainError(
                "source_unavailable", "Resource has no source in the current read scope"
            )
        if record.snapshot:
            try:
                await self.materials.media_source_in(view, ctx, record.snapshot.source)
            except DomainError as error:
                if error.code not in {"source_unavailable", "source_purged"}:
                    raise
                if ctx.source_groups():
                    raise
                return record.model_copy(
                    update={
                        "snapshot": None,
                        "text": "",
                        "source_roots": (),
                        "support_roots": (),
                        "status": "unavailable",
                        "error_code": "source_unavailable",
                    }
                )
        return record

    async def get(
        self, ctx: CallContext, resource_id: str, revision: int | None = None
    ) -> ResourceRecord:
        async with self.authority.read(ctx) as view:
            return await self.get_in(view, ctx, resource_id, revision)

    async def get_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        resource_id: str,
        revision: int | None = None,
    ) -> ResourceRecord:
        current = ResourceRecord.model_validate(
            (await self._row_in(view, ctx, resource_id))["record"]
        )
        if not current.enabled:
            raise DomainError("resource_disabled", "Resource is disabled")
        row = await self._row_in(view, ctx, resource_id, revision)
        return await self._visible_in(view, ctx, ResourceRecord.model_validate(row["record"]))

    async def resolve_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        locator: str,
        as_of_commit: int | None = None,
    ) -> ResourceRecord:
        value = locator.removeprefix("resource:")
        identity, mark, version = value.rpartition("@")
        if mark and version.isdecimal():
            record = await self.get_in(view, ctx, identity, int(version))
            if as_of_commit is not None and record.commit_seq > as_of_commit:
                raise DomainError(
                    "reference_not_found", "Resource is outside the requested history"
                )
            return record
        candidates = await self.records_in(view, ctx, as_of_commit=as_of_commit)
        matches = [
            record
            for record in candidates
            if value.casefold() in {record.reference.id.casefold(), record.name.casefold()}
        ]
        if len(matches) != 1:
            raise DomainError(
                "resource_not_found" if not matches else "ambiguous_name",
                "Resource name must identify one accessible resource",
                candidates=[
                    {"name": record.name, "reference": record.reference.model_dump(mode="json")}
                    for record in matches
                ],
            )
        return matches[0]

    async def list(
        self, ctx: CallContext, *, cursor: str | None = None, limit: int = 100
    ) -> ResourcePage:
        if not 1 <= limit <= 1000:
            raise DomainError("invalid_limit", "Resource page size must be between 1 and 1000")
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            statement = select(resources.c.record).where(
                resources.c.namespace_id == ctx.namespace_id,
                
                resources.c.head.is_(True),
            )
            if cursor is not None:
                statement = statement.where(resources.c.id > cursor)
            result: list[ResourceRecord] = []
            more = False
            for raw in (
                await view.connection.execute(statement.order_by(resources.c.id))
            ).scalars():
                try:
                    record = await self._visible_in(view, ctx, ResourceRecord.model_validate(raw))
                except DomainError as error:
                    if error.code != "source_unavailable":
                        raise
                    continue
                if len(result) == limit:
                    more = True
                    break
                result.append(record)
            return ResourcePage(
                items=tuple(result), next_cursor=result[-1].reference.id if more else None
            )

    async def records_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        *,
        as_of_commit: int | None = None,
        history: bool = False,
    ) -> list[ResourceRecord]:
        statement = select(resources.c.record).where(
            resources.c.namespace_id == ctx.namespace_id
        )
        if as_of_commit is None and not history:
            statement = statement.where(resources.c.head.is_(True))
        records: list[ResourceRecord] = []
        heads: dict[str, ResourceRecord] = {}
        for raw in (await view.connection.execute(statement)).scalars():
            record = ResourceRecord.model_validate(raw)
            if as_of_commit is not None and record.commit_seq > as_of_commit:
                continue
            if not record.enabled:
                continue
            try:
                record = await self._visible_in(view, ctx, record)
            except DomainError as error:
                if error.code != "source_unavailable":
                    raise
                continue
            records.append(record)
            old = heads.get(record.reference.id)
            if old is None or old.reference.revision < record.reference.revision:
                heads[record.reference.id] = record
        return records if history else list(heads.values())

    async def _save_in(
        self,
        tx: WriteTx,
        previous: ResourceRecord,
        record: ResourceRecord,
        location: ResourceLocation,
        max_bytes: int,
    ) -> ResourceRecord:
        current = ResourceRecord.model_validate(
            (await self._row_in(tx, tx.ctx, previous.reference.id))["record"]
        )
        if current.reference != previous.reference:
            raise DomainError(
                "resource_revision_conflict", "Resource changed while its source was being read"
            )
        reference = ResourceReference(
            id=previous.reference.id, revision=previous.reference.revision + 1
        )
        now = TimePoint(coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000))
        record = record.model_copy(
            update={
                "reference": reference,
                "commit_seq": tx.commit_seq,
                "times": record.times.model_copy(update={"recorded": now}),
            }
        )
        await tx.connection.execute(
            resources.update()
            .where(resources.c.id == previous.reference.id, resources.c.head.is_(True))
            .values(head=False)
        )
        await tx.connection.execute(
            resources.insert().values(
                id=reference.id,
                revision=reference.revision,
                namespace_id=record.namespace_id,
                
                head=True,
                location=location.model_dump(mode="json"),
                max_bytes=max_bytes,
                record=record.model_dump(mode="json"),
            )
        )
        return record

    async def register(self, ctx: CallContext, command: RegisterResource) -> ResourceRecord:
        self._validate_location(command.location)
        if ctx.source_groups():
            raise DomainError(
                "resource_scope_forbidden",
                "Registering a new external source requires an unrestricted source scope",
            )
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="resource.register",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            await require_namespace_in(tx, ctx)
            if tx.replayed:
                return await self._visible_in(
                    tx, ctx, ResourceRecord.model_validate(tx.replay_result)
                )
            now = TimePoint(
                coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
            )
            record = ResourceRecord(
                reference=ResourceReference(id=str(uuid4()), revision=1),
                namespace_id=ctx.namespace_id,
                
                name=command.name,
                description=command.description,
                address=command.location.address,
                location_kind=command.location.kind,
                read_policy=command.read_policy,
                enabled=True,
                status="registered",
                text=command.description,
                times=CognitiveTimes(formation=now, observation=now, recorded=now),
                commit_seq=tx.commit_seq,
            )
            await tx.connection.execute(
                resources.insert().values(
                    id=record.reference.id,
                    revision=1,
                    namespace_id=ctx.namespace_id,
                    
                    head=True,
                    location=command.location.model_dump(mode="json"),
                    max_bytes=command.max_bytes,
                    record=record.model_dump(mode="json"),
                )
            )
            return tx.result(record)

    async def change(self, ctx: CallContext, command: ChangeResource) -> ResourceRecord:
        if command.location is not None:
            self._validate_location(command.location)
            if ctx.source_groups():
                raise DomainError(
                    "resource_scope_forbidden",
                    "Changing an external source location requires unrestricted source authorization",
                )
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="resource.change",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return ResourceRecord.model_validate(tx.replay_result)
            row = await self._row_in(tx, ctx, command.reference.id)
            current = ResourceRecord.model_validate(row["record"])
            await self._visible_in(tx, ctx, current)
            if current.reference != command.reference:
                raise DomainError("resource_revision_conflict", "Resource has a newer revision")
            update = {
                key: getattr(command, key)
                for key in ("name", "description", "enabled", "read_policy")
                if getattr(command, key) is not None
            }
            location = ResourceLocation.model_validate(row["location"])
            if command.location is not None and command.location != location:
                location = command.location
                update.update(
                    address=location.address,
                    location_kind=location.kind,
                    snapshot=None,
                    text=command.description
                    if command.description is not None
                    else current.description,
                    source_roots=(),
                    support_roots=(),
                    status="registered",
                    error_code=None,
                    checked_at=None,
                )
            if command.enabled is False:
                update["status"] = "disabled"
            elif command.enabled is True:
                update["status"] = (
                    "available" if update.get("snapshot", current.snapshot) else "registered"
                )
            return tx.result(
                await self._save_in(
                    tx,
                    current,
                    current.model_copy(update=update),
                    location,
                    command.max_bytes if command.max_bytes is not None else int(row["max_bytes"]),
                )
            )

    async def _fetch(
        self,
        ctx: CallContext,
        location: ResourceLocation,
        maximum: int,
        previous: ResourceSnapshot | None,
    ) -> tuple[bytes | None, str, str | None, str | None, str]:
        if location.kind == "file":
            assert location.file_root is not None
            file_root = location.file_root

            def read_file() -> tuple[bytes, str, str | None, str | None, str]:
                root = file_root.resolve(strict=True)
                candidate = Path(location.address)
                path = (candidate if candidate.is_absolute() else root / candidate).resolve(
                    strict=True
                )
                if not path.is_relative_to(root):
                    raise DomainError(
                        "resource_path_forbidden", "File resource escapes its registered root"
                    )
                if path.stat().st_size > maximum:
                    raise DomainError(
                        "resource_size_exceeded", "Resource exceeds its configured byte limit"
                    )
                with path.open("rb") as stream:
                    data = stream.read(maximum + 1)
                if len(data) > maximum:
                    raise DomainError(
                        "resource_size_exceeded", "Resource exceeds its configured byte limit"
                    )
                return (
                    data,
                    mimetypes.guess_type(path.name)[0] or "application/octet-stream",
                    None,
                    None,
                    sha256(data).hexdigest(),
                )

            return await owned_thread(read_file)
        headers: dict[str, str] = {}
        if location.credential_file:
            credential_file = location.credential_file
            token = await owned_thread(lambda: credential_file.read_text(encoding="utf-8").strip())
        elif location.credential_env:
            token = os.environ.get(location.credential_env, "")
        else:
            token = ""
        if location.credential_file or location.credential_env:
            if not token:
                raise DomainError(
                    "resource_credential_unavailable",
                    "Resource credential reference is unavailable",
                )
            headers["Authorization"] = "Bearer " + token
        if previous and previous.etag:
            headers["If-None-Match"] = previous.etag
        if previous and previous.last_modified:
            headers["If-Modified-Since"] = previous.last_modified
        async with (
            self.network.client(ctx) as client,
            client.stream("GET", location.address, headers=headers) as response,
        ):
            if response.status_code == 304 and previous:
                return (
                    None,
                    previous.media_type,
                    previous.etag,
                    previous.last_modified,
                    previous.version,
                )
            if response.status_code in {401, 403}:
                raise DomainError(
                    "resource_access_forbidden",
                    "External resource denied the read request",
                    status=response.status_code,
                )
            if response.status_code == 404:
                raise DomainError(
                    "resource_external_not_found", "External resource no longer exists"
                )
            if not response.is_success:
                raise DomainError(
                    "resource_http_error",
                    "External resource request failed",
                    status=response.status_code,
                )
            data = bytearray()
            async for block in response.aiter_bytes():
                if len(data) + len(block) > maximum:
                    raise DomainError(
                        "resource_size_exceeded", "Resource exceeds its configured byte limit"
                    )
                data.extend(block)
            raw = bytes(data)
            etag, last_modified = (
                response.headers.get("etag"),
                response.headers.get("last-modified"),
            )
            return (
                raw,
                response.headers.get("content-type", "application/octet-stream").split(";", 1)[0],
                etag,
                last_modified,
                etag or last_modified or sha256(raw).hexdigest(),
            )

    async def _text(
        self, data: bytes, media_type: str
    ) -> tuple[str, ResourceTextMethod, int | None, tuple[int, ...]]:
        if media_type in {"text/html", "application/xhtml+xml"}:
            from nous.imports.document_extraction import html_records

            records = await owned_thread(lambda: html_records(data.decode("utf-8-sig")))
            return (
                "\n\n".join(item.record.text for item in records if item.record),
                "html_body",
                None,
                (),
            )
        if media_type == "application/pdf":
            from nous.imports.document_extraction import pdf_records

            records = await owned_thread(lambda: pdf_records(BytesIO(data)))
            text = "\n\n".join(
                f"[page {item.page_number}]\n{item.record.text}" for item in records if item.record
            )
            pages = tuple(
                item.page_number for item in records if item.record and item.page_number is not None
            )
            return text, "pdf_text_layer", len(records), pages
        if media_type.startswith("text/") or "json" in media_type:
            return data.decode("utf-8-sig"), "original", None, ()
        return "", "none", None, ()

    async def read(
        self, ctx: CallContext, command: ReadResource, *, automatic: bool = False
    ) -> ResourceRecord:
        async with self.authority.read(ctx) as view:
            row = await self._row_in(view, ctx, command.resource_id)
            previous = await self._visible_in(
                view, ctx, ResourceRecord.model_validate(row["record"])
            )
            location, maximum = (
                ResourceLocation.model_validate(row["location"]),
                int(row["max_bytes"]),
            )
        if not previous.enabled:
            raise DomainError("resource_disabled", "Resource is disabled")
        if command.current == "none":
            return previous
        if automatic and previous.read_policy == "manual":
            if command.current == "required":
                raise DomainError(
                    "resource_policy_forbidden", "This resource requires an explicit read operation"
                )
            return previous
        if ctx.source_groups():
            raise DomainError(
                "resource_scope_forbidden",
                "Refreshing an external source requires its unrestricted source authorization",
            )
        receipt = await self.authority.receipt(
            ctx,
            command.operation_id,
            operation_kind="resource.read",
            scope=ctx.namespace_id,
            payload=command,
        )
        if receipt:
            saved = ResourceRecord.model_validate(receipt.result)
            async with self.authority.read(ctx) as view:
                saved = await self._visible_in(view, ctx, saved)
            if command.current == "required" and saved.error_code:
                raise DomainError(
                    saved.error_code, "The requested current resource version is unavailable"
                )
            return saved
        failure: str | None = None
        lease = None
        fetched_at = _now()
        method: ResourceTextMethod = previous.snapshot.text_method if previous.snapshot else "none"
        page_count = previous.snapshot.page_count if previous.snapshot else None
        text_pages = previous.snapshot.text_pages if previous.snapshot else ()
        try:
            data, media_type, etag, modified, version = await self._fetch(
                ctx, location, maximum, previous.snapshot
            )
            if data is not None:
                text, method, page_count, text_pages = await self._text(data, media_type)
            else:
                text = previous.text
            if data is not None:
                if self.materials.content_store is None:
                    raise DomainError(
                        "content_unavailable", "Resource originals require content storage"
                    )
                lease = await self.materials.content_store.publish_bytes(data, media_type)
        except DomainError as error:
            failure = error.code
            data, media_type, etag, modified, version, text = (
                None,
                "application/octet-stream",
                None,
                None,
                "",
                previous.text,
            )
        except OSError, httpx2.HTTPError, UnicodeError:
            failure = "resource_unreachable"
            data, media_type, etag, modified, version, text = (
                None,
                "application/octet-stream",
                None,
                None,
                "",
                previous.text,
            )
        try:
            async with self.authority.write(
                ctx,
                command.operation_id,
                operation_kind="resource.read",
                scope=ctx.namespace_id,
                payload=command,
            ) as tx:
                if tx.replayed:
                    saved = ResourceRecord.model_validate(tx.replay_result)
                else:
                    snapshot = previous.snapshot
                    if lease and (
                        snapshot is None
                        or snapshot.content_hash != lease.content_hash
                        or snapshot.media_type != media_type
                    ):
                        document = BlobDocument(
                            filename=Path(urlsplit(location.address).path).name or previous.name,
                            media_type=media_type,
                            source_type="external_resource",
                            source=location.address,
                            origin={
                                "resource_id": previous.reference.id,
                                "consumer": "external_resource_reader",
                                "caller_id": ctx.caller_id,
                            },
                        )
                        material = await self.materials.put_blob_in(tx, ctx, lease, document)
                        observation = await self.materials.observe_in(
                            tx, ctx, material["id"], document, kind="external_snapshot"
                        )
                        snapshot = ResourceSnapshot(
                            source=MediaReference(
                                kind="material", id=material["id"], observation_id=observation["id"]
                            ),
                            content_hash=lease.content_hash,
                            byte_len=lease.byte_len,
                            media_type=media_type,
                            fetched_at=fetched_at,
                            version=version,
                            etag=etag,
                            last_modified=modified,
                            text_method=method,
                            page_count=page_count,
                            text_pages=text_pages,
                        )
                    elif snapshot and failure is None:
                        snapshot = snapshot.model_copy(
                            update={
                                "fetched_at": fetched_at,
                                "version": version,
                                "etag": etag,
                                "last_modified": modified,
                                "text_method": method,
                                "page_count": page_count,
                                "text_pages": text_pages,
                            }
                        )
                    record = previous.model_copy(
                        update={
                            "snapshot": snapshot,
                            "checked_at": fetched_at,
                            "error_code": failure,
                            "text": text,
                            "status": "forbidden"
                            if failure and failure.endswith("forbidden")
                            else "unavailable"
                            if failure
                            else "available",
                            "source_roots": (snapshot.source.observation_id,)
                            if snapshot and snapshot.source.observation_id
                            else (),
                            "support_roots": (snapshot.source.observation_id,)
                            if snapshot and snapshot.source.observation_id
                            else (),
                        }
                    )
                    saved = tx.result(await self._save_in(tx, previous, record, location, maximum))
        finally:
            if lease and self.materials.content_store:
                await self.materials.content_store.release(lease)
        if command.current == "required" and saved.error_code:
            raise DomainError(
                saved.error_code, "The requested current resource version is unavailable"
            )
        return saved

    async def purge_materials_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        removed = set(material_ids)
        if not removed:
            return
        for row in (
            await tx.connection.execute(
                select(resources.c.id, resources.c.revision, resources.c.record)
            )
        ).mappings():
            record = ResourceRecord.model_validate(row["record"])
            if record.snapshot and record.snapshot.source.id in removed:
                redacted = record.model_copy(
                    update={
                        "snapshot": None,
                        "text": "",
                        "source_roots": (),
                        "support_roots": (),
                        "status": "purged",
                        "error_code": "source_purged",
                    }
                )
                await tx.connection.execute(
                    resources.update()
                    .where(resources.c.id == row["id"], resources.c.revision == row["revision"])
                    .values(record=redacted.model_dump(mode="json"))
                )

        def contains_snapshot(value: JsonValue) -> bool:
            if isinstance(value, list):
                return any(contains_snapshot(item) for item in value)
            if not isinstance(value, dict):
                return False
            snapshot = value.get("snapshot")
            if isinstance(snapshot, dict):
                source = snapshot.get("source")
                if (
                    isinstance(source, dict)
                    and source.get("kind") == "material"
                    and source.get("id") in removed
                ):
                    return True
            return any(contains_snapshot(item) for item in value.values())

        await self.authority.redact_receipts_in(tx, contains_snapshot)
