"""A-Memorix public tools composed from formal domain consumers."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.amemorix.contracts import (
    AMemorixOperation,
    AMemorixPolicy,
    IngestInput,
    MaintainInput,
    ProfileInput,
    SearchInput,
    SummaryInput,
)
from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
)
from nous_wave.contracts.context import CognitiveTarget
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.cognition.schema import revisions
from nous.identity.service import require_subject_in

from .helpers import json_object

if TYPE_CHECKING:
    from nous.application import Application


from . import ingestion, maintaining, profiles, searching


class AMemorixService:
    search = searching.search
    ingest = ingestion.ingest
    adopt_host_graph = ingestion.adopt_host_graph
    profile = profiles.profile
    stats = profiles.stats
    maintain = maintaining.maintain

    def __init__(self, app: Application) -> None:
        self.app = app

    async def execute(self, ctx: CallContext, operation: AMemorixOperation) -> dict[str, JsonValue]:
        async with self.app.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
        tool, arguments = operation.tool, operation.arguments
        if tool == "search_memory":
            return await self.search(ctx, SearchInput.model_validate(arguments))
        if tool in {"ingest_text", "ingest_summary"}:
            command = (
                SummaryInput.model_validate(arguments)
                if tool == "ingest_summary"
                else IngestInput.model_validate(arguments)
            )
            return await self.ingest(ctx, command)
        if tool == "maintain_memory":
            return await self.maintain(ctx, MaintainInput.model_validate(arguments))
        if tool == "get_person_profile":
            return await self.profile(ctx, ProfileInput.model_validate(arguments))
        if tool == "memory_profile_admin":
            from .profile_admin import profile_admin

            return await profile_admin(self, ctx, arguments)
        if tool == "memory_source":
            from nous_wave.contracts.source_graph import ExpandSources

            raw = arguments.get("target")
            if isinstance(raw, str):
                target = (await self.app.references.resolve(ctx, raw)).reference
            else:
                from nous_wave.contracts.context import ContextTarget

                target = TypeAdapter(ContextTarget).validate_python(raw)
            if not isinstance(target, CognitiveTarget):
                raise DomainError(
                    "cognition_required", "Memory source requires a cognitive revision"
                )
            record = await self.app.cognition.get(ctx, target.reference)
            return json_object(
                {
                    "success": True,
                    "memory": record,
                    "reference": target.reference,
                    "sources": record.source_roots,
                    "evidence": record.evidence,
                    "graph": await self.app.retrieval.expand_sources(
                        ctx, ExpandSources(targets=(target,))
                    ),
                }
            )
        if tool == "memory_stats":
            return await self.stats(ctx)
        if tool == "memory_graph_admin":
            from .graph import graph_admin

            return await graph_admin(self, ctx, arguments)
        if tool in {"memory_source_admin", "memory_delete_admin"}:
            from .sources import source_admin

            return await source_admin(self, ctx, tool, arguments)
        if tool == "memory_import_admin":
            from .imports import import_admin

            return await import_admin(self, ctx, arguments)
        if tool == "memory_bundle_admin":
            from .bundles import bundle_admin

            return await bundle_admin(self, ctx, arguments)
        if tool == "memory_runtime_admin":
            from .runtime import runtime_admin

            return await runtime_admin(self, ctx, arguments)
        raise DomainError("capability_unavailable", f"A-Memorix operation {tool!r} is unavailable")

    async def policy(self, ctx: CallContext) -> AMemorixPolicy:
        value = await self.app.configuration.effective(ctx, "integrations.amemorix")
        return AMemorixPolicy.model_validate(value["value"])

    async def records(
        self, ctx: CallContext, kinds: tuple[str, ...] = ()
    ) -> tuple[CognitiveRecord, ...]:
        query = select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.subject_id == ctx.subject_id,
            revisions.c.head.is_(True),
        )
        if kinds:
            query = query.where(revisions.c.kind.in_(kinds))
        async with self.app.authority.read(ctx) as view:
            records = [
                CognitiveRecord.model_validate(value)
                for value in (await view.connection.execute(query)).scalars()
            ]
            visible: list[CognitiveRecord] = []
            roots = await self.app.cognition.active_roots_in(view, ctx)
            for record in records:
                if not record.purged and await self.app.cognition.visible_in(
                    view, ctx, record, roots
                ):
                    visible.append(record)
            return tuple(visible)

    async def memory_ref(self, ctx: CallContext, target: str) -> CognitiveRef:
        locator = target
        if ":" not in target or (target.startswith("memory:") and "@" not in target):
            identity = target.removeprefix("memory:")
            candidates = [
                record
                for record in await self.records(ctx, ("memory",))
                if record.reference.id == identity
            ]
            if len(candidates) != 1:
                raise DomainError("memory_not_found", "Memory target is unavailable or ambiguous")
            locator = candidates[0].reference.exact()
        resolved = await self.app.references.resolve(ctx, locator)
        if (
            not isinstance(resolved.reference, CognitiveTarget)
            or resolved.reference.reference.kind != "memory"
        ):
            raise DomainError("memory_required", "Maintenance target must be Memory")
        return resolved.reference.reference
