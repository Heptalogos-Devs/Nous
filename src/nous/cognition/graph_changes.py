"""Manual graph changes retain the actual report and share one authority transaction."""

from __future__ import annotations

from datetime import datetime
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    AdoptCognition,
    AssociationBody,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EntityBody,
    EpistemicStance,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    RevisionCause,
    TimePoint,
)
from nous_wave.contracts.graph_management import GraphChange, GraphChangeResult
from sqlalchemy import func, select, update

from nous.identity.service import require_subject_in
from nous.platform.persistence import WriteTx

from .graph_state import heads_in, resolve_entity
from .schema import revisions, vectors

if TYPE_CHECKING:
    from .service import CognitionService


def text(value: str | None, field: str) -> str:
    if value is None or not value.strip():
        raise DomainError("graph_input_required", f"{field} must not be empty")
    return value.strip()


async def revised(
    self: CognitionService,
    tx: WriteTx,
    ctx: CallContext,
    previous: CognitiveRecord,
    body: EntityBody | AssociationBody,
    evidence: tuple[EvidenceLink, ...],
    reason: str,
) -> CognitiveRecord:
    value = await self.adopt_in(
        tx,
        ctx,
        AdoptCognition(
            id=previous.reference.id,
            expected_revision=previous.reference.revision,
            body=body,
            evidence=(*previous.evidence, *evidence),
            times=previous.times,
        ),
    )
    value = value.model_copy(
        update={
            "cause": RevisionCause(
                action="graph_change", actor=ctx.caller_id, reason=reason, parent=previous.reference
            )
        }
    )
    await tx.connection.execute(
        update(revisions)
        .where(
            revisions.c.id == value.reference.id, revisions.c.revision == value.reference.revision
        )
        .values(record=value.model_dump(mode="json"))
    )
    return value


async def apply_in(
    self: CognitionService, tx: WriteTx, ctx: CallContext, command: GraphChange, event_id: str
) -> tuple[list[CognitiveRecord], list[CognitiveRef], list[CognitiveRef]]:
    records = list(await heads_in(tx, ctx))
    changed: list[CognitiveRecord] = []
    created: list[CognitiveRef] = []
    revived: list[CognitiveRef] = []
    report = (EvidenceLink(observation_id=event_id, role=command.reason),)

    async def entity(token: str, create: bool = False) -> CognitiveRecord:
        existing = resolve_entity(tuple(records), token)
        if existing is not None:
            await self.get_in(tx, ctx, existing.reference)
            if existing.deleted:
                if not create:
                    raise DomainError(
                        "entity_deleted", "Restore the Entity before changing its relations"
                    )
                value = await self.set_deleted_in(
                    tx, ctx, existing.reference, False, evidence=report, reason=command.reason
                )
                changed.append(value)
                revived.append(value.reference)
                records.remove(existing)
                records.append(value)
                return value
            return existing
        if not create:
            raise DomainError("entity_not_found", "The Entity selector has no accessible match")
        value = await self.adopt_in(
            tx,
            ctx,
            AdoptCognition(body=EntityBody(label=token, entity_type="other"), evidence=report),
        )
        records.append(value)
        changed.append(value)
        created.append(value.reference)
        return value

    if command.action == "create_node":
        value = await entity(text(command.name, "name"), True)
        if value not in changed:
            changed.append(value)
        return changed, created, revived
    if command.action in {"rename_node", "delete_node"}:
        token = command.reference.id if command.reference else text(command.name, "name")
        previous = await entity(token)
        if command.reference and previous.reference != command.reference:
            raise DomainError("revision_conflict", "The Entity has a newer revision")
        related = [
            row
            for row in records
            if not row.deleted
            and isinstance(row.body, AssociationBody)
            and previous.reference.id in {row.body.from_ref.id, row.body.to_ref.id}
        ]
        for row in related:
            await self.get_in(tx, ctx, row.reference)
        if command.action == "delete_node":
            for row in related:
                changed.append(
                    await self.set_deleted_in(
                        tx, ctx, row.reference, True, evidence=report, reason=command.reason
                    )
                )
            changed.append(
                await self.set_deleted_in(
                    tx, ctx, previous.reference, True, evidence=report, reason=command.reason
                )
            )
        else:
            assert isinstance(previous.body, EntityBody)
            name = text(command.new_name, "new_name")
            collision = resolve_entity(tuple(records), name)
            if collision and collision.reference.id != previous.reference.id:
                raise DomainError(
                    "entity_name_conflict",
                    "The new name identifies another Entity; use an explicit merge operation",
                )
            body = previous.body.model_copy(
                update={
                    "label": name,
                    "aliases": tuple(dict.fromkeys((*previous.body.aliases, previous.body.label))),
                }
            )
            value = await revised(self, tx, ctx, previous, body, report, command.reason)
            changed.append(value)
            for row in related:
                assert isinstance(row.body, AssociationBody)
                body = row.body.model_copy(
                    update={
                        "from_ref": value.reference
                        if row.body.from_ref.id == value.reference.id
                        else row.body.from_ref,
                        "to_ref": value.reference
                        if row.body.to_ref.id == value.reference.id
                        else row.body.to_ref,
                    }
                )
                changed.append(await revised(self, tx, ctx, row, body, report, command.reason))
        return changed, created, revived

    left = (
        await entity(text(command.subject, "subject"), command.action == "create_edge")
        if command.subject
        else None
    )
    right = (
        await entity(text(command.object, "object"), command.action == "create_edge")
        if command.object
        else None
    )
    associations = [row for row in records if isinstance(row.body, AssociationBody)]
    if command.reference or command.relation_hash:
        selector = command.reference.id if command.reference else command.relation_hash
        matches = [
            row
            for row in associations
            if row.reference.id == selector or row.reference.exact() == selector
        ]
    else:
        matches = [
            row
            for row in associations
            if isinstance(row.body, AssociationBody)
            and left
            and right
            and row.body.from_ref.id == left.reference.id
            and row.body.to_ref.id == right.reference.id
            and (command.predicate is None or row.body.relation == command.predicate)
        ]
    if command.action == "create_edge":
        if left is None or right is None:
            raise DomainError("graph_input_required", "An association requires subject and object")
        predicate = text(command.predicate, "predicate")
        matches = [
            row
            for row in matches
            if isinstance(row.body, AssociationBody) and row.body.relation == predicate
        ]
        if len(matches) > 1:
            raise DomainError(
                "ambiguous_association",
                "The endpoints have multiple matching Association identities",
            )
        evidence = report
        if command.source_paragraph:
            memories = await heads_in(tx, ctx, ("memory",))
            targets = [
                row
                for row in memories
                if isinstance(row.body, MemoryBody)
                and not row.deleted
                and not row.purged
                and command.source_paragraph
                in {row.reference.id, row.reference.exact(), *row.body.external_ids.values()}
            ]
            if len(targets) != 1:
                raise DomainError(
                    "memory_not_found", "source_paragraph must identify an actual Memory"
                )
            await self.get_in(tx, ctx, targets[0].reference)
            evidence = (
                *evidence,
                EvidenceLink(reference=targets[0].reference, role="source_paragraph"),
            )
        stance = EpistemicStance(
            classification="reported", attribution=event_id, confidence=command.confidence
        )
        body = AssociationBody(
            from_ref=left.reference,
            to_ref=right.reference,
            relation=predicate,
            basis_class="host_explicit",
            weight=command.weight,
            epistemic=stance,
        )
        if matches:
            previous = matches[0]
            await self.get_in(tx, ctx, previous.reference)
            value = await revised(self, tx, ctx, previous, body, evidence, command.reason)
            if previous.deleted:
                revived.append(value.reference)
        else:
            value = await self.adopt_in(tx, ctx, AdoptCognition(body=body, evidence=evidence))
            created.append(value.reference)
        changed.append(value)
    else:
        matches = [row for row in matches if not row.deleted]
        if not matches:
            raise DomainError(
                "association_not_found", "No live Association matches the exact selector"
            )
        if command.action == "update_edge_weight" and command.weight is None:
            raise DomainError(
                "graph_weight_required", "The weight change requires an explicit value"
            )
        for previous in matches:
            await self.get_in(tx, ctx, previous.reference)
            if command.reference and previous.reference != command.reference:
                raise DomainError("revision_conflict", "The Association has a newer revision")
            assert isinstance(previous.body, AssociationBody)
            if command.action == "delete_edge":
                value = await self.set_deleted_in(
                    tx, ctx, previous.reference, True, evidence=report, reason=command.reason
                )
            else:
                value = await revised(
                    self,
                    tx,
                    ctx,
                    previous,
                    previous.body.model_copy(update={"weight": command.weight}),
                    report,
                    command.reason,
                )
            changed.append(value)
    return changed, created, revived


