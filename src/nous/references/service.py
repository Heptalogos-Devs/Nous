"""A directory lookup resolves identity, then delegates all visibility to its owner."""

import sqlalchemy as sa
from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    MediaReference,
)
from nous_wave.contracts.context import CognitiveTarget, ContextTarget, MediaTarget
from nous_wave.contracts.references import ObjectTarget, ReferenceResolution, SemanticTarget
from pydantic import JsonValue, TypeAdapter

from nous.cognition.schema import revisions
from nous.cognition.service import CognitionService
from nous.identity.service import subjects
from nous.material.service import MaterialService, materials, regions
from nous.platform.persistence import Authority, ReadView, WriteTx

from .allocation import addresses_in, recover_in, validate_address
from .schema import addresses, imported

_TARGET = TypeAdapter(SemanticTarget)


class ReferenceDirectory:
    def __init__(
        self, authority: Authority, cognition: CognitionService, materials: MaterialService
    ) -> None:
        self.authority, self.cognition, self.materials = authority, cognition, materials

    async def _resolve_target(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        target: SemanticTarget,
        as_of_commit: int | None,
    ) -> ContextTarget:
        if isinstance(target, ObjectTarget):
            statement = sa.select(revisions.c.record).where(
                revisions.c.namespace_id == ctx.namespace_id,
                revisions.c.subject_id == ctx.subject_id,
                revisions.c.id == target.identity.id,
                revisions.c.kind == target.identity.kind,
            )
            if as_of_commit is not None:
                statement = statement.where(revisions.c.commit_seq <= as_of_commit)
            value = await view.connection.scalar(
                statement.order_by(revisions.c.revision.desc()).limit(1)
            )
            if value is None:
                raise DomainError("reference_not_found", "Reference has no visible revision")
            target = CognitiveTarget(reference=CognitiveRecord.model_validate(value).reference)
        if isinstance(target, CognitiveTarget):
            record = await self.cognition.get_in(view, ctx, target.reference)
            if as_of_commit is not None and record.commit_seq > as_of_commit:
                raise DomainError(
                    "reference_not_found", "Reference is outside the requested history"
                )
        elif target.reference.kind == "material":
            await self.materials.read_in(view, ctx, target.reference.id)
        elif target.reference.kind == "source_region":
            await self.materials.read_region_in(view, ctx, target.reference.id)
        else:
            from nous.material.derived import DerivedService

            await DerivedService(self.authority, self.materials).authorize_in(
                view, ctx, target.reference
            )
        return target

    async def resolve_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        locator: str,
        as_of_commit: int | None = None,
    ) -> ReferenceResolution:
        if as_of_commit is not None and (as_of_commit < 0 or as_of_commit > view.commit_seq):
            raise DomainError(
                "invalid_history", "Reference history cutoff is outside committed history"
            )
        if ":" in locator:
            kind, identity = locator.split(":", 1)
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
                resolved = await self._resolve_target(
                    view, ctx, MediaTarget(reference=reference), as_of_commit
                )
                return ReferenceResolution(
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.require_subject(),
                    reference=resolved,
                    addresses=await addresses_in(view, ctx, reference),
                )
            if "@" in identity:
                object_id, revision = identity.rsplit("@", 1)
                cognitive = CognitiveRef.model_validate(
                    {"kind": kind, "id": object_id, "revision": int(revision)}
                )
                resolved = await self._resolve_target(
                    view, ctx, CognitiveTarget(reference=cognitive), as_of_commit
                )
                return ReferenceResolution(
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.require_subject(),
                    reference=resolved,
                    addresses=await addresses_in(view, ctx, cognitive),
                )
        object_kind, separator, object_id = locator.partition(":")
        if not separator:
            object_id = locator
        statement = sa.select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.subject_id == ctx.subject_id,
            revisions.c.id == object_id,
        )
        if separator:
            statement = statement.where(revisions.c.kind == object_kind)
        if as_of_commit is not None:
            statement = statement.where(revisions.c.commit_seq <= as_of_commit)
        value = await view.connection.scalar(
            statement.order_by(revisions.c.revision.desc()).limit(1)
        )
        if value is not None:
            cognitive = CognitiveRecord.model_validate(value).reference
            resolved = await self._resolve_target(
                view, ctx, CognitiveTarget(reference=cognitive), as_of_commit
            )
            return ReferenceResolution(
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
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
            imported.c.subject_id == ctx.subject_id,
            imported.c.address == locator,
        )
        if as_of_commit is not None:
            inherited = inherited.where(imported.c.commit_seq <= as_of_commit)
        encoded = set((await view.connection.execute(local)).scalars()) | set(
            (await view.connection.execute(inherited)).scalars()
        )
        matches: dict[str, ContextTarget] = {}
        for value in encoded:
            try:
                target = await self._resolve_target(
                    view, ctx, _TARGET.validate_json(str(value)), as_of_commit
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
            subject_id=ctx.require_subject(),
            reference=resolved,
            addresses=await addresses_in(view, ctx, resolved.reference),
        )

    async def resolve(
        self, ctx: CallContext, locator: str, as_of_commit: int | None = None
    ) -> ReferenceResolution:
        async with self.authority.read(ctx) as view:
            return await self.resolve_in(view, ctx, locator, as_of_commit)

    async def reconcile(self, ctx: CallContext, operation_id: str) -> dict[str, int]:
        """Repair missing address metadata on visible identities without revising content."""
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="references.reconcile",
            scope=ctx.subject_id or ctx.namespace_id,
            payload={"namespace_id": ctx.namespace_id, "subject_id": ctx.subject_id},
        ) as tx:
            if tx.replayed:
                return TypeAdapter(dict[str, int]).validate_python(tx.replay_result)
            subject_ids = (
                [ctx.subject_id]
                if ctx.subject_id is not None
                else list(
                    (
                        await tx.connection.execute(
                            sa.select(subjects.c.id).where(
                                subjects.c.namespace_id == ctx.namespace_id
                            )
                        )
                    ).scalars()
                )
            )
            scopes = [ctx.model_copy(update={"subject_id": str(sid)}) for sid in subject_ids]
            statement = sa.select(revisions).where(revisions.c.namespace_id == ctx.namespace_id)
            if ctx.subject_id is not None:
                statement = statement.where(revisions.c.subject_id == ctx.subject_id)
            counts = {"cognition": 0, "materials": 0, "regions": 0, "commit_seq": tx.commit_seq}
            for row in (await tx.connection.execute(statement)).mappings():
                selected = ctx.model_copy(update={"subject_id": str(row["subject_id"])})
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
                        revisions.update()
                        .where(
                            revisions.c.id == record.reference.id,
                            revisions.c.revision == record.reference.revision,
                        )
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
                        from nous_wave.contracts.common import ReferenceAddresses

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
            return tx.result(counts)
