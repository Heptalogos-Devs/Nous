"""Source lifecycle acts on support roots; historical cognition is retained."""

import json
from collections.abc import Awaitable, Callable
from hashlib import sha256
from uuid import uuid4

from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    SourceCommand,
    SourcePreview,
    SourceResult,
)
from pydantic import JsonValue
from sqlalchemy import select, update

from nous.cognition.schema import revisions, vectors
from nous.cognition.service import CognitionService
from nous.identity.service import require_subject_in
from nous.material.service import materials, observations, require_observation_in, source_permitted
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority, ReadView, WriteTx

from . import source_log
from .artifacts import purge_artifacts_in
from .schema import source_operations


class SourceService:
    get_operation = source_log.get_operation
    list_operations = source_log.list_operations

    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        purge_in: Callable[[WriteTx, tuple[str, ...]], Awaitable[None]] | None = None,
        content_store: ContentStore | None = None,
        invalidate: Callable[[], Awaitable[None]] | None = None,
        purge_context_in: Callable[
            [WriteTx, CallContext, tuple[CognitiveRef, ...]], Awaitable[None]
        ]
        | None = None,
    ) -> None:
        self.authority, self.cognition = authority, cognition
        self.purge_in = purge_in
        self.content_store = content_store
        self.invalidate = invalidate
        self.purge_context_in = purge_context_in

    async def list(self, ctx: CallContext):
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            rows = (
                await view.connection.execute(
                    select(observations).where(
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.subject_id,
                    )
                )
            ).mappings()
            return [
                {**row["record"], "active": row["active"]}
                for row in rows
                if source_permitted(ctx, row["material_id"], row["id"])
            ]

    async def preview_in(
        self, view: ReadView | WriteTx, ctx: CallContext, observation_ids: tuple[str, ...]
    ) -> SourcePreview:
        for observation_id in observation_ids:
            await require_observation_in(view, ctx, observation_id, active=False)
        roots = await self.cognition.active_roots_in(view, ctx)
        selected = set(observation_ids)
        affected: list[CognitiveRef] = []
        retiring: list[CognitiveRef] = []
        retained: list[CognitiveRef] = []
        rows = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.subject_id == ctx.subject_id,
                    revisions.c.head.is_(True),
                )
            )
        ).scalars()
        for value in rows:
            record = CognitiveRecord.model_validate(value)
            if selected.intersection(record.source_roots):
                affected.append(record.reference)
                if (
                    set(record.support_roots or record.source_roots)
                    .intersection(roots)
                    .difference(selected)
                ):
                    retained.append(record.reference)
                else:
                    retiring.append(record.reference)
        identity = {
            "sources": sorted(observation_ids),
            "live": sorted(roots),
            "affected": sorted(ref.exact() for ref in affected),
        }
        token = sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
        return SourcePreview(
            observation_ids=tuple(observation_ids),
            affected=tuple(affected),
            retiring=tuple(retiring),
            retained=tuple(retained),
            preview_token=token,
        )

    async def preview(self, ctx: CallContext, observation_ids: tuple[str, ...]) -> SourcePreview:
        async with self.authority.read(ctx) as view:
            return await self.preview_in(view, ctx, observation_ids)

    async def _change(self, ctx: CallContext, command: SourceCommand, kind: str) -> SourceResult:
        detached: list[str] = []
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind=f"sources.{kind}",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                result = SourceResult.model_validate(tx.replay_result)
            else:
                result, detached = await self.change_in(tx, ctx, command, kind)
                tx.result(result)
        if detached and self.content_store:
            await self.content_store.purge(ctx, detached)
        if self.invalidate:
            await self.invalidate()
        return result

    async def change_in(
        self, tx: WriteTx, ctx: CallContext, command: SourceCommand, kind: str
    ) -> tuple[SourceResult, list[str]]:
        detached: list[str] = []
        preview = await self.preview_in(tx, ctx, command.observation_ids)
        if command.expected_preview and command.expected_preview != preview.preview_token:
            raise DomainError("source_preview_changed", "Source support changed since the preview")
        for observation_id in command.observation_ids:
            row = await require_observation_in(tx, ctx, observation_id, active=False)
            if kind == "restore" and row["record"].get("purged"):
                raise DomainError("source_purged", "Purged source content cannot be restored")
            value = dict(row["record"])
            if kind == "purge":
                value["purged"] = True
            await tx.connection.execute(
                update(observations)
                .where(observations.c.id == observation_id)
                .values(active=kind == "restore", record=value)
            )
            if kind == "purge":
                # Shared bytes remain while any surviving source still needs them.
                peers = (
                    (
                        await tx.connection.execute(
                            select(observations.c.record).where(
                                observations.c.material_id == row["material_id"],
                                observations.c.id != observation_id,
                            )
                        )
                    )
                    .scalars()
                    .all()
                )
                if all(peer.get("purged") for peer in peers):
                    material = (
                        await tx.connection.execute(
                            select(materials.c.record).where(materials.c.id == row["material_id"])
                        )
                    ).scalar_one()
                    material = {**material, "text": None, "purged": True}
                    await tx.connection.execute(
                        update(materials)
                        .where(materials.c.id == row["material_id"])
                        .values(record=material)
                    )
                    if self.content_store:
                        detached.extend(await self.content_store.detach_in(tx, row["material_id"]))
                    if self.purge_in:
                        await self.purge_in(tx, (row["material_id"],))
            if kind in {"trash", "purge"}:
                # Derived assets are removed for retired objects; raw revisions remain historical.
                for reference in preview.retiring:
                    await tx.connection.execute(
                        vectors.delete().where(vectors.c.reference == reference.exact())
                    )
        if kind == "purge":
            if self.purge_in:
                await self.purge_in(tx, ())
            retired = await self.cognition.retire_unsupported_in(tx, ctx)
            detached.extend(await purge_artifacts_in(self, tx, ctx))
            retired += await self.cognition.retire_unsupported_in(tx, ctx)
            await self._purge_import_payloads_in(tx, ctx)
            if self.purge_context_in:
                await self.purge_context_in(tx, ctx, tuple(dict.fromkeys(retired)))
        source_operation_id = str(uuid4())
        result = SourceResult(
            operation_id=command.operation_id,
            source_operation_id=source_operation_id,
            observation_ids=command.observation_ids,
            affected=preview.affected,
            commit_seq=tx.commit_seq,
        )
        await tx.connection.execute(
            source_operations.insert().values(
                id=source_operation_id,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                record={
                    "caller_id": ctx.caller_id,
                    "requested_by": command.requested_by,
                    "kind": kind,
                    "reason": command.reason,
                    "result": result.model_dump(mode="json"),
                },
            )
        )
        return result, detached

    async def _purge_import_payloads_in(self, tx: WriteTx, ctx: CallContext) -> None:
        from nous.imports.schema import catalogs, inputs, requests, units

        source_rows = (
            (
                await tx.connection.execute(
                    select(observations.c.material_id, observations.c.record).where(
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.subject_id,
                    )
                )
            )
            .mappings()
            .all()
        )
        purged_materials = {
            row["material_id"] for row in source_rows if row["record"].get("purged")
        }.difference(row["material_id"] for row in source_rows if not row["record"].get("purged"))
        if not purged_materials:
            return
        owned = (
            (
                await tx.connection.execute(
                    select(catalogs).where(
                        catalogs.c.namespace_id == ctx.namespace_id,
                        catalogs.c.subject_id == ctx.subject_id,
                    )
                )
            )
            .mappings()
            .all()
        )
        catalog_ids = {row["id"] for row in owned}
        for row in (
            (
                await tx.connection.execute(
                    select(inputs).where(
                        inputs.c.catalog_id.in_(catalog_ids),
                        inputs.c.material_id.in_(purged_materials),
                    )
                )
            )
            .mappings()
            .all()
        ):
            value = {
                **row["record"],
                "purged": True,
                "document": {
                    **row["record"]["document"],
                    "text": None,
                    "material_id": row["material_id"],
                },
            }
            await tx.connection.execute(
                update(inputs).where(inputs.c.id == row["id"]).values(record=value)
            )
        for row in (
            (await tx.connection.execute(select(units).where(units.c.catalog_id.in_(catalog_ids))))
            .mappings()
            .all()
        ):
            if row["record"]["material_id"] in purged_materials:
                await tx.connection.execute(
                    update(units)
                    .where(units.c.id == row["id"])
                    .values(
                        record={
                            **row["record"],
                            "text": "",
                            "context": dict[str, JsonValue](),
                            "purged": True,
                        }
                    )
                )
        for row in owned:
            value = {
                **row["record"],
                "units": [
                    {**unit, "text": "", "context": dict[str, JsonValue](), "purged": True}
                    if unit["material_id"] in purged_materials
                    else unit
                    for unit in row["record"]["units"]
                ],
            }
            await tx.connection.execute(
                update(catalogs).where(catalogs.c.id == row["id"]).values(record=value)
            )
        for row in (await tx.connection.execute(select(requests))).mappings().all():
            context = row["record"].get("context", {})
            if (
                context.get("namespace_id") == ctx.namespace_id
                and context.get("subject_id") == ctx.subject_id
                and set(row["record"].get("material_ids", ())).intersection(purged_materials)
            ):
                await tx.connection.execute(
                    update(requests)
                    .where(requests.c.id == row["id"])
                    .values(record={**row["record"], "command": None, "purged": True})
                )
        if self.purge_in:
            await self.purge_in(tx, tuple(sorted(purged_materials)))

        def affected_receipt(value: JsonValue) -> bool:
            if isinstance(value, list):
                return any(affected_receipt(child) for child in value)
            if not isinstance(value, dict):
                return False
            if any(
                isinstance(value.get(key), str) and value.get(key) in purged_materials
                for key in ("material_id", "id")
            ) and any(value.get(key) is not None for key in ("text", "document")):
                return True
            reference = value.get("reference")
            if isinstance(reference, dict) and "body" in value:
                source_roots = value.get("support_roots") or value.get("source_roots")
                if (
                    isinstance(source_roots, list)
                    and source_roots
                    and not any(
                        root in live_observations for root in source_roots if isinstance(root, str)
                    )
                ):
                    return True
            return any(affected_receipt(child) for child in value.values())

        live_observations = {
            row["id"]
            for row in (
                await tx.connection.execute(
                    select(observations.c.id, observations.c.record).where(
                        observations.c.namespace_id == ctx.namespace_id
                    )
                )
            ).mappings()
            if not row["record"].get("purged")
        }
        await self.authority.redact_receipts_in(tx, affected_receipt)

    async def trash(self, ctx: CallContext, command: SourceCommand):
        return await self._change(ctx, command, "trash")

    async def restore(self, ctx: CallContext, command: SourceCommand):
        return await self._change(ctx, command, "restore")

    async def purge(self, ctx: CallContext, command: SourceCommand):
        return await self._change(ctx, command, "purge")
