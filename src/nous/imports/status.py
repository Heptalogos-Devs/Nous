"""Import adoption and vector attempts remain distinct, observed business facts."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError, UnitOutput
from nous_sdk.contracts.diagnostics import ImportVectorStatus
from nous_sdk.contracts.imports import UnitAttempt
from sqlalchemy import select

from nous.cognition.schema import vectors
from nous.identity.service import require_subject_in

from .schema import outputs, requests

if TYPE_CHECKING:
    from .service import ImportService


async def vector_status(self: ImportService, ctx: CallContext) -> ImportVectorStatus:
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx)
        values = (await view.connection.execute(select(outputs.c.record))).scalars()
        visible: list[UnitOutput] = []
        for value in values:
            output = UnitOutput.model_validate(value)
            try:
                for reference in output.cognition:
                    await self.cognition.get_in(view, ctx, reference)
            except DomainError as error:
                if error.code not in {"source_unavailable", "cognition_not_found"}:
                    raise
            else:
                if output.cognition:
                    visible.append(output)
        references = {reference.exact() for output in visible for reference in output.cognition}
        rows = (
            await view.connection.execute(
                select(vectors.c.reference, vectors.c.space).where(
                    vectors.c.reference.in_(references)
                )
            )
        ).all()
        per_space: dict[str, set[str]] = {}
        for row in rows:
            per_space.setdefault(str(row.space), set()).add(str(row.reference))
        complete = empty = partial = 0
        all_indexed: set[str] = set()
        for indexed in per_space.values():
            all_indexed.update(indexed)
        for output in visible:
            requested = {reference.exact() for reference in output.cognition}
            if any(requested.issubset(indexed) for indexed in per_space.values()):
                complete += 1
            elif requested.intersection(all_indexed):
                partial += 1
            else:
                empty += 1
        unit_ids = {output.unit_id for output in visible}
        failed = running = 0
        attempts = (await view.connection.execute(select(requests.c.record))).scalars()
        for row in attempts:
            if row.get("kind") != "unit_attempt" or row.get("purged"):
                continue
            value = UnitAttempt.model_validate(row["attempt"])
            if value.unit_id in unit_ids:
                failed += value.vector_error is not None
                running += value.status == "running" and value.stage == "embedding"
        return ImportVectorStatus(
            authority_commit_seq=view.commit_seq,
            visible_outputs=len(visible),
            disabled_outputs=sum(output.vector_policy == "disabled" for output in visible),
            required_outputs=sum(output.vector_policy == "required" for output in visible),
            metadata_allowed_outputs=sum(
                output.vector_policy == "metadata_allowed" for output in visible
            ),
            outputs_with_complete_adopted_space=complete,
            outputs_without_vectors=empty,
            outputs_with_partial_vectors=partial,
            vector_failed_attempts=failed,
            running_embedding_attempts=running,
        )
