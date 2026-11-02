"""Read a resolved object and its authorized source closure."""

from __future__ import annotations

import base64
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.common import Contract
from nous_sdk.contracts.context import CognitiveTarget, MediaTarget
from nous_sdk.contracts.public import ReadRequest, ReadView
from nous_sdk.contracts.references import ResourceTarget
from nous_sdk.contracts.source_graph import ExpandSources
from nous_sdk.presentation import body_text
from pydantic import JsonValue, TypeAdapter

if TYPE_CHECKING:
    from nous.retrieval.service import RetrievalService

    from .service import ReferenceDirectory


class ReadingService:
    def __init__(
        self, directory: ReferenceDirectory, retrieval: RetrievalService, execution, maintenance
    ):
        self.directory, self.retrieval = directory, retrieval
        self.execution, self.maintenance = execution, maintenance

    async def read(self, ctx: CallContext, command: ReadRequest, *, target=None) -> ReadView:
        offset = command.offset
        if command.cursor is not None:
            try:
                raw = base64.urlsafe_b64decode(command.cursor + "=" * (-len(command.cursor) % 4))
                cursor = _ReadCursor.model_validate_json(raw)
                if cursor.namespace_id != ctx.namespace_id or cursor.ref != command.ref:
                    raise ValueError("Cursor selects another read")
                offset = cursor.offset
            except (ValueError, TypeError) as error:
                raise DomainError(
                    "read_cursor_invalid",
                    "Read cursor does not belong to this Namespace and reference",
                ) from error
        if command.ref.startswith(("work:", "job:", "maintenance:")):
            kind, identity = command.ref.split(":", 1)
            if kind == "work":
                from nous.context.lifecycle import work_record_in

                async with self.directory.authority.read(ctx) as view:
                    work = await work_record_in(view, ctx, identity)
                    for anchor in work.content.anchors:
                        await self.directory.resolve_target_in(view, ctx, anchor, None)
                value = work.model_dump(mode="json")
                title = work.content.purpose
                text = "\n\n".join(
                    part
                    for part in (
                        work.content.purpose,
                        work.content.context_text,
                        "\n".join(work.content.unresolved_questions),
                    )
                    if part
                )
            elif kind == "job":
                from nous.platform.persistence.authority import canonical_json

                job = await self.execution.status(ctx, identity)
                value = {
                    "id": job.workflow_id,
                    "name": job.name,
                    "status": job.status,
                    "result": job.output,
                    "error": str(job.error) if job.error else None,
                }
                title = job.name or identity
                text = canonical_json(value)
            else:
                from nous.platform.persistence.authority import canonical_json

                run = await self.maintenance.read(ctx, identity)
                value = run.model_dump(mode="json")
                title = run.command.purpose
                text = canonical_json(value)
            content = text[offset : offset + command.max_chars]
            following = None
            if offset + len(content) < len(text):
                following = (
                    base64.urlsafe_b64encode(
                        _ReadCursor(
                            namespace_id=ctx.namespace_id,
                            ref=command.ref,
                            offset=offset + len(content),
                        )
                        .model_dump_json()
                        .encode()
                    )
                    .decode()
                    .rstrip("=")
                )
            return ReadView(
                ref=command.ref,
                kind=kind,
                title=title,
                text=content,
                value=TypeAdapter(JsonValue).validate_python(value),
                next_cursor=following,
            )
        async with self.directory.authority.read(ctx) as view:
            if target is None:
                resolution = await self.directory.resolve_in(view, ctx, command.ref)
                target = resolution.reference
            else:
                target = await self.directory.resolve_target_in(view, ctx, target, None)
            reference = target.reference
            if isinstance(target, CognitiveTarget):
                record = await self.directory.cognition.get_in(view, ctx, reference)
                value = record.model_dump(mode="json")
                text = body_text(record.body)
                title = (text.splitlines() or [reference.kind])[0][:160]
            elif isinstance(target, MediaTarget):
                if reference.kind == "material":
                    record, _, _ = await self.directory.materials.media_source_in(
                        view, ctx, reference
                    )
                    value = record
                    text = await self.directory.materials.text_in(view, ctx, reference)
                    title = record["filename"]
                elif reference.kind == "source_region":
                    region = await self.directory.materials.read_region_in(view, ctx, reference.id)
                    material = await self.directory.materials.read_in(view, ctx, region.material_id)
                    value = region.model_dump(mode="json")
                    title = material["filename"]
                    text = await self.directory.materials.text_in(view, ctx, reference)
                else:
                    selected = await self.directory.derivations.selected_in(view, ctx, reference)
                    value = {
                        "payload": selected.payload.model_dump(mode="json"),
                        "selector": selected.selector.model_dump(mode="json")
                        if selected.selector
                        else None,
                    }
                    title = reference.kind
                    payload = selected.payload
                    from nous_sdk.contracts.derivation import (
                        StructuredRepresentation,
                        TextRepresentation,
                    )

                    from nous.platform.persistence.authority import canonical_json

                    text = (
                        payload.text
                        if isinstance(payload, TextRepresentation)
                        else canonical_json(payload.value)
                        if isinstance(payload, StructuredRepresentation)
                        else ""
                    )
            elif isinstance(target, ResourceTarget):
                if self.directory.resources is None:
                    raise DomainError("resource_unavailable", "Resource reader is unavailable")
                record = await self.directory.resources.get_in(
                    view, ctx, reference.id, reference.revision
                )
                value = record.model_dump(mode="json")
                text = record.text
                title = record.name
            else:
                record = await self.directory.read_source_object_in(view, ctx, reference)
                value = record.model_dump(mode="json")
                text = record.text
                title = reference.kind
        content = text[offset : offset + command.max_chars]
        next_offset = offset + len(content)
        next_cursor = None
        if next_offset < len(text):
            cursor = _ReadCursor(namespace_id=ctx.namespace_id, ref=command.ref, offset=next_offset)
            next_cursor = (
                base64.urlsafe_b64encode(cursor.model_dump_json().encode()).decode().rstrip("=")
            )
        sources = (
            await self.retrieval.expand_sources(ctx, ExpandSources(targets=(target,)))
            if command.sources
            else None
        )
        return ReadView(
            ref=reference.exact(),
            kind=reference.kind,
            title=title,
            text=content,
            value=TypeAdapter(JsonValue).validate_python(value),
            sources=sources,
            next_cursor=next_cursor,
        )


class _ReadCursor(Contract):
    namespace_id: str
    ref: str
    offset: int
