"""Authority for exact cognitive revisions and their supporting roots."""

from __future__ import annotations

from nous_sdk.presentation import body_text

import base64
import binascii
import hashlib
from collections import deque
from datetime import datetime
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    AdoptCognition,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    ConceptBody,
    DomainError,
    EvidenceLink,
    JournalBody,
    MediaRecord,
    MediaReference,
    MemoryBody,
    ReadRecords,
    RecordDetail,
    RecordEntry,
    RecordPurpose,
    RecordsCoverage,
    RecordsPage,
    RecordsQuery,
    RetentionChange,
    RetentionChanges,
    RetentionExpectation,
    RetentionState,
    RevisionCause,
    TimePoint,
)
from nous_sdk.contracts import EntityBody as EntityBody
from nous_sdk.contracts.common import Contract, utc_now
from nous_sdk.contracts.fact_body import FactClaimBody
from nous_sdk.contracts.retention import DecayRetention, RetentionDecayResult
from pydantic import Field, JsonValue, ValidationError
from sqlalchemy import and_, func, select, true, tuple_
from sqlalchemy import update as update

from nous.identity.access import require_namespace_in
from nous.material.coverage import merge_source_regions
from nous.material.derived import DerivedService
from nous.material.service import (
    MaterialService,
    observations,
    require_observation_in,
    source_permitted,
)
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json
from nous.platform.time.filtering import time_matches
from nous.references import allocate_in

from .entities import contains_record as contains_record
from .entities import redact_attributes_in as redact_attributes_in
from .lifecycle import record_state, selectable, snapshot, state_for, transition
from .projection import media_references_in, references_in
from .projection import evidence_in as evidence_in
from .revision_state import RevisionState
from .schema import revisions
from .schema import vectors as vectors
from .semantics import capability_for, validate_domain_in

if TYPE_CHECKING:
    from nous.vector_projection.admission import ProjectionAdmission

    from .models import ModelServices


class _Cursor(Contract):
    binding: str = Field(min_length=64, max_length=64)
    snapshot: int = Field(ge=0)
    sequence: int = Field(ge=0)
    object_id: str = Field(min_length=1)
    revision: int = Field(ge=1)

    def encode(self) -> str:
        return base64.urlsafe_b64encode(self.model_dump_json().encode()).decode().rstrip("=")


def _matches(record: CognitiveRecord, command: RecordsQuery, roots: dict[str, str]) -> bool:
    if command.text is not None:
        wanted = command.text.strip().casefold()
        if command.text_match == "label":
            if (
                not isinstance(record.body, EntityBody)
                or record.body.label.strip().casefold() != wanted
            ):
                return False
        elif wanted not in body_text(record.body).casefold():
            return False
    if command.purpose == "current" and (not selectable(record)):
        return False
    if command.purpose == "exact" and record.deleted:
        return False
    if command.states and record_state(record) not in command.states:
        return False
    if command.source_ids is not None and (
        not any(
            oid in command.source_ids or roots.get(oid) in command.source_ids
            for oid in record.source_roots
        )
    ):
        return False
    if command.entity_ids:
        entities = {ref.id for ref in references_in(record.body) if ref.kind == "entity"}
        if record.reference.kind == "entity":
            entities.add(record.reference.id)
        if not entities.intersection(command.entity_ids):
            return False
    return time_matches(record, command.times)


