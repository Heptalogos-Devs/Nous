"""Qualify only this caller operation's actual newly written exact outputs."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveRef, DomainError

from nous.platform.persistence import ReadView, WriteTx

from .projection import references_in

if TYPE_CHECKING:
    from .service import CognitionService


async def qualify_known_outputs_in(
    self: CognitionService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    references: tuple[CognitiveRef, ...],
    *,
    operation_id: str,
    operation_kind: str,
    scope: str,
    input_material_ids: tuple[str, ...] | None = None,
) -> CallContext:
    admission = await self.materials.restore_input_in(
        view,
        ctx,
        input_material_ids,
        operation_id=operation_id,
        operation_kind=operation_kind,
        scope=scope,
    )
    if not admission.observation_ids:
        raise DomainError(
            "input_admission_unavailable", "Known outputs require actual submitted-input provenance"
        )
    records = [
        await self.get_in(view, ctx, reference, check_visibility=False) for reference in references
    ]
    exacts = {
        record.reference.exact()
        for record in records
        if record.commit_seq == admission.commit_seq and not record.purged
    }
    for record in records:
        if record.purged:
            raise DomainError(
                "cognition_purged", "An erased output cannot regain a body through replay"
            )
        if record.commit_seq != admission.commit_seq:
            await self.get_in(view, ctx, record.reference)
            continue
        previous = (
            tuple(references_in(record.body))
            + tuple(link.reference for link in record.evidence if link.reference)
            + ((record.cause.parent,) if record.cause and record.cause.parent else ())
        )
        for reference in previous:
            if reference.exact() not in exacts:
                await self.get_in(view, ctx, reference)
    effective = admission.context
    # Only exact revisions are added. Old raw roots, stable object IDs and the
    # independently enforced credential ceiling remain untouched.
    constraints = effective.source_constraints or (
        () if effective.allowed_source_ids is None else (effective.allowed_source_ids,)
    )
    if not constraints:
        return effective
    return effective.model_copy(
        update={
            "source_constraints": tuple(
                tuple(dict.fromkeys((*group, *sorted(exacts)))) for group in constraints
            ),
            "allowed_source_ids": tuple(
                dict.fromkeys((*(effective.allowed_source_ids or ()), *sorted(exacts)))
            ),
        }
    )
