"""Cached import text is reauthorized under the caller's current source constraints."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import CallContext, CatalogView, DomainError, ImportResult

from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .service import ImportService


async def input_context_in(
    self: ImportService, view: ReadView | WriteTx, ctx: CallContext, row: dict[str, object]
) -> CallContext:
    from nous_wave.contracts import AdmitFiles
    from pydantic import TypeAdapter

    captured = CallContext.model_validate(row["context"])
    if (captured.caller_id, captured.namespace_id, captured.subject_id) != (
        ctx.caller_id,
        ctx.namespace_id,
        ctx.subject_id,
    ):
        raise DomainError(
            "import_request_not_found", "Import request belongs to another caller scope"
        )
    operation = row.get("input_operation")
    if operation is None or not ctx.source_groups():
        return ctx
    operation = TypeAdapter(dict[str, str]).validate_python(operation)
    command = AdmitFiles.model_validate(row["command"])
    for document in command.inputs:
        if document.material_id is not None:
            await self.materials.read_in(view, ctx, document.material_id)
    return (
        await self.materials.restore_input_in(
            view,
            ctx,
            operation_id=operation["operation_id"],
            operation_kind=operation["operation_kind"],
            scope=operation["scope"],
        )
    ).context


async def replay_catalog_in(
    self: ImportService, view: ReadView | WriteTx, ctx: CallContext, cached: CatalogView
) -> CatalogView:
    current = await self.read_catalog_in(view, ctx, cached.id)
    if not {unit.id for unit in cached.units}.issubset({unit.id for unit in current.units}):
        raise DomainError(
            "source_unavailable",
            "The import receipt includes units outside the current source scope",
        )
    return current


async def replay_catalog(self: ImportService, ctx: CallContext, cached: CatalogView) -> CatalogView:
    async with self.authority.read(ctx) as view:
        return await self.replay_catalog_in(view, ctx, cached)


async def replay_result_in(
    self: ImportService, view: ReadView | WriteTx, ctx: CallContext, cached: ImportResult
) -> ImportResult:
    catalog = await self.replay_catalog_in(view, ctx, cached.catalog) if cached.catalog else None
    for output in cached.outputs:
        for reference in output.cognition:
            record = await self.cognition.get_in(view, ctx, reference)
            if record.purged:
                raise DomainError(
                    "source_unavailable", "The import receipt refers to purged cognitive content"
                )
    return cached.model_copy(update={"catalog": catalog})


async def replay_result(
    self: ImportService, ctx: CallContext, cached: ImportResult
) -> ImportResult:
    async with self.authority.read(ctx) as view:
        return await self.replay_result_in(view, ctx, cached)


async def read_admit_result(self: ImportService, ctx: CallContext, request_id: str) -> ImportResult:
    from nous_wave.contracts import AdmitFiles
    from sqlalchemy import select

    from .schema import outputs, requests, successes

    async with self.authority.read(ctx) as view:
        row = await view.connection.scalar(
            select(requests.c.record).where(requests.c.id == request_id)
        )
        if row is None or row.get("kind") != "files":
            raise DomainError("import_request_not_found", "Import request is unavailable")
        captured = CallContext.model_validate(row["context"])
        if (captured.caller_id, captured.namespace_id, captured.subject_id) != (
            ctx.caller_id,
            ctx.namespace_id,
            ctx.subject_id,
        ):
            raise DomainError(
                "import_request_not_found", "Import request belongs to another caller scope"
            )
        if row.get("purged"):
            raise DomainError("source_purged", "Import source has been purged")
        ctx = await input_context_in(self, view, ctx, row)
        command = AdmitFiles.model_validate(row["command"])
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="imports.manifest",
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is None:
        raise DomainError("import_result_pending", "Literal import has not completed")
    result = await self.replay_result(ctx, ImportResult.model_validate(receipt.result))
    if result.skipped_success_ids:
        from nous_wave.contracts import UnitOutput

        existing = {output.id: output for output in result.outputs}
        async with self.authority.read(ctx) as view:
            for success_id in result.skipped_success_ids:
                fact = await view.connection.scalar(
                    select(successes.c.record).where(
                        successes.c.id == success_id,
                        successes.c.namespace_id == ctx.namespace_id,
                        successes.c.subject_id == ctx.subject_id,
                    )
                )
                if fact is None:
                    raise DomainError(
                        "import_result_unavailable", "A deduplicated file fact is unavailable"
                    )
                for output_id in fact["unit_outputs"].values():
                    output = UnitOutput.model_validate(
                        await view.connection.scalar(
                            select(outputs.c.record).where(outputs.c.id == output_id)
                        )
                    )
                    if command.deferred_formation and output.formation is None:
                        raise DomainError(
                            "formation_job_unavailable",
                            "Historical output has no actual deferred formation job",
                        )
                    for reference in output.cognition:
                        await self.cognition.get_in(view, ctx, reference)
                    existing[output.id] = output
        result = result.model_copy(update={"outputs": tuple(existing.values())})
    return result
