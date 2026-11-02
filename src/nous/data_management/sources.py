"""Source lifecycle acts on support roots; historical cognition is retained."""

import json
from datetime import UTC, datetime
from hashlib import sha256
from itertools import batched
from typing import Literal
from uuid import uuid4

from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    MemoryBody,
    RecordsQuery,
    RetentionChange,
    SourceCommand,
    SourcePreview,
    SourceResult,
)
from nous_sdk.contracts.retention import RetentionExpectation
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.cognition.lifecycle import selectable
from nous.cognition.lifecycle import snapshot as retention_snapshot
from nous.cognition.records import CognitiveRecords
from nous.cognition.schema import revisions, vectors
from nous.data_management.lifecycle import DataLifecycleService
from nous.identity.access import require_namespace_in
from nous.material.service import materials, observations, require_observation_in, source_permitted
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority, ReadView, WriteTx

from . import source_log, source_selection
from .artifacts import purge_artifacts_in
from .schema import source_operations


class SourceService:
    resolve_selection_in = source_selection.resolve_in
    authorize_result_in = source_log.authorize_result_in
    get_operation = source_log.get_operation
    list_operations = source_log.list_operations

    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        content_store: ContentStore | None = None,
        *,
        lifecycle: DataLifecycleService,
    ) -> None:
        self.authority, self.cognition = (authority, cognition)
        self.content_store = content_store
        self.lifecycle = lifecycle

    async def purge_namespace_in(self, tx: WriteTx, ctx: CallContext) -> tuple[str, ...]:
        """Retire all supporting originals inside an admitted Namespace removal."""
        from nous.identity.access import require_namespace_in

        namespace = await require_namespace_in(tx, ctx, purpose="management")
        if ctx.source_groups() or namespace.state != "trashed" or (not namespace.purging):
            raise DomainError(
                "namespace_purge_scope_forbidden",
                "Original removal requires the admitted complete Namespace",
            )
        material_ids = tuple(
            str(value)
            for value in (
                await tx.connection.execute(
                    select(materials.c.id).where(materials.c.namespace_id == ctx.namespace_id)
                )
            ).scalars()
        )
        for row in (
            await tx.connection.execute(
                select(observations).where(observations.c.namespace_id == ctx.namespace_id)
            )
        ).mappings():
            await tx.connection.execute(
                observations.update()
                .where(observations.c.id == row["id"])
                .values(active=False, record={**row["record"], "purged": True})
            )
        await self.lifecycle.purge_model_inputs_in(tx, material_ids)
        detached: list[str] = []
        if self.content_store:
            for identifier in material_ids:
                detached.extend(await self.content_store.detach_in(tx, identifier))
        return tuple(dict.fromkeys(detached))

    async def list(self, ctx: CallContext):
        async with self.authority.read(ctx) as view:
            return await self.list_in(view, ctx)

    async def list_in(self, view: ReadView | WriteTx, ctx: CallContext):
        await require_namespace_in(view, ctx)
        rows = (
            await view.connection.execute(
                select(observations).where(observations.c.namespace_id == ctx.namespace_id)
            )
        ).mappings()
        return [
            {**row["record"], "active": row["active"]}
            for row in rows
            if source_permitted(ctx, row["material_id"], row["id"])
        ]

    async def preview_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        observation_ids: tuple[str, ...] = (),
        *,
        references: tuple[CognitiveRef, ...] = (),
        source_names: tuple[str, ...] = (),
        records: RecordsQuery | None = None,
        retention_expectations: tuple[RetentionExpectation, ...] = (),
        action: Literal["trash", "restore", "purge"] = "trash",
    ) -> SourcePreview:
        await require_namespace_in(view, ctx)
        await self.cognition.require_retention_preconditions_in(view, ctx, retention_expectations)
        observation_ids, objects = await self.resolve_selection_in(
            view, ctx, observation_ids, references, source_names, records
        )
        for observation_id in observation_ids:
            await require_observation_in(view, ctx, observation_id, active=False)
        roots = await self.cognition.source_roots_in(view, ctx)
        selected = set(observation_ids)
        direct = {record.reference.id: record for record in objects}
        purge_plan: dict[str, CognitiveRecord] = {}
        if action == "purge":
            plan = await self.cognition.retirement_plan_in(
                view, ctx, frozenset(direct), frozenset(observation_ids)
            )
            purge_plan = {record.reference.id: record for record in plan}
        affected: list[CognitiveRef] = []
        retiring: list[CognitiveRef] = []
        retained: list[CognitiveRef] = []
        rows = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id, revisions.c.head.is_(True)
                )
            )
        ).scalars()
        for value in rows:
            record = CognitiveRecord.model_validate(value)
            if (
                record.reference.id not in direct
                and record.reference.id not in purge_plan
                and (not selected.intersection(record.source_roots))
            ):
                continue
            if not await self.cognition.visible_in(view, ctx, record, purpose="management"):
                continue
            affected.append(record.reference)
            projected_roots = {
                oid: material for oid, material in roots.items() if oid not in selected
            }
            remains = (
                record.reference.id not in direct
                and record.reference.id not in purge_plan
                and await self.cognition.support_available_in(
                    view,
                    ctx,
                    record,
                    projected_roots,
                    withdrawn_observations=frozenset(selected)
                    if action == "trash"
                    else frozenset(),
                    purged_observations=frozenset(selected) if action == "purge" else frozenset(),
                    purged_cognition=frozenset(direct) if action == "purge" else frozenset(),
                )
            )
            available_before = selectable(record) and await self.cognition.visible_in(
                view, ctx, record, roots
            )
            if available_before and (remains or action == "restore"):
                retained.append(record.reference)
            elif available_before:
                retiring.append(record.reference)
        identity = {
            "action": action,
            "sources": sorted(observation_ids),
            "objects": sorted(record.reference.exact() for record in objects),
            "live": sorted(roots),
            "affected": sorted(ref.exact() for ref in affected),
        }
        token = sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
        return SourcePreview(
            observation_ids=observation_ids,
            references=tuple(record.reference for record in objects),
            affected=tuple(affected),
            retiring=tuple(retiring),
            retained=tuple(retained),
            preview_token=token,
            retention_states=tuple(
                retention_snapshot(record, int(datetime.now(UTC).timestamp() * 1000))
                for record in objects
                if record.reference.kind in {"memory", "association"}
            ),
        )

    async def preview(
        self,
        ctx: CallContext,
        observation_ids: tuple[str, ...] = (),
        *,
        references: tuple[CognitiveRef, ...] = (),
        source_names: tuple[str, ...] = (),
        records: RecordsQuery | None = None,
        retention_expectations: tuple[RetentionExpectation, ...] = (),
        action: Literal["trash", "restore", "purge"] = "trash",
    ) -> SourcePreview:
        async with self.authority.read(ctx) as view:
            return await self.preview_in(
                view,
                ctx,
                observation_ids,
                references=references,
                source_names=source_names,
                records=records,
                retention_expectations=retention_expectations,
                action=action,
            )

    async def _change(
        self, ctx: CallContext, command: SourceCommand, kind: Literal["trash", "restore", "purge"]
    ) -> SourceResult:
        detached: list[str] = []
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind=f"sources.{kind}",
            scope=ctx.namespace_id,
            payload=command.operation_payload(),
        ) as tx:
            await require_namespace_in(tx, ctx)
            if tx.replayed:
                result = SourceResult.model_validate(tx.replay_result)
                await self.authorize_result_in(tx, ctx, result)
            else:
                result, detached = await self.change_in(tx, ctx, command, kind)
                from nous.retrieval.maintenance import enqueue_index_in

                await enqueue_index_in(tx, ctx)
                tx.result(result)
        if detached and self.content_store:
            await self.content_store.purge(ctx, detached)
        return result

    async def change_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        command: SourceCommand,
        kind: Literal["trash", "restore", "purge"],
    ) -> tuple[SourceResult, list[str]]:
        detached: list[str] = []
        preview = await self.preview_in(
            tx,
            ctx,
            command.observation_ids,
            references=command.references,
            source_names=command.source_names,
            records=command.records,
            retention_expectations=command.retention_expectations,
            action=kind,
        )
        if command.expected_preview and command.expected_preview != preview.preview_token:
            raise DomainError("source_preview_changed", "Source support changed since the preview")
        changed_objects: list[CognitiveRef] = []
        if kind == "trash":
            changed_objects = await self.change_objects_in(
                tx, ctx, preview.references, kind, command
            )
        for observation_id in preview.observation_ids:
            row = await require_observation_in(tx, ctx, observation_id, active=False)
            if kind == "restore" and row["record"].get("purged"):
                raise DomainError("source_purged", "Purged source content cannot be restored")
            value = dict(row["record"])
            if kind == "trash" and row["active"]:
                value["trashed_at"] = tx.recorded_at
            elif kind == "restore":
                value.pop("trashed_at", None)
            if kind == "purge":
                if command.grace_hours is not None:
                    deleted_at = row["record"].get("trashed_at")
                    if row["active"] or not isinstance(deleted_at, str):
                        raise DomainError(
                            "deletion_time_unavailable",
                            "Source grace period requires its recorded deletion time",
                        )
                    if (
                        datetime.fromisoformat(tx.recorded_at) - datetime.fromisoformat(deleted_at)
                    ).total_seconds() < command.grace_hours * 3600:
                        raise DomainError(
                            "deletion_grace_pending",
                            "The source has not completed its deletion grace period",
                        )
                value["purged"] = True
                value.pop("statement", None)
            await tx.connection.execute(
                update(observations)
                .where(observations.c.id == observation_id)
                .values(active=kind == "restore", record=value)
            )
            if kind == "purge":
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
                    await self.lifecycle.purge_model_inputs_in(tx, (row["material_id"],))
            if kind in {"trash", "purge"}:
                for reference in preview.retiring:
                    await tx.connection.execute(
                        vectors.delete().where(vectors.c.reference == reference.exact())
                    )
        if kind == "purge":
            await self.lifecycle.purge_model_inputs_in(tx, ())
            retired = await self.cognition.retire_unsupported_in(tx, ctx)
            detached.extend(await purge_artifacts_in(self, tx, ctx))
            retired += await self.cognition.retire_unsupported_in(tx, ctx)
            await self._purge_import_payloads_in(tx, ctx)
            await self.lifecycle.purge_cognitive_references_in(
                tx, ctx, tuple(dict.fromkeys(retired))
            )
        if kind != "trash":
            changed_objects = await self.change_objects_in(
                tx, ctx, preview.references, kind, command
            )
        source_operation_id = str(uuid4())
        before_available = {ref.id: ref for ref in (*preview.retiring, *preview.retained)}
        after_available: dict[str, CognitiveRef] = {}
        for identifiers in batched(
            tuple(dict.fromkeys(ref.id for ref in preview.affected)), 1000, strict=False
        ):
            page = await self.cognition.records_in(
                tx, ctx, RecordsQuery(object_ids=identifiers, limit=1000)
            )
            after_available.update(
                (entry.record.reference.id, entry.record.reference) for entry in page.items
            )
        result = SourceResult(
            operation_id=command.operation_id,
            source_operation_id=source_operation_id,
            observation_ids=preview.observation_ids,
            references=tuple(changed_objects),
            affected=preview.affected,
            retired=tuple(
                (ref for key, ref in before_available.items() if key not in after_available)
            ),
            restored=tuple(
                (ref for key, ref in after_available.items() if key not in before_available)
            ),
            commit_seq=tx.commit_seq,
        )
        await tx.connection.execute(
            source_operations.insert().values(
                id=source_operation_id,
                namespace_id=ctx.namespace_id,
                record={
                    "caller_id": ctx.caller_id,
                    "requested_by": command.requested_by,
                    "kind": kind,
                    "reason": command.reason,
                    "result": result.model_dump(mode="json"),
                },
            )
        )
        return (result, detached)

    async def change_objects_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        references: tuple[CognitiveRef, ...],
        kind: Literal["trash", "restore", "purge"],
        command: SourceCommand,
    ) -> list[CognitiveRef]:
        result: list[CognitiveRef] = []
        ordered = sorted(
            references,
            key=lambda ref: (
                ref.kind == "association" if kind == "restore" else ref.kind != "association",
                ref.id,
            ),
        )
        for reference in ordered:
            previous = await self.cognition.get_in(
                tx, ctx, reference, check_visibility=kind != "purge", purpose="management"
            )
            if previous.purged:
                if kind == "restore":
                    raise DomainError(
                        "cognition_purged", "Permanently erased objects cannot be restored"
                    )
                result.append(previous.reference)
                continue
            if kind == "purge":
                if command.grace_hours is not None:
                    now = round(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
                    if (
                        not previous.deleted
                        or now - previous.times.recorded.coordinate < command.grace_hours * 3600000
                    ):
                        raise DomainError(
                            "deletion_grace_pending",
                            "The selected object has not completed its deletion grace period",
                        )
                changed = await self.cognition.purge_cognition_in(
                    tx, ctx, reference, force=command.force, reason=command.reason
                )
            elif previous.deleted == (kind == "trash"):
                changed = previous
            elif isinstance(previous.body, MemoryBody):
                changed = await self.cognition.change_retention_in(
                    tx,
                    ctx,
                    RetentionChange(
                        reference=reference,
                        action="trash" if kind == "trash" else "restore",
                        reason=command.reason,
                        force=command.force,
                    ),
                )
            else:
                changed = await self.cognition.set_deleted_in(
                    tx, ctx, reference, kind == "trash", reason=command.reason
                )
            result.append(changed.reference)
        return result

    async def _purge_import_payloads_in(self, tx: WriteTx, ctx: CallContext) -> None:
        from nous.imports.schema import catalogs, inputs, requests, units

        source_rows = (
            (
                await tx.connection.execute(
                    select(observations.c.material_id, observations.c.record).where(
                        observations.c.namespace_id == ctx.namespace_id
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
                    select(catalogs).where(catalogs.c.namespace_id == ctx.namespace_id)
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
                and context.get("namespace_id") == ctx.namespace_id
                and set(row["record"].get("material_ids", ())).intersection(purged_materials)
            ):
                await tx.connection.execute(
                    update(requests)
                    .where(requests.c.id == row["id"])
                    .values(record={**row["record"], "command": None, "purged": True})
                )
        await self.lifecycle.purge_model_inputs_in(tx, tuple(sorted(purged_materials)))

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
                    and (
                        not any(
                            root in live_observations
                            for root in source_roots
                            if isinstance(root, str)
                        )
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

    async def submit_trash(self, ctx: CallContext, command: SourceCommand) -> str:
        from nous.platform.execution import JobSpec

        async with self.authority.write(
            ctx,
            command.operation_id + ":submit",
            operation_kind="sources.submit_trash",
            scope=ctx.namespace_id,
            payload=command.operation_payload(),
        ) as tx:
            if tx.replayed:
                return str(tx.replay_result)
            await self.preview_in(
                tx,
                ctx,
                command.observation_ids,
                references=command.references,
                source_names=command.source_names,
                records=command.records,
                retention_expectations=command.retention_expectations,
            )
            job = await tx.enqueue(
                JobSpec(
                    workflow_name="nous.sources.trash",
                    queue_name="default",
                    args=(ctx.model_dump(mode="json"), command.model_dump(mode="json")),
                )
            )
            return tx.result(job.workflow_id)

    async def run_trash(self, context: JsonValue, payload: JsonValue) -> dict[str, JsonValue]:
        ctx, command = (CallContext.model_validate(context), SourceCommand.model_validate(payload))
        result = await self.trash(ctx, command)
        ids = [ref.id for ref in result.retired if ref.kind == "memory"]
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            {
                "operation_id": result.source_operation_id,
                "deleted_count": len(ids),
                "deleted_memory_ids": ids,
                "observation_ids": list(result.observation_ids),
            }
        )

    async def restore(self, ctx: CallContext, command: SourceCommand):
        return await self._change(ctx, command, "restore")

    async def purge(self, ctx: CallContext, command: SourceCommand):
        return await self._change(ctx, command, "purge")
