"""The graph directory is a view of authorized canonical revisions."""

from __future__ import annotations

from datetime import datetime as datetime
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    AdoptCognition,
    AssociationBody,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EntityBody,
    EpistemicStance,
    EvidenceLink,
    RetentionChange,
    RevisionCause,
)
from nous_sdk.contracts import ImportDocument as ImportDocument
from nous_sdk.contracts import TimePoint as TimePoint
from nous_sdk.contracts.graph_management import (
    GraphChange,
    GraphReadQuery,
    GraphSearchMatch,
    GraphSearchQuery,
    GraphSearchResult,
    GraphSnapshot,
)
from nous_sdk.contracts.graph_management import GraphChangeResult as GraphChangeResult
from sqlalchemy import func as func
from sqlalchemy import select

from nous.cognition.entities import project_entity_in
from nous.identity.access import require_namespace_in
from nous.platform.persistence import ReadView, WriteTx

from .lifecycle import selectable
from .revision_state import RevisionState
from .schema import revisions
from .schema import vectors as vectors

if TYPE_CHECKING:
    from .memory import MemoryService
    from .records import CognitiveRecords


async def heads_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    kinds: tuple[str, ...] = ("entity", "association"),
    *,
    reference_ids: tuple[str, ...] | None = None,
) -> tuple[CognitiveRecord, ...]:
    statement = select(revisions.c.record).where(
        revisions.c.namespace_id == ctx.namespace_id,
        revisions.c.head.is_(True),
        revisions.c.kind.in_(kinds),
    )
    if reference_ids is not None:
        statement = statement.where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.id.in_(reference_ids))
    values = (await view.connection.execute(statement.order_by(revisions.c.id))).scalars()
    return tuple(CognitiveRecord.model_validate(value) for value in values)


def resolve_entity(
    records: tuple[CognitiveRecord, ...], token: str, *, include_deleted: bool = False
) -> CognitiveRecord | None:
    key = token.strip().casefold()
    if not key:
        return None
    matches = [
        record
        for record in records
        if isinstance(record.body, EntityBody)
        and selectable(record, include_deleted=include_deleted)
        and (
            token
            in {record.reference.id, record.reference.exact(), *record.body.identifiers.values()}
            or key
            in {value.casefold() for value in (record.body.label, *record.body.effective_aliases)}
        )
    ]
    if len(matches) > 1:
        raise DomainError("ambiguous_entity", "The selector identifies multiple Entity identities")
    return matches[0] if matches else None


if TYPE_CHECKING:
    from .records import CognitiveRecords


def text(value: str | None, field: str) -> str:
    if value is None or not value.strip():
        raise DomainError("graph_input_required", f"{field} must not be empty")
    return value.strip()


