"""Read current adopted results and the original accepted input scope."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import ImportResult
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError

from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .service import ImportService


async def input_context_in(
    self: ImportService, view: ReadView | WriteTx, ctx: CallContext, row: dict[str, object]
) -> CallContext:
    from pydantic import TypeAdapter

    captured = CallContext.model_validate(row["context"])
    if (captured.caller_id, captured.namespace_id, captured.namespace_id) != (
        ctx.caller_id,
        ctx.namespace_id,
        ctx.namespace_id,
    ):
        raise DomainError(
            "import_request_not_found", "Import request belongs to another caller scope"
        )
    if not ctx.source_groups():
        return ctx
    for material_id in row.get("original_material_ids", ()):
        await self.materials.read_in(view, ctx, material_id)
    admitted = TypeAdapter(tuple[str, ...]).validate_python(row.get("input_observation_ids", ()))
    if not admitted:
        return ctx
    return (await self.materials.restore_input_in(view, ctx, observation_ids=admitted)).context


async def read_admit_result(self: ImportService, ctx: CallContext, request_id: str) -> ImportResult:
    from sqlalchemy import select

    from .schema import outputs, requests, successes

    async with self.authority.read(ctx) as view:
        row = await view.connection.scalar(
            select(requests.c.record).where(requests.c.id == request_id)
        )
        if row is None or row.get("kind") != "files":
            raise DomainError("import_request_not_found", "Import request is unavailable")
        captured = CallContext.model_validate(row["context"])
        if (captured.caller_id, captured.namespace_id, captured.namespace_id) != (
            ctx.caller_id,
            ctx.namespace_id,
            ctx.namespace_id,
        ):
            raise DomainError(
                "import_request_not_found", "Import request belongs to another caller scope"
            )
        if row.get("purged"):
            raise DomainError("source_purged", "Import source has been purged")
        ctx = await input_context_in(self, view, ctx, row)
        command = row["command"]
    if row.get("result") is None:
        raise DomainError("import_result_pending", "Literal import has not completed")
    from nous_sdk.contracts import FileSuccess, UnitOutput

    async with self.authority.read(ctx) as view:
        summary = dict(row["result"])
        selected_outputs = tuple(
            [
                UnitOutput.model_validate(
                    await view.connection.scalar(
                        select(outputs.c.record).where(outputs.c.id == identity)
                    )
                )
                for identity in summary.pop("output_ids")
            ]
        )
        file_facts = tuple(
            [
                FileSuccess.model_validate(
                    await view.connection.scalar(
                        select(successes.c.record).where(
                            successes.c.id == identity, successes.c.namespace_id == ctx.namespace_id
                        )
                    )
                )
                for identity in summary.pop("file_success_ids")
            ]
        )
        catalog = (
            await self.read_catalog_in(view, ctx, row["catalog_id"])
            if row.get("catalog_id")
            else None
        )
        result = ImportResult(
            catalog=catalog, outputs=selected_outputs, file_successes=file_facts, **summary
        )
        for output in result.outputs:
            for reference in output.cognition:
                await self.cognition.get_in(view, ctx, reference)
    if result.skipped_success_ids:
        from nous_sdk.contracts import UnitOutput

        existing = {output.id: output for output in result.outputs}
        async with self.authority.read(ctx) as view:
            for success_id in result.skipped_success_ids:
                fact = await view.connection.scalar(
                    select(successes.c.record).where(
                        successes.c.id == success_id,
                        successes.c.namespace_id == ctx.namespace_id,
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
                    if command.get("deferred_formation") and output.formation is None:
                        raise DomainError(
                            "formation_job_unavailable",
                            "Historical output has no actual deferred formation job",
                        )
                    for reference in output.cognition:
                        await self.cognition.get_in(view, ctx, reference)
                    existing[output.id] = output
        result = result.model_copy(update={"outputs": tuple(existing.values())})
    return result
