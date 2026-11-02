"""Local journal, reflection and hypothesis work shares the durable growth owner."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveRecord, DomainError, QueryRequest, TimeFilter
from nous_sdk.contracts.maintenance import MaintenanceSubmission, StartMaintenance
from nous_sdk.contracts.maintenance_tasks import (
    CreateJournal,
    DreamRequest,
    ReflectRequest,
    SummarizeWork,
)
from nous_sdk.contracts.source_objects import ObservationRecord
from nous_sdk.contracts.targets import CognitiveTarget
from nous_sdk.presentation import body_text

from nous.cognition.projection import references_in

if TYPE_CHECKING:
    from .service import MaintenanceService


async def journal_create(
    self: MaintenanceService, ctx: CallContext, command: CreateJournal
) -> MaintenanceSubmission:
    if command.period is None and not command.episodes and not command.references:
        raise DomainError(
            "journal_scope_required", "Choose a period, episodes or actual experience references"
        )
    if any(reference.kind != "episode" for reference in command.episodes) or any(
        reference.kind != "journal" for reference in command.revisits
    ):
        raise DomainError(
            "journal_reference_invalid",
            "Episode and revisited journal inputs require their exact domain references",
        )
    return await self.submit(
        ctx,
        StartMaintenance(
            operation_id=command.operation_id,
            kind="journal",
            instruction=command.purpose,
            references=tuple(
                dict.fromkeys((*command.episodes, *command.references, *command.revisits))
            ),
            query=QueryRequest(
                mode="lexical",
                kinds=("memory", "episode", "journal"),
                domains=("cognition", "observation"),
                times=(TimeFilter(axis="observation", range=command.period),),
                limit=command.limit,
            )
            if command.period
            else None,
            period=command.period,
            style=command.style,
            revisits=command.revisits,
            limit=command.limit,
            auto_adopt=command.auto_adopt,
        ),
    )


async def reflect(
    self: MaintenanceService, ctx: CallContext, command: ReflectRequest
) -> MaintenanceSubmission:
    return await self.submit(
        ctx,
        StartMaintenance(
            operation_id=command.operation_id,
            kind="reflect",
            instruction=command.question,
            references=command.references,
            query=command.query
            or QueryRequest(text=command.question, mode="lexical", limit=command.limit),
            limit=command.limit,
            auto_adopt=command.auto_adopt,
        ),
    )


async def dream(
    self: MaintenanceService, ctx: CallContext, command: DreamRequest
) -> MaintenanceSubmission:
    return await self.submit(
        ctx,
        StartMaintenance(
            operation_id=command.operation_id,
            kind="dream",
            instruction=command.theme,
            references=command.references,
            query=command.query
            or QueryRequest(text=command.theme, mode="lexical", limit=command.limit),
            limit=command.limit,
            auto_adopt=command.auto_adopt,
        ),
    )


async def summarize_work(
    self: MaintenanceService, ctx: CallContext, command: SummarizeWork
) -> MaintenanceSubmission:
    async with self.authority.read(ctx) as view:
        context = await self.retrieval.references.query_context_in(view, ctx, command.work_context)
        references = []
        for locator in context.anchors:
            target = (await self.retrieval.references.resolve_in(view, ctx, locator)).reference
            if isinstance(target, CognitiveTarget):
                references.append(target.reference)
    return await self.submit(
        ctx,
        StartMaintenance(
            operation_id=command.operation_id,
            kind="journal",
            style="work_review",
            instruction=command.purpose or context.goal,
            references=tuple(references),
            query=QueryRequest(text=context.goal, mode="lexical", limit=command.limit)
            if context.goal
            else None,
            limit=command.limit,
            auto_adopt=command.auto_adopt,
        ),
    )


async def prepare_scope(
    self: MaintenanceService, ctx: CallContext, command: StartMaintenance
) -> StartMaintenance:
    references = {reference.exact(): reference for reference in command.references}
    sources = list(command.material_sources)
    query = command.query
    if (
        command.kind == "growth"
        and not references
        and not command.source_catalog_id
        and not command.material_sources
        and query is None
    ):
        from nous_sdk.contracts import RecordsQuery

        current = await self.cognition.records(
            ctx, RecordsQuery(kinds=("memory",), limit=min(command.limit, 32))
        )
        references.update(
            (entry.record.reference.exact(), entry.record.reference) for entry in current.items
        )

    if (
        query is None
        and command.kind in {"growth", "seed"}
        and (references or command.source_catalog_id)
    ):
        fragments: list[str] = []
        async with self.authority.read(ctx) as view:
            for reference in tuple(references.values())[:8]:
                fragments.append(
                    body_text((await self.cognition.get_in(view, ctx, reference)).body)
                )
            if command.source_catalog_id is not None and self.imports is not None:
                catalog = await self.imports.read_catalog_in(view, ctx, command.source_catalog_id)
                from nous.cognition.growth_sources import from_units_in, read_in

                material_sources = await read_in(
                    self.cognition,
                    view,
                    ctx,
                    await from_units_in(self.cognition, view, ctx, catalog.units),
                )
                fragments.extend(source.text for source in material_sources)
        terms = "".join(
            character if character.isalnum() or character.isspace() else " "
            for character in " ".join(fragments)[:4096]
        )
        if terms.strip():
            query = QueryRequest(text=terms, mode="lexical", limit=min(32, command.limit))
    if query is not None:
        selected = await self.retrieval.query(
            ctx, query.model_copy(update={"limit": command.limit})
        )
        for hit in selected.hits:
            if isinstance(hit.object, CognitiveRecord):
                references.setdefault(hit.object.reference.exact(), hit.object.reference)
            elif isinstance(hit.object, ObservationRecord):
                sources.append(hit.object.source)
    if command.kind in {"journal", "reflect", "dream"}:
        # Explicit dependencies belong to the same local catalog, so a model can
        # refer to real episode members or relation endpoints with short keys.
        async with self.authority.read(ctx) as view:
            pending = list(references.values())
            while pending and len(references) < command.limit:
                reference = pending.pop(0)
                record = await self.cognition.get_in(view, ctx, reference)
                for dependency in references_in(record.body):
                    if dependency.exact() not in references and len(references) < command.limit:
                        references[dependency.exact()] = dependency
                        pending.append(dependency)
    return command.model_copy(
        update={
            "references": tuple(references.values())[: command.limit],
            "material_sources": tuple(dict.fromkeys(sources))[: command.limit],
        }
    )