class CognitiveRecords:
    def __init__(
        self,
        authority: Authority,
        *,
        materials: MaterialService | None = None,
        derivations: DerivedService | None = None,
        models: ModelServices | None = None,
        projection: ProjectionAdmission | None = None,
    ) -> None:
        self.authority = authority
        self.materials = materials or MaterialService(authority)
        self.derivations = derivations or DerivedService(authority, self.materials)
        self.models = models
        self.projection = projection

    async def source_roots_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        *,
        purpose: RecordPurpose = "current",
        include_purged: bool = False,
    ):
        from nous.material.read_facts import source_facts_in

        facts = await source_facts_in(view, ctx)
        if facts is not None:
            return {
                event.id: event.material_id
                for event in facts.observations.values()
                if (purpose == "management" or event.active)
                and (include_purged or not event.purged)
            }
        rows = (
            await view.connection.execute(
                select(observations.c.id, observations.c.material_id).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.active.is_(True) if purpose != "management" else true(),
                    observations.c.record["purged"].as_boolean().is_not(True)
                    if not include_purged
                    else true(),
                )
            )
        ).all()
        return {str(row.id): str(row.material_id) for row in rows}

    async def visible_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        record: CognitiveRecord | MediaRecord,
        roots: dict[str, str] | None = None,
        *,
        purpose: RecordPurpose = "exact",
    ) -> bool:
        if record.namespace_id != ctx.namespace_id:
            return False
        roots = (
            roots if roots is not None else await self.source_roots_in(view, ctx, purpose=purpose)
        )
        if (
            isinstance(record, CognitiveRecord)
            and (not record.purged)
            and (purpose != "management")
            and (not await self.support_available_in(view, ctx, record, roots))
        ):
            return False
        live = [oid for oid in record.source_roots if oid in roots]
        from nous.material.read_facts import source_facts_in

        facts = await source_facts_in(view, ctx)
        contact_sources = (
            {
                identity: event.contact_source
                for identity in live
                if (event := facts.observations.get(identity)) is not None
                and event.kind == "contact"
                and (event.contact_source is not None)
            }
            if facts
            else {
                str(row.id): MediaReference.model_validate(row.record["contact_source"])
                for row in (
                    await view.connection.execute(
                        select(observations.c.id, observations.c.record).where(
                            observations.c.namespace_id == ctx.namespace_id,
                            observations.c.id.in_(live),
                            observations.c.kind == "contact",
                        )
                    )
                ).all()
                if row.record.get("contact_source") is not None
            }
        )
        support = set(record.support_roots or record.source_roots)
        if not support.intersection(roots):
            return False
        for group in ctx.source_groups():
            if record.reference.exact() in group or record.reference.id in group:
                continue
            scoped = ctx.for_source_group(group)
            media_roots: set[str] = set()
            if isinstance(record, CognitiveRecord):
                for media in dict.fromkeys(
                    (*media_references_in(record.body), *contact_sources.values())
                ):
                    try:
                        if media.kind in {"representation", "derived_region"}:
                            info = await self.derivations.source_in(view, scoped, media)
                            media_roots.update(info.roots)
                        else:
                            provenance = await self.materials.provenance_in(
                                view, scoped, media, derivations=self.derivations
                            )
                            media_roots.update(provenance.roots)
                        media_roots.update(
                            (oid for oid, source in contact_sources.items() if source == media)
                        )
                    except DomainError as error:
                        if error.code not in {
                            "source_unavailable",
                            "region_not_found",
                            "representation_not_found",
                            "derived_region_not_found",
                        }:
                            raise
            if not all(
                oid in media_roots
                or source_permitted(scoped, roots[oid], oid)
                or (
                    bool(record.source_regions.get(oid))
                    and all(
                        region in group
                        or f"source-region:{region}" in group
                        or f"source_region:{region}" in group
                        for region in record.source_regions[oid]
                    )
                )
                for oid in live
            ):
                return False
        return True

    async def get_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        reference: CognitiveRef,
        *,
        check_visibility: bool = True,
        purpose: RecordPurpose = "exact",
    ):
        row = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.id == reference.id,
                    revisions.c.revision == reference.revision,
                    revisions.c.kind == reference.kind,
                    revisions.c.namespace_id == ctx.namespace_id,
                )
            )
        ).scalar_one_or_none()
        if row is None:
            raise DomainError("cognition_not_found", "Cognitive revision is unavailable")
        record = CognitiveRecord.model_validate(row)
        if purpose == "current":
            from .lifecycle import selectable

            head = await view.connection.scalar(
                select(revisions.c.revision).where(
                    revisions.c.id == reference.id,
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.head.is_(True),
                )
            )
            if head != reference.revision or not selectable(record):
                raise DomainError(
                    "cognition_inactive", "The revision is not current available cognition"
                )
        if check_visibility and (not await self.visible_in(view, ctx, record, purpose=purpose)):
            raise DomainError(
                "source_unavailable", "Cognitive sources are unavailable in this scope"
            )
        return record

    async def read_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        reference: CognitiveRef,
        *,
        purpose: RecordPurpose = "exact",
    ) -> CognitiveRecord:
        if purpose == "management":
            return await self.get_in(view, ctx, reference, purpose=purpose)
        record = await self.get_in(view, ctx, reference, check_visibility=False, purpose=purpose)
        record = await self.project_record_in(view, ctx, record)
        if not await self.visible_in(view, ctx, record, purpose=purpose):
            raise DomainError(
                "source_unavailable", "Cognitive sources are unavailable in this scope"
            )
        return record

    async def adopt_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        command: AdoptCognition,
        *,
        state: RevisionState | None = None,
        retiring_entities: frozenset[str] = frozenset(),
    ):
        """Validate a domain proposal before submitting its revision."""
        if command.body.kind == "fact_claim":
            raise DomainError("fact_operation_required", "Use the Fact operations to change claims")
        object_id = command.id or str(uuid4())
        await validate_domain_in(
            tx,
            ctx,
            command.body,
            object_id,
            {
                ref.exact(): (await self.get_in(tx, ctx, ref)).body
                for ref in references_in(command.body)
            },
            retiring_entities,
        )
        return await self.commit_in(
            tx, ctx, command.model_copy(update={"id": object_id}), state=state
        )

    async def commit_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        command: AdoptCognition,
        *,
        state: RevisionState | None = None,
    ) -> CognitiveRecord:
        """Commit a domain's validated change with common revision and source rules."""
        await require_namespace_in(tx, ctx, capability=capability_for(command.body))
        object_id = command.id or str(uuid4())
        head = (
            await tx.connection.execute(
                select(revisions.c.revision).where(
                    revisions.c.id == object_id,
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.head.is_(True),
                )
            )
        ).scalar_one_or_none()
        if (head or 0) != command.expected_revision:
            raise DomainError(
                "revision_conflict",
                "Cognition was changed since the expected revision",
                expected=command.expected_revision,
                actual=head or 0,
            )
        existing_record: CognitiveRecord | None = None
        if head:
            existing_record = CognitiveRecord.model_validate(
                (
                    await tx.connection.execute(
                        select(revisions.c.record).where(
                            revisions.c.id == object_id, revisions.c.revision == head
                        )
                    )
                ).scalar_one()
            )
            if existing_record.purged:
                raise DomainError(
                    "cognition_purged", "A purged cognitive identity cannot be revived"
                )
            existing_kind = (
                await tx.connection.execute(
                    select(revisions.c.kind).where(
                        revisions.c.id == object_id, revisions.c.revision == head
                    )
                )
            ).scalar_one()
            if existing_kind != command.body.kind:
                raise DomainError(
                    "cognitive_kind_immutable", "A cognitive identity cannot change its domain kind"
                )
        if command.body.kind == "episode" and (
            existing_record is None or command.body != existing_record.body
        ):
    
            await require_namespace_in(tx, ctx, "episodes")
        roots: set[str] = set()
        support: set[str] = set()
        requirements: list[tuple[str, tuple[str, ...]]] = []
        unchanged_lifecycle = (
            state is not None
            and state.cause.action in {"trash", "restore"}
            and (existing_record is not None)
            and (command.body == existing_record.body)
        )
        cutoff = (
            command.body.knowledge_cutoff
            if isinstance(command.body, JournalBody) and (not unchanged_lifecycle)
            else None
        )
        if cutoff is not None and cutoff > tx.commit_seq:
            raise DomainError("knowledge_cutoff", "Journal perspective is outside recorded history")

        def require_known(sequence: int) -> None:
            if cutoff is not None and sequence > cutoff:
                raise DomainError(
                    "knowledge_cutoff", "Journal cites knowledge after its writing perspective"
                )

        def require_root(observation_id: str, region_ids: tuple[str, ...]) -> None:
            roots.add(observation_id)
            requirements.append((observation_id, region_ids))

        body_evidence = tuple(evidence_in(command.body))
        for evidence in (*command.evidence, *body_evidence):
            if evidence.observation_id:
                observation = await require_observation_in(
                    tx, ctx, evidence.observation_id, region_id=evidence.region_id
                )
                if evidence.polarity != "boundary":
                    require_known(int(observation["commit_seq"]))
                source_ref = MediaReference(
                    kind="source_region" if evidence.region_id else "material",
                    id=evidence.region_id or observation["material_id"],
                    observation_id=evidence.observation_id,
                )
                provenance = await self.materials.provenance_in(
                    tx, ctx, source_ref, derivations=self.derivations
                )
                for root in provenance.roots:
                    require_root(root, provenance.regions.get(root, ()))
                if evidence.polarity == "support":
                    support.update(provenance.roots)
                if isinstance(command.body, MemoryBody) and provenance.roots != (
                    evidence.observation_id,
                ):
                    command = command.model_copy(
                        update={
                            "body": command.body.model_copy(
                                update={
                                    "media": tuple(dict.fromkeys((*command.body.media, source_ref)))
                                }
                            )
                        }
                    )
            if evidence.reference:
                source = await self.read_in(tx, ctx, evidence.reference)
                if evidence.polarity != "boundary":
                    require_known(source.commit_seq)
                if source.purged and evidence.polarity == "support":
                    raise DomainError(
                        "cognition_purged", "A purged revision cannot support new cognitive content"
                    )
                for observation_id in source.source_roots:
                    require_root(observation_id, source.source_regions.get(observation_id, ()))
                if evidence.polarity == "support":
                    support.update(source.support_roots or source.source_roots)
        for media in media_references_in(command.body):
            if media.kind in {"representation", "derived_region"}:
                info = await self.derivations.source_in(tx, ctx, media)
                for observation_id in info.roots:
                    require_root(observation_id, info.regions.get(observation_id, ()))
                support.update(info.roots)
            else:
                provenance = await self.materials.provenance_in(
                    tx, ctx, media, derivations=self.derivations
                )
                for root in provenance.roots:
                    require_root(root, provenance.regions.get(root, ()))
                support.update(provenance.roots)
        for reference in references_in(command.body):
            if reference.id == object_id and reference.revision >= (head or 0) + 1:
                raise DomainError("self_reference", "A revision cannot reference itself")
            source = await self.read_in(tx, ctx, reference)
            for observation_id in source.source_roots:
                require_root(observation_id, source.source_regions.get(observation_id, ()))
            if isinstance(command.body, JournalBody) and reference in command.body.episodes:
                require_known(source.commit_seq)
        if isinstance(command.body, MemoryBody) and command.body.contact_observation_id:
            contact_id = command.body.contact_observation_id
            contact = await tx.connection.scalar(
                select(observations.c.record).where(
                    observations.c.id == contact_id,
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.kind == "contact",
                    observations.c.active.is_(True),
                )
            )
            if (
                contact is None
                or MediaReference.model_validate(contact.get("contact_source"))
                not in command.body.media
            ):
                raise DomainError(
                    "contact_source_mismatch", "Memory must retain its actual encountered selection"
                )
            selected = MediaReference.model_validate(contact["contact_source"])
            require_root(contact_id, (selected.id,) if selected.kind == "source_region" else ())
        if not support:
            raise DomainError(
                "grounding_required", "Cognitive adoption requires a live supporting source"
            )
        now = ctx.clock
        times = command.times or CognitiveTimes(formation=now, recorded=now)
        recorded = TimePoint(
            coordinate=int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        )
        times = times.model_copy(update={"recorded": recorded})
        state = state or RevisionState(
            cause=RevisionCause(
                action="adopt",
                actor=ctx.caller_id,
                reason="cognitive adoption",
                parent=existing_record.reference if existing_record else None,
            ),
            lifecycle=(
                existing_record.lifecycle
                if existing_record and existing_record.lifecycle
                else RetentionState(
                    anchor_unix_ms=existing_record.times.recorded.coordinate
                    if existing_record
                    else recorded.coordinate
                )
            )
            if command.body.kind in {"memory", "association"}
            else None,
            deleted=existing_record.deleted if existing_record else False,
        )
        if (
            command.body.kind in {"memory", "association"}
            and state.lifecycle is None
            and (not state.purged)
        ):
            from dataclasses import replace

            from .lifecycle import state_for

            state = replace(
                state,
                lifecycle=state_for(existing_record)
                if existing_record is not None
                else RetentionState(anchor_unix_ms=recorded.coordinate),
            )
        result = CognitiveRecord(
            reference=CognitiveRef(kind=command.body.kind, id=object_id, revision=(head or 0) + 1),
            namespace_id=ctx.namespace_id,
            body=None if state.purged else command.body,
            evidence=command.evidence,
            source_roots=tuple(sorted(roots)),
            support_roots=tuple(sorted(support)),
            source_regions=merge_source_regions(requirements),
            times=times,
            producer_call_id=command.producer_call_id,
            commit_seq=tx.commit_seq,
            lifecycle=state.lifecycle,
            deleted=state.deleted,
            purged=state.purged,
            cause=state.cause,
        )
        result = result.model_copy(
            update={"addresses": await allocate_in(tx, ctx, result.reference)}
        )
        if head:
            await tx.connection.execute(
                update(revisions)
                .where(revisions.c.id == object_id, revisions.c.head.is_(True))
                .values(head=False)
            )
        await tx.connection.execute(
            revisions.insert().values(
                id=object_id,
                revision=result.reference.revision,
                namespace_id=ctx.namespace_id,
                kind=result.reference.kind,
                head=True,
                record=result.model_dump(mode="json"),
                commit_seq=tx.commit_seq,
            )
        )
        if self.projection is not None and (
            not (
                command.body.kind in {"memory", "association"}
                and existing_record is not None
                and (command.body == existing_record.body)
                and (
                    state.cause.action
                    in {
                        "reinforce",
                        "weaken",
                        "protect",
                        "unprotect",
                        "pin",
                        "unpin",
                        "freeze",
                        "forget",
                        "restore",
                        "trash",
                        "purge",
                        "decay",
                        "remember_forever",
                        "allow_decay",
                    }
                )
            )
        ):
            await self.projection.changed_in(tx, ctx, result)
        from nous.retrieval.maintenance import enqueue_index_in

        await enqueue_index_in(tx, ctx)
        return result

    async def adopt(self, ctx: CallContext, command: AdoptCognition):
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="cognition.adopt",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return await self.replay_record_in(
                    tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
                )
            return tx.result(await self.adopt_in(tx, ctx, command))

    async def get(self, ctx: CallContext, reference: CognitiveRef):
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            return await self.read_in(view, ctx, reference)

    async def records_in(
        self: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, command: RecordsQuery
    ) -> RecordsPage:
        await require_namespace_in(view, ctx)
        binding = hashlib.sha256(
            canonical_json(
                (
                    ctx.caller_id,
                    ctx.namespace_id,
                    ctx.namespace_id,
                    ctx.source_groups(),
                    command.model_dump(mode="json", exclude={"cursor", "limit"}),
                )
            ).encode()
        ).hexdigest()
        position: _Cursor | None = None
        if command.cursor is not None:
            try:
                position = _Cursor.model_validate_json(
                    base64.b64decode(
                        command.cursor + "=" * (-len(command.cursor) % 4),
                        altchars=b"-_",
                        validate=True,
                    )
                )
            except (ValidationError, ValueError, binascii.Error) as error:
                raise DomainError("invalid_cursor", "Record cursor is invalid") from error
            if position.binding != binding:
                raise DomainError(
                    "invalid_cursor", "Record cursor belongs to a different query or scope"
                )
        snapshot = (
            position.snapshot
            if position
            else command.as_of_commit
            if command.as_of_commit is not None
            else view.commit_seq
        )
        if snapshot > view.commit_seq:
            raise DomainError("invalid_history", "Record snapshot is outside committed history")
        query = select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id, revisions.c.commit_seq <= snapshot
        )
        if command.view == "current":
            latest = (
                select(revisions.c.id, func.max(revisions.c.revision).label("revision"))
                .where(
                    revisions.c.namespace_id == ctx.namespace_id, revisions.c.commit_seq <= snapshot
                )
                .group_by(revisions.c.id)
                .subquery()
            )
            query = query.join(
                latest,
                and_(revisions.c.id == latest.c.id, revisions.c.revision == latest.c.revision),
            )
        if command.kinds:
            query = query.where(revisions.c.kind.in_(command.kinds))
        if command.object_ids:
            query = query.where(revisions.c.id.in_(command.object_ids))
        order = (revisions.c.commit_seq, revisions.c.id, revisions.c.revision)
        query = query.order_by(*(column.desc() for column in order))
        roots = await self.source_roots_in(view, ctx, purpose=command.purpose)
        collected: list[CognitiveRecord] = []
        examined = 0
        batch_size = max(128, command.limit + 1)
        exhausted = False
        while len(collected) <= command.limit:
            batch = query
            if position is not None:
                batch = batch.where(
                    tuple_(*order) < (position.sequence, position.object_id, position.revision)
                )
            rows = (await view.connection.execute(batch.limit(batch_size))).scalars().all()
            if not rows:
                exhausted = True
                break
            for raw in rows:
                record = CognitiveRecord.model_validate(raw)
                if command.purpose != "management":
                    record = await self.project_record_in(view, ctx, record)
                examined += 1
                position = _Cursor(
                    binding=binding,
                    snapshot=snapshot,
                    sequence=record.commit_seq,
                    object_id=record.reference.id,
                    revision=record.reference.revision,
                )
                if _matches(record, command, roots) and await self.visible_in(
                    view, ctx, record, roots, purpose=command.purpose
                ):
                    collected.append(record)
                    if len(collected) > command.limit:
                        break
            if len(collected) > command.limit:
                break
            if len(rows) < batch_size:
                exhausted = True
                break
        next_cursor = None
        if len(collected) > command.limit:
            last = collected[command.limit - 1]
            next_cursor = _Cursor(
                binding=binding,
                snapshot=snapshot,
                sequence=last.commit_seq,
                object_id=last.reference.id,
                revision=last.reference.revision,
            ).encode()
        records = tuple(collected[: command.limit])
        heads: dict[str, CognitiveRef] = {}
        if records:
            for raw in (
                await view.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.id.in_([record.reference.id for record in records]),
                        revisions.c.head.is_(True),
                    )
                )
            ).scalars():
                head = CognitiveRecord.model_validate(raw)
                if await self.visible_in(view, ctx, head, roots, purpose=command.purpose):
                    heads[head.reference.id] = head.reference
        return RecordsPage(
            items=tuple(
                RecordEntry(
                    record=record,
                    current_reference=heads.get(record.reference.id),
                    state=record_state(record),
                )
                for record in records
            ),
            next_cursor=next_cursor,
            coverage=RecordsCoverage(
                snapshot_commit=snapshot,
                authorization_commit=view.commit_seq,
                examined=examined,
                returned=len(records),
                complete=exhausted,
            ),
        )

    async def records(
        self: CognitiveRecords, ctx: CallContext, command: RecordsQuery
    ) -> RecordsPage:
        async with self.authority.read(ctx) as view:
            return await self.records_in(view, ctx, command)

    async def read_records(
        self: CognitiveRecords, ctx: CallContext, command: ReadRecords
    ) -> tuple[CognitiveRecord, ...]:
        async with self.authority.read(ctx) as view:
            return await self.read_records_in(view, ctx, command)

    async def read_records_in(
        self: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, command: ReadRecords
    ) -> tuple[CognitiveRecord, ...]:
        await require_namespace_in(view, ctx)
        values = (
            await view.connection.execute(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    tuple_(revisions.c.kind, revisions.c.id, revisions.c.revision).in_(
                        [(ref.kind, ref.id, ref.revision) for ref in command.references]
                    ),
                )
            )
        ).scalars()
        by_ref = {
            record.reference: record
            for record in (CognitiveRecord.model_validate(value) for value in values)
        }
        heads: dict[str, int] = {}
        if command.purpose == "current":
            heads = {
                str(row.id): int(row.revision)
                for row in (
                    await view.connection.execute(
                        select(revisions.c.id, revisions.c.revision).where(
                            revisions.c.namespace_id == ctx.namespace_id,
                            revisions.c.id.in_([ref.id for ref in command.references]),
                            revisions.c.head.is_(True),
                        )
                    )
                ).all()
            }
        roots = await self.source_roots_in(view, ctx, purpose=command.purpose)
        result: list[CognitiveRecord] = []
        for reference in command.references:
            record = by_ref.get(reference)
            if record is None:
                raise DomainError("cognition_not_found", "Cognitive revision is unavailable")
            if command.purpose != "management":
                record = await self.project_record_in(view, ctx, record)
            if command.purpose == "current" and (
                heads.get(reference.id) != reference.revision or not selectable(record)
            ):
                raise DomainError(
                    "cognition_inactive", "The revision is not current available cognition"
                )
            if command.purpose != "management" and record.deleted:
                raise DomainError(
                    "cognition_deleted", "Deleted revisions require management reading"
                )
            if not await self.visible_in(view, ctx, record, roots, purpose=command.purpose):
                raise DomainError(
                    "source_unavailable", "Cognitive sources are unavailable in this scope"
                )
            result.append(record)
        return tuple(result)

    async def record_detail(
        self: CognitiveRecords,
        ctx: CallContext,
        reference: CognitiveRef,
        history_limit: int = 100,
        purpose: RecordPurpose = "exact",
    ) -> RecordDetail:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            record = await self.read_in(view, ctx, reference, purpose=purpose)
            if purpose != "management" and record.deleted:
                raise DomainError(
                    "cognition_deleted", "Deleted revisions require management reading"
                )
            history = await self.records_in(
                view,
                ctx,
                RecordsQuery(
                    object_ids=(reference.id,), view="history", purpose=purpose, limit=history_limit
                ),
            )
            current = next((entry.current_reference for entry in history.items), None)
            return RecordDetail(
                entry=RecordEntry(
                    record=record, current_reference=current, state=record_state(record)
                ),
                history=history,
            )

    async def require_retention_preconditions_in(
        self: CognitiveRecords,
        view: ReadView | WriteTx,
        ctx: CallContext,
        expectations: tuple[RetentionExpectation, ...],
    ) -> None:
        if not expectations:
            return
        await require_namespace_in(view, ctx)
        for expected in expectations:
            raw = await view.connection.scalar(
                select(revisions.c.record).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.kind == expected.reference.kind,
                    revisions.c.id == expected.reference.id,
                    revisions.c.head.is_(True),
                )
            )
            if raw is None:
                raise DomainError(
                    "retention_precondition_failed",
                    "The expected retained object is no longer present",
                    reference=expected.reference.exact(),
                )
            record = CognitiveRecord.model_validate(raw)
            await self.get_in(view, ctx, record.reference, purpose="management")
            if record.purged or record.reference.kind not in {"memory", "association"}:
                raise DomainError(
                    "retention_precondition_failed",
                    "The expected retained object is unavailable",
                    reference=expected.reference.exact(),
                )
            actual = snapshot(
                record,
                int(datetime.fromisoformat(view.recorded_at).timestamp() * 1000)
                if isinstance(view, WriteTx)
                else utc_now().coordinate,
            )
            actual_fields = actual.model_dump(mode="json", exclude={"score"})
            wanted = expected.model_dump(mode="json")
            if actual_fields != wanted:
                raise DomainError(
                    "retention_precondition_failed",
                    "Retention state changed after selection",
                    reference=expected.reference.exact(),
                    current=record.reference.exact(),
                    fields=[name for name in wanted if actual_fields.get(name) != wanted[name]],
                )

    async def change_retention_in(
        self: CognitiveRecords, tx: WriteTx, ctx: CallContext, command: RetentionChange
    ) -> CognitiveRecord:
        previous = await self.get_in(tx, ctx, command.reference)
        lifecycle = state_for(previous)
        current = await tx.connection.scalar(
            select(revisions.c.revision).where(
                revisions.c.id == previous.reference.id,
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.head.is_(True),
            )
        )
        if current != command.reference.revision:
            raise DomainError(
                "retention_revision_conflict",
                "The retained object changed after selection",
                reference=command.reference.exact(),
            )
        if previous.deleted and command.action not in {"restore", "purge"}:
            raise DomainError("retention_deleted", "Restore the retained object before changing it")
        if command.hours is not None and command.action != "protect":
            raise DomainError("invalid_retention_hours", "Hours applies to protection")
        now = int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        if (
            command.action in {"trash", "purge"}
            and lifecycle.protected(now)
            and (not command.force)
        ):
            raise DomainError(
                "retention_protected",
                "The retained object is protected; unprotect it or explicitly force the change",
            )
        updated = transition(lifecycle, command, now)
        deleted = (
            command.action in {"trash", "purge"}
            if command.action in {"trash", "purge", "restore"}
            else previous.deleted
        )
        if updated == lifecycle and deleted == previous.deleted and (command.action != "purge"):
            return previous
        assert previous.body is not None
        result = await self.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=previous.reference.revision,
                body=previous.body,
                evidence=(
                    EvidenceLink(
                        reference=previous.reference, role=command.reason or command.action
                    ),
                ),
                times=previous.times,
                producer_call_id=previous.producer_call_id,
            ),
            state=RevisionState(
                lifecycle=updated,
                deleted=deleted,
                purged=command.action == "purge",
                cause=RevisionCause(
                    action=command.action,
                    actor=ctx.caller_id,
                    reason=command.reason,
                    parent=previous.reference,
                ),
            ),
        )
        if command.action == "purge":
            await self.erase_history_in(tx, ctx, previous.reference.id)
        else:
            for asset in (
                await tx.connection.execute(
                    select(vectors).where(vectors.c.reference == previous.reference.exact())
                )
            ).mappings():
                await tx.connection.execute(
                    vectors.insert().values(
                        **{
                            **asset,
                            "reference": result.reference.exact(),
                            "commit_seq": tx.commit_seq,
                        }
                    )
                )
        return result

    async def change_retentions_in(
        self: CognitiveRecords, tx: WriteTx, ctx: CallContext, command: RetentionChanges
    ) -> tuple[CognitiveRecord, ...]:
        if len({ref.id for ref in command.references}) != len(command.references):
            raise DomainError("retention_selection", "Select each retained identity once")
        for reference in command.references:
            record = await self.get_in(tx, ctx, reference)
            state_for(record)
            current = await tx.connection.scalar(
                select(revisions.c.revision).where(
                    revisions.c.id == reference.id,
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.head.is_(True),
                )
            )
            if current != reference.revision:
                raise DomainError(
                    "retention_revision_conflict",
                    "The frozen lifecycle selection changed",
                    reference=reference.exact(),
                )
        return tuple(
            [
                await self.change_retention_in(
                    tx,
                    ctx,
                    RetentionChange(
                        reference=reference,
                        action=command.action,
                        strength=command.strength,
                        hours=command.hours,
                        reason=command.reason,
                        force=command.force,
                    ),
                )
                for reference in command.references
            ]
        )

    async def change_retention(
        self: CognitiveRecords, ctx: CallContext, command: RetentionChange
    ) -> CognitiveRecord:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="retention.change",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return await self.replay_record_in(
                    tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
                )
            result = tx.result(await self.change_retention_in(tx, ctx, command))
        return result

    async def change_retentions(
        self: CognitiveRecords, ctx: CallContext, command: RetentionChanges
    ) -> tuple[CognitiveRecord, ...]:
        from pydantic import TypeAdapter

        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="retention.change_many",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return await self.replay_records_in(
                    tx,
                    ctx,
                    TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result),
                )
            result = tx.result(await self.change_retentions_in(tx, ctx, command))
        return result

    async def retention_snapshot(self: CognitiveRecords, ctx: CallContext, reference: CognitiveRef):
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            return snapshot(
                await self.get_in(view, ctx, reference, purpose="management"), utc_now().coordinate
            )

    async def decay_retention_in(
        self: CognitiveRecords, tx: WriteTx, ctx: CallContext, command: DecayRetention
    ) -> RetentionDecayResult:
        page = await self.records_in(
            tx, ctx, RecordsQuery(kinds=command.kinds, limit=command.limit, cursor=command.cursor)
        )
        now = int(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
        changed: list[CognitiveRef] = []
        for entry in page.items:
            before = state_for(entry.record)
            change = RetentionChange(
                reference=entry.record.reference, action="decay", reason="retention evaluation"
            )
            if transition(before, change, now) != before:
                changed.append((await self.change_retention_in(tx, ctx, change)).reference)
        return RetentionDecayResult(
            scanned=len(page.items),
            changed=tuple(changed),
            next_cursor=page.next_cursor,
            complete=page.next_cursor is None,
        )

    async def decay_retention(
        self: CognitiveRecords, ctx: CallContext, command: DecayRetention
    ) -> RetentionDecayResult:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="retention.decay",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return RetentionDecayResult.model_validate(tx.replay_result)
            result = tx.result(await self.decay_retention_in(tx, ctx, command))
        return result

    async def retirement_plan_in(
        self: CognitiveRecords,
        view: ReadView | WriteTx,
        ctx: CallContext,
        object_ids: frozenset[str],
        observation_ids: frozenset[str],
    ) -> tuple[CognitiveRecord, ...]:
        _, records, supported = await self.support_snapshot_in(
            view, ctx, object_ids, observation_ids
        )
        heads: dict[str, CognitiveRecord] = {}
        for record in records.values():
            previous = heads.get(record.reference.id)
            if previous is None or previous.reference.revision < record.reference.revision:
                heads[record.reference.id] = record
        return tuple(
            record
            for record in heads.values()
            if not record.purged
            and (record.reference.id in object_ids or record.reference.exact() not in supported)
        )

    async def retire_unsupported_in(
        self: CognitiveRecords, tx: WriteTx, ctx: CallContext
    ) -> tuple[CognitiveRef, ...]:
        sources, records, supported = await self.support_snapshot_in(tx, ctx)
        retired: list[CognitiveRef] = []
        for record in records.values():
            if record.purged or record.reference.exact() in supported:
                continue
            tombstone = record.model_copy(
                update={
                    "body": None,
                    "deleted": True,
                    "purged": True,
                    "evidence": tuple(
                        link.model_copy(update={"role": "purged"}) for link in record.evidence
                    ),
                }
            )
            await tx.connection.execute(
                update(revisions)
                .where(
                    revisions.c.id == record.reference.id,
                    revisions.c.revision == record.reference.revision,
                )
                .values(record=tombstone.model_dump(mode="json"))
            )
            await tx.connection.execute(
                vectors.delete().where(vectors.c.reference == record.reference.exact())
            )
            retired.append(record.reference)
        for record in records.values():
            if record.reference.exact() in supported and await redact_attributes_in(
                tx, record, sources
            ):
                retired.append(record.reference)
        if retired:
            keys = {reference.exact() for reference in retired}
            await self.authority.redact_receipts_in(tx, lambda value: contains_record(value, keys))
        return tuple(retired)

    async def qualify_known_outputs_in(
        self: CognitiveRecords,
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
                "input_admission_unavailable",
                "Known outputs require actual submitted-input provenance",
            )
        records = [
            await self.get_in(view, ctx, reference, check_visibility=False)
            for reference in references
        ]
        exacts = {
            record.reference.exact()
            for record in records
            if record.commit_seq == admission.commit_seq and (not record.purged)
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

    async def support_available_in(
        self: CognitiveRecords,
        view: ReadView | WriteTx,
        ctx: CallContext,
        record: CognitiveRecord,
        roots: dict[str, str],
        path: frozenset[str] = frozenset(),
        *,
        withdrawn_observations: frozenset[str] = frozenset(),
        purged_observations: frozenset[str] = frozenset(),
        purged_cognition: frozenset[str] = frozenset(),
    ) -> bool:
        """Inspect declared proof liveness; caller source permissions remain with visible_in."""
        identity = record.reference.exact()
        if (
            record.purged
            or record.reference.id in purged_cognition
            or identity in purged_cognition
            or (identity in path)
            or (not set(record.support_roots or record.source_roots).intersection(roots))
        ):
            return False
        if isinstance(record.body, MemoryBody) and record.body.contact_observation_id is not None:
            if record.body.contact_observation_id not in roots:
                return False
        media = tuple(media_references_in(record.body))
        for reference in media:
            if (
                await self.derivations.availability_in(
                    view,
                    ctx,
                    reference,
                    withdrawn_observations=withdrawn_observations,
                    purged_observations=purged_observations,
                    purged_cognition=purged_cognition,
                )
                != "available"
            ):
                return False
        if media:
            return True
        links = tuple(link for link in record.evidence if link.polarity == "support") or tuple(
            link for link in evidence_in(record.body) if link.polarity == "support"
        )
        for link in links:
            if link.observation_id in roots:
                assert link.observation_id is not None
                source = MediaReference(
                    kind="source_region" if link.region_id else "material",
                    id=link.region_id or roots[link.observation_id],
                    observation_id=link.observation_id,
                )
                if (
                    await self.derivations.availability_in(
                        view,
                        ctx,
                        source,
                        withdrawn_observations=withdrawn_observations,
                        purged_observations=purged_observations,
                        purged_cognition=purged_cognition,
                    )
                    == "available"
                ):
                    return True
            if link.reference is not None:
                raw = await view.connection.scalar(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.kind == link.reference.kind,
                        revisions.c.id == link.reference.id,
                        revisions.c.revision == link.reference.revision,
                    )
                )
                if raw is not None and await self.support_available_in(
                    view,
                    ctx,
                    CognitiveRecord.model_validate(raw),
                    roots,
                    path | {identity},
                    withdrawn_observations=withdrawn_observations,
                    purged_observations=purged_observations,
                    purged_cognition=purged_cognition,
                ):
                    return True
        return False

    async def history(
        self: CognitiveRecords, ctx: CallContext, object_id: str, limit: int = 100
    ) -> tuple[CognitiveRecord, ...]:
        if not 1 <= limit <= 1000:
            raise DomainError("invalid_limit", "History limit must be between 1 and 1000")
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            values = (
                await view.connection.execute(
                    select(revisions.c.record)
                    .where(
                        revisions.c.namespace_id == ctx.namespace_id, revisions.c.id == object_id
                    )
                    .order_by(revisions.c.revision.desc())
                    .limit(limit)
                )
            ).scalars()
            result: list[CognitiveRecord] = []
            for value in values:
                record = await self.project_record_in(
                    view, ctx, CognitiveRecord.model_validate(value)
                )
                if await self.visible_in(view, ctx, record):
                    result.append(record)
            return tuple(result)

    async def concept_by_alias(
        self: CognitiveRecords, ctx: CallContext, label: str
    ) -> CognitiveRecord:
        key = " ".join(label.casefold().split())
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            values = (
                await view.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.kind == "concept",
                        revisions.c.head.is_(True),
                    )
                )
            ).scalars()
            matches: list[CognitiveRecord] = []
            for value in values:
                record = CognitiveRecord.model_validate(value)
                if (
                    isinstance(record.body, ConceptBody)
                    and (not record.deleted)
                    and (record.body.state == "active")
                    and (
                        key
                        in {
                            " ".join(term.casefold().split())
                            for term in (record.body.label, *record.body.aliases)
                        }
                    )
                    and await self.visible_in(view, ctx, record)
                ):
                    matches.append(record)
            if not matches:
                raise DomainError("concept_not_found", "No accessible Concept matches this alias")
            if len(matches) > 1:
                raise DomainError(
                    "ambiguous_name", "The alias identifies multiple accessible Concepts"
                )
            return matches[0]

    async def set_deleted(
        self: CognitiveRecords,
        ctx: CallContext,
        reference: CognitiveRef,
        deleted: bool,
        operation_id: str,
    ) -> CognitiveRecord:
        async with self.authority.read(ctx) as view:
            previous = await self.get_in(view, ctx, reference)
        if isinstance(previous.body, MemoryBody):
            return await self.change_retention(
                ctx,
                RetentionChange(
                    operation_id=operation_id,
                    reference=reference,
                    action="trash" if deleted else "restore",
                ),
            )
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="cognition.set_deleted",
            scope=ctx.namespace_id,
            payload={"reference": reference.model_dump(mode="json"), "deleted": deleted},
        ) as tx:
            if tx.replayed:
                return await self.replay_record_in(
                    tx, ctx, CognitiveRecord.model_validate(tx.replay_result)
                )
            adopted = await self.set_deleted_in(tx, ctx, reference, deleted)
            result = tx.result(adopted)
        return result

    async def set_deleted_in(
        self: CognitiveRecords,
        tx: WriteTx,
        ctx: CallContext,
        reference: CognitiveRef,
        deleted: bool,
        *,
        evidence: tuple[EvidenceLink, ...] = (),
        reason: str = "Manual cognitive lifecycle change",
    ) -> CognitiveRecord:
        previous = await self.get_in(tx, ctx, reference)
        if previous.reference.kind in {"memory", "association"}:
            from nous_sdk.contracts import RetentionChange

            return await self.change_retention_in(
                tx,
                ctx,
                RetentionChange(
                    reference=reference, action="trash" if deleted else "restore", reason=reason
                ),
            )
        if previous.body is None:
            raise DomainError("cognition_purged", "A purged object cannot be changed")
        from nous_sdk.contracts import RevisionCause

        return await self.commit_in(
            tx,
            ctx,
            AdoptCognition(
                id=reference.id,
                expected_revision=reference.revision,
                body=previous.body,
                evidence=(
                    *previous.evidence,
                    EvidenceLink(reference=reference, polarity="boundary", role=reason),
                    *(link.model_copy(update={"polarity": "boundary"}) for link in evidence),
                ),
                times=previous.times,
                producer_call_id=previous.producer_call_id,
            ),
            state=RevisionState(
                deleted=deleted,
                lifecycle=previous.lifecycle,
                cause=RevisionCause(
                    action="trash" if deleted else "restore",
                    actor=ctx.caller_id,
                    reason=reason,
                    parent=reference,
                ),
            ),
        )

    async def replay_record_in(
        self: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, record: CognitiveRecord
    ) -> CognitiveRecord:
        """An operation receipt never preserves an older, broader source authorization."""
        return await self.get_in(view, ctx, record.reference)

    async def automatic_in(
        self: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef
    ) -> CognitiveRecord | None:
        """Automatic uses retain exact provenance and honor the current domain decision."""
        from .lifecycle import selectable

        exact = await self.read_in(view, ctx, reference)
        if not selectable(exact):
            return None
        raw = await view.connection.scalar(
            select(revisions.c.record).where(
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.id == reference.id,
                revisions.c.head.is_(True),
            )
        )
        if raw is None:
            return None
        head = await self.project_record_in(view, ctx, CognitiveRecord.model_validate(raw))
        if not selectable(head) or not await self.visible_in(view, ctx, head):
            return None
        return exact

    async def replay_records_in(
        self: CognitiveRecords,
        view: ReadView | WriteTx,
        ctx: CallContext,
        records: tuple[CognitiveRecord, ...],
    ) -> tuple[CognitiveRecord, ...]:
        return tuple([await self.replay_record_in(view, ctx, record) for record in records])

    async def project_record_in(
        self: CognitiveRecords, view: ReadView | WriteTx, ctx: CallContext, record: CognitiveRecord
    ) -> CognitiveRecord:
        if not isinstance(record.body, EntityBody):
            return record
        body = record.body
        roots = await self.source_roots_in(view, ctx)
        base_links = await self.live_links_in(view, ctx, record.evidence, roots)
        base_live = bool(base_links)
        attributes = tuple(
            [
                attribute
                for attribute in body.attributes
                if await self.links_live_in(view, ctx, attribute.evidence, roots)
            ]
        )
        controls = body.profile_controls
        if controls.aliases is not None and (
            not await self.links_live_in(view, ctx, controls.aliases.evidence, roots)
        ):
            controls = controls.model_copy(update={"aliases": None})
        if controls.summary is not None and (
            not await self.links_live_in(view, ctx, controls.summary.evidence, roots)
        ):
            controls = controls.model_copy(update={"summary": None})
        generated = body.generated_profile
        if generated is not None and (
            not await self.links_live_in(view, ctx, generated.evidence, roots, require_all=True)
        ):
            generated = None
        body = body.model_copy(
            update={
                "description": body.description if base_live else "",
                "aliases": body.aliases if base_live else (),
                "attributes": attributes,
                "profile_controls": controls,
                "generated_profile": generated,
            }
        )
        supported: set[str] = set()
        for link in (*base_links, *tuple(evidence_in(body))):
            if link.polarity != "support":
                continue
            if link.observation_id is not None and link.observation_id in roots:
                supported.add(link.observation_id)
            elif link.reference is not None:
                raw = await view.connection.scalar(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.id == link.reference.id,
                        revisions.c.revision == link.reference.revision,
                    )
                )
                if raw is not None:
                    source = CognitiveRecord.model_validate(raw)
                    supported.update(root for root in source.source_roots if root in roots)
        return record.model_copy(
            update={
                "body": body,
                "evidence": base_links,
                "source_roots": tuple(sorted(supported)),
                "support_roots": tuple(sorted(supported)),
                "source_regions": {
                    root: selectors
                    for root, selectors in record.source_regions.items()
                    if root in supported
                },
            }
        )

    async def erase_history_in(
        self: CognitiveRecords, tx: WriteTx, ctx: CallContext, identity: str
    ) -> None:
        history = [
            CognitiveRecord.model_validate(raw)
            for raw in (
                await tx.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id, revisions.c.id == identity
                    )
                )
            ).scalars()
        ]
        for record in history:
            tombstone = record.model_copy(update={"body": None, "purged": True, "deleted": True})
            await tx.connection.execute(
                update(revisions)
                .where(
                    revisions.c.id == identity, revisions.c.revision == record.reference.revision
                )
                .values(record=tombstone.model_dump(mode="json"))
            )
        refs = tuple(record.reference for record in history) + await self.retire_unsupported_in(
            tx, ctx
        )
        await tx.connection.execute(
            vectors.delete().where(vectors.c.reference.in_([ref.exact() for ref in refs]))
        )
        from nous.platform.execution import JobSpec
        from nous.retrieval.maintenance import enqueue_index_in

        if tx.authority.execution is not None:
            job_id = f"purge-dependents:{ctx.namespace_id}:{tx.commit_seq}:{identity}"
            await tx.enqueue(
                JobSpec(
                    workflow_name="nous.cognition.purge_dependents",
                    queue_name="default",
                    workflow_id=job_id,
                    args=(
                        ctx.model_dump(mode="json"),
                        [ref.model_dump(mode="json") for ref in refs],
                        job_id,
                    ),
                )
            )
        await enqueue_index_in(tx, ctx)

        def contains(value: JsonValue) -> bool:
            if isinstance(value, list):
                return any(contains(item) for item in value)
            if not isinstance(value, dict):
                return False
            reference = value.get("reference")
            return (
                isinstance(reference, dict)
                and reference.get("id") == identity
                and ("body" in value)
                or any(contains(item) for item in value.values())
            )

        await self.authority.redact_receipts_in(tx, contains)

    async def purge_cognition_in(
        self: CognitiveRecords,
        tx: WriteTx,
        ctx: CallContext,
        reference: CognitiveRef,
        *,
        force: bool = False,
        reason: str,
    ) -> CognitiveRecord:
        previous = await self.get_in(tx, ctx, reference, purpose="management")
        if previous.purged:
            return previous
        if isinstance(previous.body, MemoryBody) and await self.visible_in(tx, ctx, previous):
            return await self.change_retention_in(
                tx,
                ctx,
                RetentionChange(reference=reference, action="purge", force=force, reason=reason),
            )
        if isinstance(previous.body, MemoryBody) and previous.lifecycle is not None:
            now = round(datetime.fromisoformat(tx.recorded_at).timestamp() * 1000)
            if previous.lifecycle.protected(now) and (not force):
                raise DomainError(
                    "memory_protected",
                    "Memory is protected; unprotect or explicitly force the change",
                )
        await self.erase_history_in(tx, ctx, reference.id)
        return previous.model_copy(update={"body": None, "purged": True, "deleted": True})

    async def live_links_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        links: tuple[EvidenceLink, ...],
        roots: dict[str, str],
        *,
        require_current: bool = False,
    ) -> tuple[EvidenceLink, ...]:
        kept: list[EvidenceLink] = []
        for link in links:
            if link.polarity != "support":
                continue
            live = False
            if link.observation_id in roots:
                assert link.observation_id is not None
                reference = MediaReference(
                    kind="source_region" if link.region_id else "material",
                    id=link.region_id or roots[link.observation_id],
                    observation_id=link.observation_id,
                )
                live = await self.derivations.availability_in(view, ctx, reference) == "available"
            elif link.reference is not None:
                raw = await view.connection.scalar(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.id == link.reference.id,
                        revisions.c.revision == link.reference.revision,
                    )
                )
                if raw is not None:
                    source = CognitiveRecord.model_validate(raw)
                    live = await self.support_available_in(view, ctx, source, roots)
                    if live and (require_current or isinstance(source.body, FactClaimBody)):
                        head_raw = await view.connection.scalar(
                            select(revisions.c.record).where(
                                revisions.c.namespace_id == ctx.namespace_id,
                                revisions.c.id == source.reference.id,
                                revisions.c.head.is_(True),
                            )
                        )
                        head = (
                            CognitiveRecord.model_validate(head_raw)
                            if head_raw is not None
                            else None
                        )
                        live = (
                            head is not None
                            and head.reference == link.reference
                            and selectable(head)
                            and (
                                not (
                                    isinstance(head.body, FactClaimBody)
                                    and head.body.status != "active"
                                )
                            )
                        )
            if live:
                kept.append(link)
        return tuple(kept)

    async def links_live_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        links: tuple[EvidenceLink, ...],
        roots: dict[str, str],
        *,
        require_all: bool = False,
    ) -> bool:
        kept = await self.live_links_in(view, ctx, links, roots, require_current=require_all)
        return bool(kept) and (
            not require_all or len(kept) == sum(link.polarity == "support" for link in links)
        )

    async def support_snapshot_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        excluded_objects: frozenset[str] = frozenset(),
        excluded_sources: frozenset[str] = frozenset(),
    ) -> tuple[set[str], dict[str, CognitiveRecord], set[str]]:
        sources = {
            str(row.id)
            for row in (
                await view.connection.execute(
                    select(observations.c.id, observations.c.record).where(
                        observations.c.namespace_id == ctx.namespace_id
                    )
                )
            ).all()
            if not row.record.get("purged") and str(row.id) not in excluded_sources
        }
        values = (
            await view.connection.execute(
                select(revisions.c.record).where(revisions.c.namespace_id == ctx.namespace_id)
            )
        ).scalars()
        records = {
            record.reference.exact(): record
            for record in (CognitiveRecord.model_validate(value) for value in values)
        }
        invalid_media: set[str] = set()
        for key, record in records.items():
            if isinstance(record.body, MemoryBody):
                encounter = record.body.contact_observation_id
                if isinstance(encounter, str) and encounter not in sources:
                    invalid_media.add(key)
            for reference in media_references_in(record.body):
                availability = await self.derivations.availability_in(
                    view,
                    ctx,
                    reference,
                    purged_observations=excluded_sources,
                    purged_cognition=excluded_objects,
                )
                if availability in {"purged", "missing"}:
                    invalid_media.add(key)
                    break
        supported: set[str] = set()
        dependants: dict[str, set[str]] = {}
        for key, record in records.items():
            if record.purged or record.reference.id in excluded_objects or key in invalid_media:
                continue
            links = tuple(evidence_in(record))
            if tuple(media_references_in(record.body)) and set(record.support_roots).intersection(
                sources
            ):
                supported.add(key)
            for link in links:
                if link.polarity != "support":
                    continue
                if link.observation_id in sources:
                    supported.add(key)
                if link.reference:
                    dependants.setdefault(link.reference.exact(), set()).add(key)
        pending = deque(supported)
        while pending:
            for key in dependants.get(pending.popleft(), ()):
                if key not in supported and key not in invalid_media:
                    supported.add(key)
                    pending.append(key)
        return (sources, records, supported)

    async def revise_evidence_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        reference: CognitiveRef,
        *,
        add: tuple[EvidenceLink, ...] = (),
        remove: tuple[EvidenceLink, ...] = (),
        reason: str,
    ) -> CognitiveRecord:
        previous = await self.get_in(tx, ctx, reference)
        if previous.body is None:
            raise DomainError("cognition_purged", "A purged object cannot change evidence")
        return await self._persist_evidence_in(
            tx, ctx, previous, add=add, remove=remove, reason=reason
        )

    async def _persist_evidence_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        previous: CognitiveRecord,
        *,
        add: tuple[EvidenceLink, ...],
        remove: tuple[EvidenceLink, ...],
        reason: str,
    ) -> CognitiveRecord:
        body = previous.body
        assert body is not None
        links = tuple(
            dict.fromkeys((*(link for link in previous.evidence if link not in remove), *add))
        )
        if body == previous.body and links == previous.evidence:
            return previous
        return await self.commit_in(
            tx,
            ctx,
            AdoptCognition(
                id=previous.reference.id,
                expected_revision=previous.reference.revision,
                body=body,
                evidence=(
                    *links,
                    EvidenceLink(
                        reference=previous.reference, polarity="boundary", role="evidence history"
                    ),
                ),
                times=previous.times,
                producer_call_id=previous.producer_call_id,
            ),
            state=RevisionState(
                deleted=previous.deleted,
                lifecycle=previous.lifecycle,
                cause=RevisionCause(
                    action="evidence.change",
                    actor=ctx.caller_id,
                    reason=reason,
                    parent=previous.reference,
                ),
            ),
        )