async def manage_graph(
    self: CognitionService, ctx: CallContext, command: GraphChange
) -> GraphChangeResult:
    document = ImportDocument(
        text=command.model_dump_json(),
        format="json",
        filename="graph-change.json",
        media_type="application/json",
        source_type="administrative_change",
        source="graph_management",
        origin={
            "caller_id": ctx.caller_id,
            **({"reported_by": command.reported_by} if command.reported_by else {}),
        },
    )
    lease = (
        await self.materials.content_store.publish_bytes(
            document.inline_text().encode(), document.media_type
        )
        if self.materials.content_store
        else None
    )
    try:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="graph.change",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                result = GraphChangeResult.model_validate(tx.replay_result)
                await self.replay_records_in(tx, ctx, result.records)
                return result
            await require_subject_in(tx, ctx, "memory")
            if ctx.source_groups():
                raise DomainError(
                    "source_unavailable",
                    "A new manual report requires an unrestricted input-admission scope",
                )
            now = TimePoint(
                coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
            )
            document = document.model_copy(update={"observed_at": now})
            material = await self.materials.put_in(tx, ctx, document, lease)
            event = await self.materials.observe_in(
                tx, ctx, material["id"], document, kind="import"
            )
            changed, created, revived = await apply_in(self, tx, ctx, command, event["id"])
            retired = [
                record.cause.parent.exact()
                for record in changed
                if record.commit_seq == tx.commit_seq and record.cause and record.cause.parent
            ]
            retired_vectors = int(
                (
                    await tx.connection.scalar(
                        select(func.count())
                        .select_from(vectors)
                        .where(vectors.c.reference.in_(retired))
                    )
                )
                or 0
            )
            result = GraphChangeResult(
                operation_id=command.operation_id,
                records=tuple(changed),
                created=tuple(created),
                revived=tuple(revived),
                report_material_id=material["id"],
                observation_id=event["id"],
                commit_seq=tx.commit_seq,
                retired_vector_associations=retired_vectors,
            )
            tx.result(result)
        if self.invalidate:
            await self.invalidate()
        return result
    finally:
        if lease and self.materials.content_store:
            await self.materials.content_store.release(lease)
