"""A-Memorix public tools composed from formal domain consumers."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.amemorix.contracts import (
    AMemorixOperation,
    AMemorixPolicy,
    IngestInput,
    MaintainInput,
    ProfileInput,
    SearchInput,
    SummaryInput,
    V5AdminInput,
)
from nous_sdk.contracts import (
    CallContext,
    CognitiveKind,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    RecordsQuery,
)
from nous_sdk.contracts.context import CognitiveTarget
from nous_sdk.contracts.records import RecordPurpose
from pydantic import JsonValue, TypeAdapter

from nous.identity.access import require_namespace_in

from .helpers import json_object

if TYPE_CHECKING:
    from nous.application import Application


from . import feedback, ingestion, maintaining, profiles, searching


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
        if operation.tool == "memory_runtime_admin" and operation.arguments.get("action") == "delete_vector_space":
            from .runtime import runtime_admin
            return await runtime_admin(self, ctx, operation.arguments)
        async with self.app.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
        tool, arguments = operation.tool, operation.arguments
        if tool == "enqueue_feedback_task":
            return await feedback.enqueue(self, ctx, arguments)
        if tool == "memory_feedback_admin":
            return await feedback.admin(self, ctx, arguments)
        if tool == "image_memory":
            from .images import image_memory

            return await image_memory(self, ctx, arguments)
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
        if tool == "memory_v5_admin":
            from .maintaining import v5_admin

            return await v5_admin(self, ctx, V5AdminInput.model_validate(arguments))
        if tool in {"memory_correction_admin", "memory_fuzzy_modify_admin"}:
            from .correcting import correction_admin

            return await correction_admin(self, ctx, arguments)
        if tool == "get_person_profile":
            return await self.profile(ctx, ProfileInput.model_validate(arguments))
        if tool == "memory_profile_admin":
            from .profile_admin import profile_admin

            return await profile_admin(self, ctx, arguments)
        if tool == "memory_fact_admin":
            from .facts import fact_admin

            return await fact_admin(self, ctx, arguments)
        if tool == "memory_episode_admin":
            from .episodes import episode_admin

            return await episode_admin(self, ctx, arguments)
        if tool == "memory_source":
            from nous_sdk.contracts.source_graph import ExpandSources

            raw = arguments.get("target")
            if isinstance(raw, str):
                target = (await self.app.references.resolve(ctx, raw)).reference
            else:
                from nous_sdk.contracts.context import ContextTarget

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
        if tool == "memory_tuning_admin":
            from .tuning import tuning_admin

            return await tuning_admin(self, ctx, arguments)
        raise DomainError("capability_unavailable", f"A-Memorix operation {tool!r} is unavailable")

    async def policy(self, ctx: CallContext) -> AMemorixPolicy:
        value = await self.app.configuration.effective(ctx, "integrations.amemorix")
        return AMemorixPolicy.model_validate(value["value"])

    async def records(
        self,
        ctx: CallContext,
        kinds: tuple[CognitiveKind, ...] = (),
        *,
        purpose: RecordPurpose = "current",
    ) -> tuple[CognitiveRecord, ...]:
        command = RecordsQuery(kinds=kinds, purpose=purpose, limit=1000)
        async with self.app.authority.read(ctx) as view:
            result: list[CognitiveRecord] = []
            while True:
                page = await self.app.cognition.records_in(view, ctx, command)
                result.extend(item.record for item in page.items)
                if page.next_cursor is None:
                    return tuple(result)
                command = command.model_copy(update={"cursor": page.next_cursor})

    async def memory_ref(self, ctx: CallContext, target: str) -> CognitiveRef:
        locator = target
        if ":" not in target or (target.startswith("memory:") and "@" not in target):
            identity = target.removeprefix("memory:")
            candidates = [
                record
                for record in await self.records(ctx, ("memory",), purpose="management")
                if record.reference.id == identity and not record.purged
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

    async def retention_ref(self, ctx: CallContext, target: str) -> CognitiveRef:
        from nous_sdk.contracts import AssociationBody, MemoryBody
        token = target.removeprefix("relation:").removeprefix("association:").removeprefix("memory:")
        candidates = [record for record in await self.records(ctx, ("memory", "association"), purpose="management")
            if not record.purged and (record.reference.id == token or record.reference.exact() == target
                or isinstance(record.body, (MemoryBody, AssociationBody)) and target in record.body.external_ids.values())]
        if len(candidates) == 1:
            return candidates[0].reference
        resolved = await self.app.references.resolve(ctx, target.replace("relation:", "association:", 1) if target.startswith("relation:") else target)
        if not isinstance(resolved.reference, CognitiveTarget) or resolved.reference.reference.kind not in {"memory", "association"}:
            raise DomainError("retention_required", "Retention targets must be Memory or Association")
        return resolved.reference.reference
