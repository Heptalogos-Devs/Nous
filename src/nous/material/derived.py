"""Adopt exact derivations in the same authority as their actual inputs."""

from __future__ import annotations

from typing import TYPE_CHECKING, TypeVar
from uuid import uuid4

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.derivation import (
    ArtifactRepresentation,
    CreateRepresentation,
    DerivationInput,
    DerivationStrategy,
    DerivedRepresentation,
    RepresentationRef,
    StructuredRepresentation,
)
from nous_sdk.contracts.media import MediaReference
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import func, select

from nous.identity.access import require_namespace_in
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.references import allocate_in

from .coverage import merge_source_regions
from .derived_access import SourceInfo, merge_access, representation_ref
from .derived_bundles import DerivedBundles
from .derived_erasure import purge_materials_in
from .derived_producer import verify_result
from .derived_schema import representations
from .service import MaterialService

if TYPE_CHECKING:
    from nous.platform.models.generation import ModelResult

T = TypeVar("T")


class DerivedService(DerivedBundles):
    def __init__(self, authority: Authority, materials: MaterialService) -> None:
        self.authority, self.materials = authority, materials

    async def for_material_in(
        self, view: ReadView | WriteTx, ctx: CallContext, material_id: str
    ) -> tuple[DerivedRepresentation, ...]:
        await self.materials.read_in(view, ctx, material_id)
        ids = func.json_each(representations.c.record, "$.material_ids").table_valued("value")
        rows = (
            await view.connection.execute(
                select(representations.c.record)
                .where(
                    representations.c.namespace_id == ctx.namespace_id,
                    representations.c.head.is_(True),
                    select(ids.c.value).where(ids.c.value == material_id).exists(),
                )
                .order_by(representations.c.commit_seq, representations.c.id)
            )
        ).scalars()
        result = []
        for raw in rows:
            value = DerivedRepresentation.model_validate(raw)
            result.append(await self.read_in(view, ctx, value.reference))
        return tuple(result)

    async def for_material(
        self, ctx: CallContext, material_id: str
    ) -> tuple[DerivedRepresentation, ...]:
        async with self.authority.read(ctx) as view:
            return await self.for_material_in(view, ctx, material_id)

    async def adopt_in(
        self, tx: WriteTx, ctx: CallContext, command: CreateRepresentation
    ) -> DerivedRepresentation:
        await require_namespace_in(tx, ctx)
        object_id = command.id or str(uuid4())
        head = await tx.connection.scalar(
            select(representations.c.record).where(
                representations.c.namespace_id == ctx.namespace_id,
                representations.c.id == object_id,
                representations.c.head.is_(True),
            )
        )
        previous = DerivedRepresentation.model_validate(head) if head is not None else None
        if (previous.reference.revision if previous else 0) != command.expected_revision:
            raise DomainError(
                "representation_revision_conflict",
                "Representation was changed since its expected revision",
            )
        if previous is not None:
            await self.read_in(tx, ctx, previous.reference)
            if previous.kind != command.kind:
                raise DomainError(
                    "representation_kind_conflict", "A representation identity retains its kind"
                )
        infos: list[SourceInfo] = []
        for item in command.inputs:
            if (
                item.source.kind == "representation"
                and representation_ref(item.source).id == object_id
            ):
                raise DomainError("derivation_cycle", "A representation cannot be its own input")
            if item.role in {
                "description",
                "transcript",
                "extracted_text",
            } and item.source.kind not in {"representation", "derived_region"}:
                raise DomainError(
                    "derivation_role_invalid", "Textual inputs require committed derived identities"
                )
            infos.append(await self.source_in(tx, ctx, item.source))
        inputs = tuple(
            item.model_copy(update={"source": info.reference})
            for item, info in zip(command.inputs, infos, strict=True)
        )
        access = merge_access(tuple(info.access for info in infos))
        original = any(
            value == "original" for value in (access.visual, access.audio, access.source_text)
        )
        if command.kind == "structured_interpretation":
            if not original:
                raise DomainError(
                    "derivation_access_invalid",
                    "Structured media interpretation requires actual original input",
                )
            if command.strategy == "describe_then_structure" and not any(
                item.role in {"description", "transcript"} for item in inputs
            ):
                raise DomainError(
                    "derivation_description_missing",
                    "Second-stage interpretation requires its committed description",
                )
        if command.timeline and command.timeline.exact() not in {
            item.source.exact() for item in inputs
        }:
            raise DomainError(
                "derivation_timeline_invalid", "Timeline must identify one exact supplied input"
            )
        material_ids = tuple(sorted({identity for info in infos for identity in info.material_ids}))
        if isinstance(command.payload, ArtifactRepresentation):
            artifact = await self.materials.read_in(tx, ctx, command.payload.material_id)
            if (artifact["content_hash"], artifact["byte_len"], artifact["media_type"]) != (
                command.payload.content_hash,
                command.payload.byte_len,
                command.payload.media_type,
            ):
                raise DomainError(
                    "derivation_artifact_conflict", "Artifact differs from its retained bytes"
                )
            required = set(artifact.get("required_source_ids", []))
            if not all(
                item.source.exact() in required or item.source.id in required for item in inputs
            ):
                raise DomainError(
                    "derivation_artifact_scope", "Artifact must retain all exact input dependencies"
                )
            material_ids = tuple(sorted({*material_ids, command.payload.material_id}))
        if command.producer is not None:
            expected_task = (
                {
                    "nous.memory-notes/1": "formation.notes",
                    "nous.graph-extraction/1": "formation.graph",
                    "nous.graph-resolution/1": "formation.entity-resolution",
                }.get(command.payload.schema_id)
                if isinstance(command.payload, StructuredRepresentation)
                else "media.describe"
                if command.kind
                in {
                    "image_description",
                    "audio_description",
                    "scene_description",
                    "resource_synopsis",
                }
                else None
            )
            verify_result(command.producer, command.payload, expected_task=expected_task)
        elif (
            isinstance(command.payload, StructuredRepresentation)
            and command.kind != "parsed_record"
        ):
            raise DomainError(
                "producer_required", "Structured interpretation requires model provenance"
            )
        reference = RepresentationRef(id=object_id, revision=command.expected_revision + 1)
        source_regions = merge_source_regions(
            (root, info.regions.get(root, ())) for info in infos for root in info.roots
        )
        value = DerivedRepresentation(
            reference=reference,
            addresses=await allocate_in(tx, ctx, reference.media()),
            namespace_id=ctx.namespace_id,
            inputs=inputs,
            kind=command.kind,
            strategy=command.strategy,
            payload=command.payload,
            producer=command.producer,
            processor=command.processor,
            access=access,
            quality=command.quality,
            source_roots=tuple(sorted({root for info in infos for root in info.roots})),
            material_ids=material_ids,
            source_regions=source_regions,
            timeline=command.timeline,
            transformation=command.transformation,
            formed_at=ctx.clock,
            recorded_at=tx.recorded_at,
            commit_seq=tx.commit_seq,
            previous=previous.reference if previous else None,
        )
        await tx.connection.execute(
            representations.update()
            .where(
                representations.c.id == object_id,
                representations.c.namespace_id == ctx.namespace_id,
            )
            .values(head=False)
        )
        await tx.connection.execute(
            representations.insert().values(
                id=reference.id,
                revision=reference.revision,
                namespace_id=ctx.namespace_id,
                head=True,
                record=value.model_dump(mode="json"),
                commit_seq=tx.commit_seq,
            )
        )
        from nous.retrieval.maintenance import enqueue_index_in

        await enqueue_index_in(tx, ctx)
        return value

    async def adopt_model_result_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        result: ModelResult[T],
        *,
        inputs: tuple[DerivationInput, ...],
        schema_id: str,
        strategy: DerivationStrategy = "direct_structured",
        timeline: MediaReference | None = None,
    ) -> DerivedRepresentation:
        value = TypeAdapter(dict[str, JsonValue]).validate_python(
            TypeAdapter(object).dump_python(result.output, mode="json")
        )
        return await self.adopt_in(
            tx,
            ctx,
            CreateRepresentation(
                inputs=inputs,
                kind="structured_interpretation",
                strategy=strategy,
                payload=StructuredRepresentation(schema_id=schema_id, value=value),
                producer=result.producer,
                timeline=timeline,
            ),
        )

    async def adopt(self, ctx: CallContext, command: CreateRepresentation) -> DerivedRepresentation:
        async with self.authority.write(ctx) as tx:
            value = await self.adopt_in(tx, ctx, command)
        return value

    async def history(
        self, ctx: CallContext, representation_id: str
    ) -> tuple[DerivedRepresentation, ...]:
        async with self.authority.read(ctx) as view:
            references = (
                await view.connection.execute(
                    select(representations.c.revision)
                    .where(
                        representations.c.id == representation_id,
                        representations.c.namespace_id == ctx.namespace_id,
                    )
                    .order_by(representations.c.revision)
                )
            ).scalars()
            result: list[DerivedRepresentation] = []
            for revision in references:
                try:
                    result.append(
                        await self.read_in(
                            view,
                            ctx,
                            RepresentationRef(id=representation_id, revision=int(revision)),
                        )
                    )
                except DomainError as error:
                    if error.code != "source_unavailable":
                        raise
            return tuple(result)

    purge_materials_in = purge_materials_in
