"""Exact derived selections are checked against their payload and enclosing selection."""

import json
from dataclasses import dataclass
from uuid import uuid4

from jsonpointer import JsonPointerException, resolve_pointer
from nous_wave.contracts import CallContext, DomainError, MediaReference
from nous_wave.contracts.derivation import (
    ArtifactRepresentation,
    CreateDerivedRegion,
    DerivedRegion,
    DerivedRepresentation,
    DerivedSelector,
    RepresentationPayload,
    StructuredRepresentation,
    TextRepresentation,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.platform.persistence import ReadView, WriteTx
from nous.references import allocate_in

from .derived_access import DerivedAccess, representation_ref
from .derived_schema import derived_regions


@dataclass(frozen=True)
class SelectedPayload:
    payload: RepresentationPayload
    selector: DerivedSelector | None = None


def select_payload(value: DerivedRepresentation, selector: DerivedSelector) -> SelectedPayload:
    payload = value.payload
    if isinstance(payload, TextRepresentation) and selector.kind == "text_span":
        assert selector.start is not None and selector.end is not None
        if selector.end > len(payload.text):
            raise DomainError("derived_region_invalid", "Selection exceeds the retained text")
        return SelectedPayload(TextRepresentation(text=payload.text[selector.start : selector.end]))
    if isinstance(payload, StructuredRepresentation) and selector.kind == "json_pointer":
        assert selector.json_pointer is not None
        try:
            selected = TypeAdapter(JsonValue).validate_python(
                resolve_pointer(payload.value, selector.json_pointer)
            )
        except JsonPointerException as error:
            raise DomainError(
                "derived_region_invalid", "JSON pointer does not resolve in the retained structure"
            ) from error
        return SelectedPayload(TextRepresentation(text=json.dumps(selected, ensure_ascii=False)))
    if isinstance(payload, ArtifactRepresentation) and selector.kind == "media":
        info = payload.media_info
        if selector.pixel_rect:
            rect = selector.pixel_rect
            if (
                not info.width
                or not info.height
                or rect.x + rect.width > info.width
                or rect.y + rect.height > info.height
            ):
                raise DomainError(
                    "derived_region_invalid", "Pixel selection exceeds the retained artifact"
                )
        if selector.time_span and (
            info.duration_ms is None or selector.time_span.end_ms > info.duration_ms
        ):
            raise DomainError(
                "derived_region_invalid", "Time selection exceeds the retained artifact"
            )
        if (selector.page_start or selector.page_end) and payload.media_type != "application/pdf":
            raise DomainError("derived_region_invalid", "Page selection requires a PDF artifact")
        return SelectedPayload(payload, selector)
    raise DomainError(
        "derived_region_invalid", "Selector is incompatible with the retained payload"
    )


def contained(parent: DerivedSelector, child: DerivedSelector) -> bool:
    if parent.kind != child.kind:
        return False
    if parent.kind == "text_span":
        assert (
            parent.start is not None
            and parent.end is not None
            and child.start is not None
            and child.end is not None
        )
        return parent.start <= child.start < child.end <= parent.end
    if parent.kind == "json_pointer":
        assert parent.json_pointer is not None and child.json_pointer is not None
        return child.json_pointer == parent.json_pointer or child.json_pointer.startswith(
            parent.json_pointer + "/"
        )
    if parent.pixel_rect:
        if child.pixel_rect is None:
            return False
        outer, inner = parent.pixel_rect, child.pixel_rect
        if not (
            outer.x <= inner.x
            and outer.y <= inner.y
            and inner.x + inner.width <= outer.x + outer.width
            and inner.y + inner.height <= outer.y + outer.height
        ):
            return False
    if parent.time_span and (
        child.time_span is None
        or child.time_span.start_ms < parent.time_span.start_ms
        or child.time_span.end_ms > parent.time_span.end_ms
    ):
        return False
    if parent.page_start is not None and (
        child.page_start is None or child.page_start < parent.page_start
    ):
        return False
    if parent.page_end is not None and (child.page_end is None or child.page_end > parent.page_end):
        return False
    return True


class DerivedRegions(DerivedAccess):
    async def adopt_region_in(
        self, tx: WriteTx, ctx: CallContext, command: CreateDerivedRegion
    ) -> DerivedRegion:
        if command.parent_id:
            enclosing = await self.read_region_in(tx, ctx, command.parent_id)
            if (
                enclosing.representation != command.representation
                or enclosing.selector is None
                or not contained(enclosing.selector, command.selector)
            ):
                raise DomainError(
                    "derived_region_scope", "Child selection must stay inside its exact parent"
                )
            representation = await self.raw_in(tx, ctx, command.representation)
        else:
            representation = await self.read_in(tx, ctx, command.representation)
        select_payload(representation, command.selector)
        region_id = command.id or str(uuid4())
        existing = await tx.connection.scalar(
            select(derived_regions.c.record).where(derived_regions.c.id == region_id)
        )
        if existing is not None:
            saved = DerivedRegion.model_validate(existing)
            if (
                saved.namespace_id != ctx.namespace_id
                or saved.subject_id != ctx.subject_id
                or saved.representation != command.representation
                or saved.selector != command.selector
                or saved.parent_id != command.parent_id
                or saved.purged
            ):
                raise DomainError(
                    "derived_region_conflict",
                    "Derived region identity already has different coordinates",
                )
            return saved
        value = DerivedRegion(
            id=region_id,
            addresses=await allocate_in(
                tx, ctx, MediaReference(kind="derived_region", id=region_id)
            ),
            namespace_id=ctx.namespace_id,
            subject_id=ctx.require_subject(),
            representation=command.representation,
            selector=command.selector,
            parent_id=command.parent_id,
            recorded_at=tx.recorded_at,
            commit_seq=tx.commit_seq,
        )
        await tx.connection.execute(
            derived_regions.insert().values(
                id=value.id,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                record=value.model_dump(mode="json"),
                commit_seq=tx.commit_seq,
            )
        )
        return value

    async def register_region(
        self, ctx: CallContext, command: CreateDerivedRegion
    ) -> DerivedRegion:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="media.derived_region",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                replay = DerivedRegion.model_validate(tx.replay_result)
                return await self.read_region_in(tx, ctx, replay.id)
            return tx.result(await self.adopt_region_in(tx, ctx, command))

    async def selected_in(
        self, view: ReadView | WriteTx, ctx: CallContext, reference: MediaReference
    ) -> SelectedPayload:
        if reference.kind == "representation":
            value = await self.read_in(view, ctx, representation_ref(reference))
            if reference.observation_id and reference.observation_id not in value.source_roots:
                raise DomainError(
                    "source_unavailable", "Observation is not an input of this derivation"
                )
            assert value.payload is not None
            return SelectedPayload(value.payload)
        if reference.kind == "derived_region":
            region = await self.read_region_in(view, ctx, reference.id)
            if region.selector is None:
                raise DomainError("source_unavailable", "Derived region coordinates were purged")
            value = await self.raw_in(view, ctx, region.representation)
            if reference.observation_id and reference.observation_id not in value.source_roots:
                raise DomainError(
                    "source_unavailable", "Observation is not an input of this derivation"
                )
            return select_payload(value, region.selector)
        raise DomainError("representation_reference_invalid", "A derived reference is required")
