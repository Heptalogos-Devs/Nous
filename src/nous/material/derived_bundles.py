"""Portable representation graphs retain exact identity and physical dependency closure."""

from uuid import uuid4

from nous_wave.contracts import CallContext, DomainError, MediaReference
from nous_wave.contracts.derivation import (
    ArtifactRepresentation,
    DerivedBundle,
    DerivedRegion,
    DerivedRepresentation,
    RepresentationRef,
)
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx
from nous.references import allocate_in

from .coverage import merge_source_regions
from .derived_access import merge_access, representation_ref
from .derived_availability import DerivedAvailability
from .derived_schema import derived_regions, representations


class DerivedBundles(DerivedAvailability):
    async def export_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        references: tuple[MediaReference, ...] = (),
        *,
        observation_ids: tuple[str, ...] = (),
    ) -> DerivedBundle:
        selected: dict[str, DerivedRepresentation] = {}
        selected_regions: dict[str, DerivedRegion] = {}

        async def visit(reference: MediaReference) -> None:
            if reference.kind == "derived_region":
                region = await self.read_region_in(view, ctx, reference.id)
                selected_regions[region.id] = region
                # A portable complete graph includes its container. A subregion grant cannot export it.
                await visit(region.representation.media())
                if region.parent_id and region.parent_id not in selected_regions:
                    await visit(MediaReference(kind="derived_region", id=region.parent_id))
                return
            if reference.kind != "representation" or reference.exact() in selected:
                return
            value = await self.read_in(view, ctx, representation_ref(reference))
            selected[reference.exact()] = value
            for item in value.inputs:
                await visit(item.source)
            if value.previous:
                try:
                    await visit(value.previous.media())
                except DomainError as error:
                    if error.code != "source_unavailable":
                        raise
                    # Erased history carries no payload and does not travel as source content.

        for reference in references:
            await visit(reference)
        roots = set(observation_ids)
        if roots:
            for value in (
                await view.connection.execute(
                    select(representations.c.record).where(
                        representations.c.namespace_id == ctx.namespace_id,
                        representations.c.subject_id == ctx.require_subject(),
                    )
                )
            ).scalars():
                record = DerivedRepresentation.model_validate(value)
                if not record.purged and set(record.source_roots).issubset(roots):
                    await visit(record.reference.media())
            for value in (
                await view.connection.execute(
                    select(derived_regions.c.record).where(
                        derived_regions.c.namespace_id == ctx.namespace_id,
                        derived_regions.c.subject_id == ctx.require_subject(),
                    )
                )
            ).scalars():
                region = DerivedRegion.model_validate(value)
                if not region.purged and region.representation.media().exact() in selected:
                    await visit(MediaReference(kind="derived_region", id=region.id))
        return DerivedBundle(
            representations=tuple(
                sorted(
                    selected.values(),
                    key=lambda item: (item.commit_seq, item.reference.id, item.reference.revision),
                )
            ),
            regions=tuple(selected_regions.values()),
        )

    async def install_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        bundle: DerivedBundle,
        mapping: dict[str, str],
    ) -> tuple[MediaReference, ...]:
        """Install archived provenance after its materials and observations, in the same transaction."""
        representation_ids = {value.reference.id for value in bundle.representations}
        region_ids = {value.id for value in bundle.regions}
        for original_id in representation_ids | region_ids:
            mapping.setdefault(original_id, str(uuid4()))

        def identity(value: str) -> str:
            if value not in mapping:
                raise DomainError(
                    "bundle_reference_missing",
                    "Derived dependency is outside the bundle",
                    reference=value,
                )
            return mapping[value]

        def reference(value: MediaReference) -> MediaReference:
            original = representation_ref(value) if value.kind == "representation" else None
            target = (
                f"{identity(original.id)}@{original.revision}" if original else identity(value.id)
            )
            return value.model_copy(
                update={
                    "id": target,
                    "observation_id": identity(value.observation_id)
                    if value.observation_id
                    else None,
                }
            )

        heads: dict[str, int] = {}
        for value in bundle.representations:
            heads[value.reference.id] = max(
                heads.get(value.reference.id, 0), value.reference.revision
            )
        pending = list(bundle.representations)
        installed: list[MediaReference] = []
        exacts = {value.reference.media().exact() for value in pending}
        all_regions = {value.id: value for value in bundle.regions}
        region_pending = list(bundle.regions)
        installed_regions: set[str] = set()
        while pending or region_pending:
            progress = False
            for region in list(region_pending):
                if reference(region.representation.media()).exact() not in {
                    r.exact() for r in installed
                }:
                    continue
                if region.parent_id and region.parent_id not in installed_regions:
                    if region.parent_id not in all_regions:
                        raise DomainError(
                            "bundle_reference_missing", "Derived region parent is absent"
                        )
                    continue
                updated = region.model_copy(
                    update={
                        "id": identity(region.id),
                        "namespace_id": ctx.namespace_id,
                        "subject_id": ctx.require_subject(),
                        "addresses": await allocate_in(
                            tx, ctx, reference(MediaReference(kind="derived_region", id=region.id))
                        ),
                        "representation": RepresentationRef(
                            id=identity(region.representation.id),
                            revision=region.representation.revision,
                        ),
                        "parent_id": identity(region.parent_id) if region.parent_id else None,
                        "recorded_at": tx.recorded_at,
                        "commit_seq": tx.commit_seq,
                    }
                )
                parent = await self.raw_in(tx, ctx, updated.representation)
                from .derived_regions import select_payload

                if updated.purged or updated.selector is None:
                    raise DomainError(
                        "bundle_source_missing", "Erased regions cannot supply portable content"
                    )
                select_payload(parent, updated.selector)
                await tx.connection.execute(
                    derived_regions.insert().values(
                        id=updated.id,
                        namespace_id=ctx.namespace_id,
                        subject_id=ctx.require_subject(),
                        record=updated.model_dump(mode="json"),
                        commit_seq=tx.commit_seq,
                    )
                )
                installed_regions.add(region.id)
                region_pending.remove(region)
                progress = True
            for value in list(pending):
                dependencies = [
                    item.source
                    for item in value.inputs
                    if item.source.kind in {"representation", "derived_region"}
                ]
                if any(
                    dep.kind == "derived_region" and dep.id not in installed_regions
                    for dep in dependencies
                ):
                    continue
                if any(
                    dep.kind == "representation" and dep.exact() in exacts for dep in dependencies
                ):
                    continue
                inputs = tuple(
                    item.model_copy(update={"source": reference(item.source)})
                    for item in value.inputs
                )
                infos = [await self.source_in(tx, ctx, item.source) for item in inputs]
                roots = tuple(sorted({root for info in infos for root in info.roots}))
                mapped_roots = tuple(sorted(identity(root) for root in value.source_roots))
                if roots != mapped_roots:
                    raise DomainError(
                        "bundle_source_conflict",
                        "Derived roots differ from their actual archived inputs",
                    )
                payload = value.payload
                if payload is None or value.purged:
                    raise DomainError(
                        "bundle_source_missing",
                        "Erased representations cannot supply portable content",
                    )
                if isinstance(payload, ArtifactRepresentation):
                    payload = payload.model_copy(
                        update={"material_id": identity(payload.material_id)}
                    )
                    from .service import materials

                    raw = await tx.connection.scalar(
                        select(materials.c.record).where(
                            materials.c.id == payload.material_id,
                            materials.c.namespace_id == ctx.namespace_id,
                        )
                    )
                    if raw is None or (raw["content_hash"], raw["byte_len"], raw["media_type"]) != (
                        payload.content_hash,
                        payload.byte_len,
                        payload.media_type,
                    ):
                        raise DomainError(
                            "bundle_content_hash",
                            "Derived artifact differs from its archived material",
                        )
                material_ids = {mid for info in infos for mid in info.material_ids}
                if isinstance(payload, ArtifactRepresentation):
                    material_ids.add(payload.material_id)
                if material_ids != {identity(mid) for mid in value.material_ids}:
                    raise DomainError(
                        "bundle_source_conflict",
                        "Derived material closure differs from its archived inputs",
                    )
                access = merge_access(tuple(info.access for info in infos))
                if access != value.access:
                    raise DomainError(
                        "bundle_access_conflict",
                        "Derived access channels differ from its actual inputs",
                    )
                source_regions = merge_source_regions(
                    (root, info.regions.get(root, ())) for info in infos for root in info.roots
                )
                target = reference(value.reference.media())
                updated = value.model_copy(
                    update={
                        "reference": representation_ref(target),
                        "addresses": await allocate_in(tx, ctx, target),
                        "namespace_id": ctx.namespace_id,
                        "subject_id": ctx.require_subject(),
                        "inputs": inputs,
                        "payload": payload,
                        "producer": value.producer.model_copy(
                            update={"model_call_id": identity(value.producer.model_call_id)}
                        )
                        if value.producer.model_call_id
                        else value.producer,
                        "source_roots": roots,
                        "material_ids": tuple(sorted(material_ids)),
                        "source_regions": source_regions,
                        "access": access,
                        "timeline": reference(value.timeline) if value.timeline else None,
                        "previous": RepresentationRef(
                            id=identity(value.previous.id), revision=value.previous.revision
                        )
                        if value.previous
                        and value.previous.media().exact()
                        in {v.reference.media().exact() for v in bundle.representations}
                        else None,
                        "commit_seq": tx.commit_seq,
                        "recorded_at": tx.recorded_at,
                    }
                )
                await tx.connection.execute(
                    representations.insert().values(
                        id=updated.reference.id,
                        revision=updated.reference.revision,
                        namespace_id=ctx.namespace_id,
                        subject_id=ctx.require_subject(),
                        head=heads[value.reference.id] == value.reference.revision,
                        record=updated.model_dump(mode="json"),
                        commit_seq=tx.commit_seq,
                    )
                )
                installed.append(target)
                exacts.remove(value.reference.media().exact())
                pending.remove(value)
                progress = True
            if not progress:
                raise DomainError(
                    "bundle_derivation_cycle", "Derived graph has missing inputs or a cycle"
                )
        return tuple(installed)
