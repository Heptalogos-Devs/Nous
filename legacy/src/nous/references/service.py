"""A directory lookup resolves identity, then delegates all visibility to its owner."""

from typing import TYPE_CHECKING

import sqlalchemy as sa
from nous_sdk.contracts import (
    CognitiveRecord,
    CognitiveRef,
    MediaReference,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.context import CognitiveTarget, MediaTarget
from nous_sdk.contracts.query_context import QueryContext
from nous_sdk.contracts.references import (
    EvidenceTarget,
    ObjectTarget,
    ObservationTarget,
    ReferenceResolution,
    ResolvedTarget,
    ResourceTarget,
    SemanticTarget,
)
from nous_sdk.contracts.source_objects import EvidenceReference, ObservationReference
from pydantic import JsonValue, TypeAdapter

from nous.cognition.records import CognitiveRecords
from nous.cognition.schema import revisions
from nous.material.service import MaterialService, materials, regions
from nous.platform.persistence import Authority, ReadView, WriteTx

from . import source_objects
from .allocation import addresses_in, recover_in, validate_address
from .schema import addresses, imported, query_groups

if TYPE_CHECKING:
    from nous.data_management.resources import ResourceService

_TARGET = TypeAdapter(SemanticTarget)


async def _head_in(
    view: ReadView | WriteTx,
    statement: sa.Select[tuple[JsonValue]],
    as_of_time: int | None,
) -> JsonValue:
    if as_of_time is None:
        return await view.connection.scalar(
            statement.order_by(revisions.c.revision.desc()).limit(1)
        )
    values = (
        await view.connection.execute(statement.order_by(revisions.c.revision.desc()))
    ).scalars()
    for value in values:
        record = CognitiveRecord.model_validate(value)
        if (
            record.times.recorded.domain == "unix_ms"
            and record.times.recorded.coordinate <= as_of_time
        ):
            return value
    return None


class ReferenceDirectory:
    observation_records_in = source_objects.observation_records_in
    evidence_records_in = source_objects.evidence_records_in
    read_source_object_in = source_objects.read_source_object_in
    require_source_object_in = source_objects.require_source_object_in
    material_ids_in = source_objects.material_ids_in

    async def query_context_in(self, view: ReadView | WriteTx, ctx: CallContext, locator: str):

        from nous.context.lifecycle import session_record_in, work_record_in

        goal = ""
        unresolved: tuple[str, ...] = ()
        targets: list[SemanticTarget] = []
        work_id = None
        if locator.startswith("session:"):
            session = await session_record_in(view, ctx, locator.removeprefix("session:"))
            if session.closed:
                raise DomainError("query_context_inactive", "The query session is closed")
            targets.extend(item.target for item in session.residents if item.state != "evicted")
            work_id = session.active_work_context_id
        elif locator.startswith("work:"):
            work_id = locator.removeprefix("work:")
        else:
            targets.append((await self.resolve_in(view, ctx, locator)).reference)
        if work_id is not None:
            work = await work_record_in(view, ctx, work_id)
            if work.purged or work.state != "open":
                raise DomainError("query_context_inactive", "The query work context is inactive")
            goal = work.content.purpose
            unresolved = work.content.unresolved_questions
            targets.extend(work.content.anchors)
        anchors: list[str] = []
        entities: list[str] = []
        attachments: list[MediaReference] = []
        for target in targets:
            resolved = await self.resolve_target_in(view, ctx, target, None)
            if isinstance(resolved, MediaTarget):
                attachments.append(resolved.reference)
            else:
                anchors.append(resolved.reference.exact())
                if isinstance(resolved, CognitiveTarget) and resolved.reference.kind == "entity":
                    entities.append(resolved.reference.exact())
        return QueryContext(
            goal=goal,
            anchors=tuple(dict.fromkeys(anchors)),
            entities=tuple(dict.fromkeys(entities)),
            attachments=tuple(dict.fromkeys(attachments)),
            unresolved=unresolved,
        )

    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        materials: MaterialService,
        *,
        resources: ResourceService | None = None,
    ) -> None:
        self.authority, self.cognition, self.materials = authority, cognition, materials
        self.derivations = cognition.derivations
        self.resources = resources

    async def resolve_target_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        target: SemanticTarget,
        as_of_commit: int | None,
        as_of_time: int | None = None,
    ) -> ResolvedTarget:
        if isinstance(target, ObjectTarget):
            statement = sa.select(revisions.c.record).where(
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.id == target.identity.id,
                revisions.c.kind == target.identity.kind,
            )
            if as_of_commit is not None:
                statement = statement.where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.commit_seq <= as_of_commit)
            value = await _head_in(view, statement, as_of_time)
            if value is None:
                raise DomainError("reference_not_found", "Reference has no visible revision")
            target = CognitiveTarget(reference=CognitiveRecord.model_validate(value).reference)
        if isinstance(target, CognitiveTarget):
            record = await self.cognition.get_in(view, ctx, target.reference)
            if as_of_commit is not None and record.commit_seq > as_of_commit:
                raise DomainError(
                    "reference_not_found", "Reference is outside the requested history"
                )
            if as_of_time is not None and (
                record.times.recorded.domain != "unix_ms"
                or record.times.recorded.coordinate > as_of_time
            ):
                raise DomainError(
                    "reference_not_found", "Reference is outside recorded-time history"
                )
        elif isinstance(target, ResourceTarget):
            if self.resources is None:
                raise DomainError(
                    "resource_directory_unavailable", "External resource directory is unavailable"
                )
            record = await self.resources.get_in(
                view, ctx, target.reference.id, target.reference.revision
            )
            if as_of_commit is not None and record.commit_seq > as_of_commit:
                raise DomainError(
                    "reference_not_found", "Resource is outside the requested history"
                )
            if as_of_time is not None and record.times.recorded.coordinate > as_of_time:
                raise DomainError(
                    "reference_not_found", "Resource is outside recorded-time history"
                )
        elif isinstance(target, (ObservationTarget, EvidenceTarget)):
            source = await self.read_source_object_in(view, ctx, target.reference)
            if as_of_commit is not None and source.commit_seq > as_of_commit:
                raise DomainError(
                    "reference_not_found", "Source object is outside the requested history"
                )
            if as_of_time is not None and source.times.recorded.coordinate > as_of_time:
                raise DomainError(
                    "reference_not_found", "Source object is outside recorded-time history"
                )
        elif target.reference.kind in {"material", "source_region"}:
            await self.materials.media_source_in(view, ctx, target.reference)
        else:
            await self.derivations.authorize_in(view, ctx, target.reference)
        return target

    async def resolve_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        locator: str,
        as_of_commit: int | None = None,
        as_of_time: int | None = None,
    ) -> ReferenceResolution:
        if as_of_commit is not None and (as_of_commit < 0 or as_of_commit > view.commit_seq):
            raise DomainError(
                "invalid_history", "Reference history cutoff is outside committed history"
            )
        group_ref, separator, item = locator.partition(":")
        if separator and group_ref.startswith("q") and group_ref[1:].isdigit() and item.isdigit():
            targets = await view.connection.scalar(
                sa.select(query_groups.c.targets).where(
                    query_groups.c.namespace_id == ctx.namespace_id,
                    query_groups.c.group_ref == group_ref,
                    query_groups.c.caller_id == ctx.caller_id,
                )
            )
            position = int(item) - 1
            if targets is None or position < 0 or position >= len(targets):
                raise DomainError(
                    "reference_not_found", "Query result reference does not exist in this Namespace"
                )
            target = _TARGET.validate_python(targets[position])
            resolved = await self.resolve_target_in(view, ctx, target, as_of_commit, as_of_time)
            return ReferenceResolution(namespace_id=ctx.namespace_id, reference=resolved)
        if ":" in locator:
            kind, identity = locator.split(":", 1)
            if kind == "resource":
                if self.resources is None:
                    raise DomainError(
                        "resource_directory_unavailable",
                        "External resource directory is unavailable",
                    )
                record = await self.resources.resolve_in(view, ctx, locator, as_of_commit)
                target = ResourceTarget(reference=record.reference)
                await self.resolve_target_in(view, ctx, target, as_of_commit, as_of_time)
                return ReferenceResolution(
                    namespace_id=ctx.namespace_id,
                    reference=target,
                )
            if kind in {"observation", "evidence"}:
                if kind == "observation":
                    object_id, mark, version = identity.rpartition("@")
                    if mark and version != "1":
                        raise DomainError(
                            "invalid_reference", "Observation references have one immutable version"
                        )
                    target = ObservationTarget(
                        reference=ObservationReference(id=object_id if mark else identity)
                    )
                else:
                    target = EvidenceTarget(reference=EvidenceReference(id=identity))
                await self.resolve_target_in(view, ctx, target, as_of_commit, as_of_time)
                return ReferenceResolution(
                    namespace_id=ctx.namespace_id,
                    reference=target,
                )
            if kind in {
                "material",
                "source-region",
                "source_region",
                "representation",
                "derived-region",
                "derived_region",
            }:
                reference = MediaReference.model_validate(
                    {"kind": kind.replace("-", "_"), "id": identity}
                )
                resolved = await self.resolve_target_in(
                    view, ctx, MediaTarget(reference=reference), as_of_commit, as_of_time
                )
                return ReferenceResolution(
                    namespace_id=ctx.namespace_id,
                    reference=resolved,
                    addresses=await addresses_in(view, ctx, reference),
                )
            if "@" in identity:
                object_id, revision = identity.rsplit("@", 1)
                cognitive = CognitiveRef.model_validate(
                    {"kind": kind, "id": object_id, "revision": int(revision)}
                )
                resolved = await self.resolve_target_in(
                    view, ctx, CognitiveTarget(reference=cognitive), as_of_commit, as_of_time
                )
                return ReferenceResolution(
                    namespace_id=ctx.namespace_id,
                    reference=resolved,
                    addresses=await addresses_in(view, ctx, cognitive),
                )
        object_kind, separator, object_id = locator.partition(":")
        if not separator:
            object_id = locator
        statement = sa.select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.id == object_id,
        )
        if separator:
            statement = statement.where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.kind == object_kind)
        if as_of_commit is not None:
            statement = statement.where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.commit_seq <= as_of_commit)
        value = await _head_in(view, statement, as_of_time)
        if value is not None:
            cognitive = CognitiveRecord.model_validate(value).reference
            resolved = await self.resolve_target_in(
                view, ctx, CognitiveTarget(reference=cognitive), as_of_commit, as_of_time
            )
            return ReferenceResolution(
                namespace_id=ctx.namespace_id,
                reference=resolved,
                addresses=await addresses_in(view, ctx, cognitive),
            )
        if not separator:
            raise DomainError("reference_not_found", "Canonical reference is unavailable")
        validate_address(locator)
        local = sa.select(addresses.c.target).where(
            addresses.c.namespace_id == ctx.namespace_id, addresses.c.address == locator
        )
        inherited = sa.select(imported.c.target).where(
            imported.c.namespace_id == ctx.namespace_id,
            imported.c.address == locator,
        )
        if as_of_commit is not None:
            inherited = inherited.where(imported.c.commit_seq <= as_of_commit)
        encoded = set((await view.connection.execute(local)).scalars()) | set(
            (await view.connection.execute(inherited)).scalars()
        )
        matches: dict[str, ResolvedTarget] = {}
        for value in encoded:
            try:
                target = await self.resolve_target_in(
                    view, ctx, _TARGET.validate_json(str(value)), as_of_commit, as_of_time
                )
                matches[target.reference.exact()] = target
            except DomainError as error:
                if error.code not in {
                    "reference_not_found",
                    "cognition_not_found",
                    "source_unavailable",
                    "region_not_found",
                }:
                    raise
        if not matches:
            raise DomainError(
                "reference_not_found", "Readable reference is unavailable in this scope"
            )
        if len(matches) != 1:
            raise DomainError(
                "reference_ambiguous",
                "Readable reference has multiple visible bindings",
                candidates=TypeAdapter(list[JsonValue]).validate_python(sorted(matches)),
            )
        resolved = next(iter(matches.values()))
        return ReferenceResolution(
            namespace_id=ctx.namespace_id,
            reference=resolved,
            addresses=await addresses_in(view, ctx, resolved.reference)
            if isinstance(resolved, (CognitiveTarget, MediaTarget))
            else None,
        )

    async def resolve(
        self,
        ctx: CallContext,
        locator: str,
        as_of_commit: int | None = None,
        as_of_time: int | None = None,
    ) -> ReferenceResolution:
        async with self.authority.read(ctx) as view:
            return await self.resolve_in(view, ctx, locator, as_of_commit, as_of_time)

    async def retain_query(self, ctx: CallContext, result):

        from nous_sdk.contracts.resources import ResourceRecord
        from nous_sdk.contracts.retrieval import QueryResult
        from nous_sdk.contracts.source_objects import ObservationRecord

        targets = []
        for hit in result.hits:
            record = hit.object
            if isinstance(record, CognitiveRecord):
                target = CognitiveTarget(reference=record.reference)
            elif isinstance(record, ResourceRecord):
                target = ResourceTarget(reference=record.reference)
            elif isinstance(record, ObservationRecord):
                target = ObservationTarget(reference=record.reference)
            elif isinstance(record.reference, MediaReference):
                target = MediaTarget(reference=record.reference)
            else:
                target = EvidenceTarget(reference=record.reference)
            targets.append(target.model_dump(mode="json"))
        async with self.authority.write(ctx) as tx:
            group_ref = "q" + str(tx.commit_seq)
            await tx.connection.execute(
                query_groups.insert().values(
                    namespace_id=ctx.namespace_id,
                    group_ref=group_ref,
                    caller_id=ctx.caller_id,
                    targets=targets,
                    commit_seq=tx.commit_seq,
                )
            )
            refs = tuple(group_ref + ":" + str(index + 1) for index in range(len(targets)))
        return QueryResult.model_validate({**result.model_dump(), "result_refs": refs})

    async def reconcile(self, ctx: CallContext, operation_id: str) -> dict[str, int]:
        """Repair missing address metadata on visible identities without revising content."""
        async with self.authority.write(ctx) as tx:
            scopes = [ctx]
            statement = sa.select(revisions).where(revisions.c.namespace_id == ctx.namespace_id)
            if ctx.namespace_id is not None:
                statement = statement.where(revisions.c.namespace_id == ctx.namespace_id)
            counts = {"cognition": 0, "materials": 0, "regions": 0, "commit_seq": tx.commit_seq}
            for row in (await tx.connection.execute(statement)).mappings():
                selected = ctx.model_copy(update={})
                record = CognitiveRecord.model_validate(row["record"])
                try:
                    await self.cognition.get_in(tx, selected, record.reference)
                except DomainError as error:
                    if error.code != "source_unavailable":
                        raise
                    continue
                prior = await addresses_in(tx, selected, record.reference)
                allocated = await recover_in(tx, selected, record.reference, record.addresses)
                if prior is None or record.addresses != allocated:
                    updated = dict(row["record"])
                    updated["addresses"] = allocated.model_dump(mode="json")
                    await tx.connection.execute(
                        revisions.update().where(revisions.c.namespace_id == ctx.namespace_id, revisions.c.id == record.reference.id, revisions.c.revision == record.reference.revision)
                        .values(record=updated)
                    )
                    counts["cognition"] += 1
            for table, kind, counter in (
                (materials, "material", "materials"),
                (regions, "source_region", "regions"),
            ):
                rows = (
                    await tx.connection.execute(
                        sa.select(table).where(table.c.namespace_id == ctx.namespace_id)
                    )
                ).mappings()
                for row in rows:
                    for selected in scopes:
                        try:
                            if kind == "material":
                                await self.materials.read_in(tx, selected, str(row["id"]))
                            else:
                                await self.materials.read_region_in(tx, selected, str(row["id"]))
                        except DomainError as error:
                            if error.code != "source_unavailable":
                                raise
                            continue
                        reference = MediaReference.model_validate({"kind": kind, "id": row["id"]})
                        prior = await addresses_in(tx, selected, reference)
                        from nous_sdk.contracts.common import ReferenceAddresses

                        known = row["record"].get("addresses")
                        allocated = await recover_in(
                            tx,
                            selected,
                            reference,
                            ReferenceAddresses.model_validate(known) if known else None,
                        )
                        if prior is None or row["record"].get("addresses") != allocated.model_dump(
                            mode="json"
                        ):
                            updated = dict(row["record"])
                            updated["addresses"] = allocated.model_dump(mode="json")
                            await tx.connection.execute(
                                table.update().where(table.c.id == row["id"]).values(record=updated)
                            )
                            counts[counter] += 1
                        break
            return counts