class GraphService:
    def __init__(self, records: CognitiveRecords, *, memories: MemoryService):
        self.memories = memories
        self.records = records

    async def graph_snapshot(self, ctx: CallContext) -> GraphSnapshot:
        async with self.records.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            records: list[CognitiveRecord] = []
            for record in await heads_in(view, ctx):
                record = await project_entity_in(self.records, view, ctx, record)
                if not selectable(record) or not await self.records.visible_in(view, ctx, record):
                    continue
                records.append(record)
            return GraphSnapshot(
                records=tuple(records),
                watermark=view.commit_seq,
            )

    async def search_graph(self, ctx: CallContext, command: GraphSearchQuery) -> GraphSearchResult:
        query = command.text.strip().casefold()
        if not query:
            raise DomainError("invalid_query", "Graph search text must not be empty")
        snapshot = await self.graph_snapshot(ctx)
        known = {record.reference.exact(): record for record in snapshot.records}

        async def name(reference: CognitiveRef) -> str:
            record = known.get(reference.exact())
            if record is None:
                record = await self.records.get(ctx, reference)
                known[reference.exact()] = record
            return record.body.label if isinstance(record.body, EntityBody) else reference.exact()

        matches: list[GraphSearchMatch] = []
        for record in snapshot.records:
            body = record.body
            source_name = target_name = None
            fields: tuple[tuple[str, str], ...]
            if isinstance(body, EntityBody):
                fields = (("name", body.label), ("id", record.reference.id))
            elif isinstance(body, AssociationBody):
                try:
                    source_name, target_name = (await name(body.from_ref), await name(body.to_ref))
                except DomainError as error:
                    if error.code not in {"source_unavailable", "cognition_not_found"}:
                        raise
                    continue
                fields = (
                    ("subject", source_name),
                    ("object", target_name),
                    ("predicate", body.relation),
                    ("id", record.reference.id),
                )
            else:
                continue
            found: tuple[str, str, int] | None = None
            for field, value in fields:
                token = value.strip().casefold()
                rank = (
                    0
                    if token == query
                    else 1
                    if token.startswith(query)
                    else 2
                    if query in token
                    else None
                )
                if token and rank is not None and (found is None or rank < found[2]):
                    found = (field, value, rank)
            if found:
                matches.append(
                    GraphSearchMatch(
                        record=record,
                        field=found[0],
                        value=found[1],
                        rank=found[2],
                        source_name=source_name,
                        target_name=target_name,
                    )
                )
        matches.sort(
            key=lambda item: (
                item.rank,
                item.record.reference.kind != "entity",
                (item.source_name or item.value).casefold(),
                item.record.reference.id,
            )
        )
        selected = matches[: command.limit]
        for item in selected:
            await self.records.get(ctx, item.record.reference)
        return GraphSearchResult(
            query=command.text.strip(),
            matches=tuple(selected),
            total_matches=len(matches),
            watermark=snapshot.watermark,
        )

    async def read_graph(self, ctx: CallContext, command: GraphReadQuery) -> GraphSnapshot:
        snapshot = await self.graph_snapshot(ctx)
        if not (command.node or command.source or command.target):
            return snapshot

        def resolve(token: str | None) -> str | None:
            if token is None:
                return None
            record = resolve_entity(snapshot.records, token)
            if record is None:
                raise DomainError("entity_not_found", "The selected graph Entity is unavailable")
            return record.reference.id

        node, source, target = (
            resolve(command.node),
            resolve(command.source),
            resolve(command.target),
        )
        selected: list[CognitiveRecord] = []
        identities = {value for value in (node, source, target) if value is not None}
        for record in snapshot.records:
            body = record.body
            if (
                isinstance(body, AssociationBody)
                and (node is None or node in {body.from_ref.id, body.to_ref.id})
                and (source is None or source == body.from_ref.id)
                and (target is None or target == body.to_ref.id)
            ):
                selected.append(record)
                identities.update((body.from_ref.id, body.to_ref.id))
        selected.extend(
            record
            for record in snapshot.records
            if isinstance(record.body, EntityBody) and record.reference.id in identities
        )
        return GraphSnapshot(
            records=tuple(selected),
            watermark=snapshot.watermark,
        )

    async def manage_graph(self, ctx: CallContext, command: GraphChange) -> GraphChangeResult:
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
            await self.records.materials.content_store.publish_bytes(
                document.inline_text().encode(), document.media_type
            )
            if self.records.materials.content_store
            else None
        )
        try:
            async with self.records.authority.write(ctx) as tx:
                await require_namespace_in(tx, ctx, "memory")
                now = TimePoint(
                    coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
                )
                document = document.model_copy(update={"observed_at": now})
                admission = await self.records.materials.admit_text_in(tx, ctx, document, lease)
                material, event = (admission.material, admission.event)
                changed, created, revived = await self.apply_in(
                    tx, ctx, command, event["id"], admission.context
                )
                retired = [
                    record.cause.parent.exact()
                    for record in changed
                    if record.commit_seq == tx.commit_seq and record.cause and record.cause.parent
                ]
                retired_vectors = int(
                    await tx.connection.scalar(
                        select(func.count()).select_from(vectors).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference.in_(retired))
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
            return result
        finally:
            if lease and self.records.materials.content_store:
                await self.records.materials.content_store.release(lease)

    async def revised(
        self,
        tx: WriteTx,
        ctx: CallContext,
        previous: CognitiveRecord,
        body: EntityBody | AssociationBody,
        evidence: tuple[EvidenceLink, ...],
        reason: str,
        *,
        deleted: bool | None = None,
    ) -> CognitiveRecord:
        value = await self.records.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=previous.reference.revision,
                body=body,
                evidence=(
                    EvidenceLink(reference=previous.reference, role="previous_revision"),
                    *evidence,
                ),
                times=previous.times,
            ),
            state=RevisionState(
                cause=RevisionCause(
                    action="graph_change",
                    actor=ctx.caller_id,
                    reason=reason,
                    parent=previous.reference,
                ),
                lifecycle=previous.lifecycle,
                deleted=previous.deleted if deleted is None else deleted,
            ),
        )
        return value

    async def apply_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        command: GraphChange,
        event_id: str,
        admitted_ctx: CallContext,
    ) -> tuple[list[CognitiveRecord], list[CognitiveRef], list[CognitiveRef]]:
        records = list(await heads_in(tx, ctx))
        changed: list[CognitiveRecord] = []
        created: list[CognitiveRef] = []
        revived: list[CognitiveRef] = []
        report = (EvidenceLink(observation_id=event_id, role="graph_management"), *command.evidence)

        async def entity(token: str, create: bool = False) -> CognitiveRecord:
            existing = resolve_entity(tuple(records), token, include_deleted=create)
            if existing is not None:
                await self.records.get_in(tx, ctx, existing.reference)
                if existing.deleted:
                    if not create:
                        raise DomainError(
                            "entity_deleted", "Restore the Entity before changing its relations"
                        )
                    value = await self.records.set_deleted_in(
                        tx,
                        admitted_ctx,
                        existing.reference,
                        False,
                        evidence=report,
                        reason=command.reason,
                    )
                    changed.append(value)
                    revived.append(value.reference)
                    records.remove(existing)
                    records.append(value)
                    return value
                return existing
            if not create:
                raise DomainError("entity_not_found", "The Entity selector has no accessible match")
            value = await self.records.adopt_in(
                tx,
                admitted_ctx,
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
            return (changed, created, revived)
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
                and (previous.reference.id in {row.body.from_ref.id, row.body.to_ref.id})
            ]
            for row in related:
                await self.records.get_in(tx, ctx, row.reference)
            if command.action == "delete_node":
                for row in related:
                    changed.append(
                        await self.memories.change_retention_in(
                            tx,
                            admitted_ctx,
                            RetentionChange(
                                reference=row.reference, action="trash", reason=command.reason
                            ),
                        )
                    )
                changed.append(
                    await self.memories.change_retention_in(
                        tx,
                        admitted_ctx,
                        RetentionChange(
                            reference=previous.reference, action="trash", reason=command.reason
                        ),
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
                        "aliases": tuple(
                            dict.fromkeys((*previous.body.aliases, previous.body.label))
                        ),
                    }
                )
                value = await self.revised(tx, admitted_ctx, previous, body, report, command.reason)
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
                    changed.append(
                        await self.revised(tx, admitted_ctx, row, body, report, command.reason)
                    )
            return (changed, created, revived)
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
        if command.reference:
            matches = [
                row
                for row in associations
                if row.reference.id == command.reference.id
            ]
        else:
            matches = [
                row
                for row in associations
                if isinstance(row.body, AssociationBody)
                and left
                and right
                and (row.body.from_ref.id == left.reference.id)
                and (row.body.to_ref.id == right.reference.id)
                and (command.predicate is None or row.body.relation == command.predicate)
            ]
        if command.action == "create_edge":
            if left is None or right is None:
                raise DomainError(
                    "graph_input_required", "An association requires subject and object"
                )
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
                await self.records.get_in(tx, ctx, previous.reference)
                value = await self.revised(
                    tx, admitted_ctx, previous, body, evidence, command.reason, deleted=False
                )
                if previous.deleted:
                    revived.append(value.reference)
            else:
                value = await self.records.adopt_in(
                    tx, admitted_ctx, AdoptCognition(body=body, evidence=evidence)
                )
                created.append(value.reference)
            changed.append(value)
            for endpoint in (left, right):
                if not any(record.reference == endpoint.reference for record in changed):
                    changed.append(endpoint)
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
                await self.records.get_in(tx, ctx, previous.reference)
                if command.reference and previous.reference != command.reference:
                    raise DomainError("revision_conflict", "The Association has a newer revision")
                assert isinstance(previous.body, AssociationBody)
                if command.action == "delete_edge":
                    value = await self.memories.change_retention_in(
                        tx,
                        admitted_ctx,
                        RetentionChange(
                            reference=previous.reference, action="trash", reason=command.reason
                        ),
                    )
                else:
                    value = await self.revised(
                        tx,
                        admitted_ctx,
                        previous,
                        previous.body.model_copy(update={"weight": command.weight}),
                        report,
                        command.reason,
                    )
                changed.append(value)
        return (changed, created, revived)
