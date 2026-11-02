"""Current semantic references move with identity changes; prior evidence stays exact."""

from __future__ import annotations

from collections.abc import Iterator
from typing import TYPE_CHECKING, cast

from nous_sdk.contracts import (
    AdoptCognition,
    CognitiveBody,
    CognitiveRecord,
    CognitiveRef,
    MemorySemanticsBody,
    RevisionCause,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.cognition import ReferencePartition
from nous_sdk.contracts.fact_body import FactClaimBody
from pydantic import BaseModel, TypeAdapter
from sqlalchemy import select

from nous.platform.persistence import WriteTx

from .revision_state import RevisionState
from .schema import revisions

if TYPE_CHECKING:
    from .records import CognitiveRecords
_HISTORY = {"evidence", "lineage", "supersedes", "canonical", "successors", "identity_lineage"}


def current_references(value: object, field: str = "") -> Iterator[CognitiveRef]:
    if field in _HISTORY:
        return
    if isinstance(value, CognitiveRef):
        yield value
    elif isinstance(value, BaseModel):
        for name in type(value).model_fields:
            yield from current_references(getattr(value, name), name)
    elif isinstance(value, tuple):
        for item in cast(tuple[object, ...], value):
            yield from current_references(item)


def replace_current(
    value: object, replacements: dict[str, CognitiveRef], field: str = ""
) -> object:
    if field in _HISTORY:
        return value
    if isinstance(value, CognitiveRef):
        return replacements.get(value.id, value)
    if isinstance(value, BaseModel):
        return value.model_copy(
            update={
                name: replace_current(getattr(value, name), replacements, name)
                for name in type(value).model_fields
            }
        )
    if isinstance(value, tuple):
        return tuple(
            replace_current(item, replacements) for item in cast(tuple[object, ...], value)
        )
    return value


async def replace_references_in(
    self: CognitiveRecords,
    tx: WriteTx,
    ctx: CallContext,
    replacements: dict[str, CognitiveRef],
    reason: str,
    *,
    exclude: frozenset[str] = frozenset(),
    partitions: tuple[CognitiveRef, ...] = (),
    assignments: tuple[ReferencePartition, ...] = (),
    member_partitions: dict[str, int] | None = None,
) -> tuple[CognitiveRecord, ...]:
    rows = (
        await tx.connection.execute(
            select(revisions.c.record).where(
                revisions.c.namespace_id == ctx.namespace_id, revisions.c.head.is_(True)
            )
        )
    ).scalars()
    explicit = {item.consumer.id: item for item in assignments}
    if len(explicit) != len(assignments) or any(
        item.partition >= len(partitions) for item in assignments
    ):
        raise DomainError(
            "split_assignment_invalid", "Assign each consumer once to an existing partition"
        )
    changed: list[CognitiveRecord] = []
    used: set[str] = set()
    for raw in rows:
        record = CognitiveRecord.model_validate(raw)
        if (
            record.purged
            or record.body is None
            or record.reference.id in exclude
            or isinstance(record.body, FactClaimBody)
        ):
            continue
        if not any(ref.id in replacements for ref in current_references(record.body)):
            continue
        await self.get_in(tx, ctx, record.reference)
        selected = replacements
        if partitions:
            assignment = explicit.get(record.reference.id)
            inferred = (member_partitions or {}).get(record.reference.id)
            if isinstance(record.body, MemorySemanticsBody):
                inferred = (member_partitions or {}).get(record.body.memory.id, inferred)
            if assignment is not None:
                if assignment.consumer != record.reference:
                    raise DomainError(
                        "revision_conflict", "A split consumer changed since its selected revision"
                    )
                inferred = assignment.partition
                used.add(record.reference.id)
            if inferred is None:
                raise DomainError(
                    "split_assignment_required",
                    "Select the partition for each current consumer",
                    consumer=record.reference.model_dump(mode="json"),
                )
            selected = {identity: partitions[inferred] for identity in replacements}
        body = TypeAdapter(CognitiveBody).validate_python(replace_current(record.body, selected))
        if body == record.body:
            continue
        value = await self.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=record.reference.id,
                expected_revision=record.reference.revision,
                body=body,
                evidence=record.evidence,
                times=record.times,
            ),
            state=RevisionState(
                lifecycle=record.lifecycle,
                deleted=record.deleted,
                cause=RevisionCause(
                    action="identity.references",
                    actor=ctx.caller_id,
                    reason=reason,
                    parent=record.reference,
                ),
            ),
        )
        changed.append(value)
    if explicit.keys() - used:
        raise DomainError(
            "split_assignment_invalid",
            "An assigned consumer does not reference the selected identity",
        )
    return tuple(changed)
