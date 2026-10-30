"""MaiBot Episode actions consume the Native Episode and execution owners."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.amemorix.episodes import EpisodeAdminInput
from nous_sdk.contracts import (
    CallContext,
    CognitiveRef,
    DomainError,
    EpisodeBody,
    MemoryBody,
    TimeRange,
)
from nous_sdk.contracts.episodes import (
    EpisodeQuery,
    EpisodeView,
    ProcessEpisodeSources,
    RebuildEpisodes,
)
from pydantic import JsonValue

from .helpers import json_object, point

if TYPE_CHECKING:
    from .service import AMemorixService


def item(view: EpisodeView, *, paragraphs: bool = False) -> dict[str, JsonValue]:
    record, body = view.record, view.record.body
    assert isinstance(body, EpisodeBody)
    result: dict[str, JsonValue] = {
        "episode_id": record.reference.exact(),
        "id": record.reference.id,
        "reference": record.reference.model_dump(mode="json"),
        "revision": record.reference.revision,
        "title": body.title,
        "summary": body.summary,
        "sources": list(view.sources),
        "source": view.sources[0] if len(view.sources) == 1 else None,
        "paragraph_count": len(body.members),
        "boundary_explanation": body.boundary_explanation,
        "state": body.state,
        "track": body.track,
        "lineage": [ref.model_dump(mode="json") for ref in body.lineage],
        "created_at": record.times.formation.coordinate / 1000
        if record.times.formation.domain == "unix_ms"
        else None,
        "updated_at": record.times.recorded.coordinate / 1000
        if record.times.recorded.domain == "unix_ms"
        else None,
        "event_time_start": body.experience.start / 1000
        if body.experience.domain == "unix_ms" and body.experience.start is not None
        else None,
        "event_time_end": body.experience.end / 1000
        if body.experience.domain == "unix_ms" and body.experience.end is not None
        else None,
    }
    if paragraphs:
        result["paragraphs"] = [
            {
                "hash": member.reference.exact(),
                "reference": member.reference.model_dump(mode="json"),
                "content": member.body.text,
                "source_roots": list(member.source_roots),
            }
            for member in view.members
            if isinstance(member.body, MemoryBody)
        ]
    return result


async def episode_admin(
    self: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = EpisodeAdminInput.model_validate(arguments)
    owner = self.app.episodes
    if command.action in {"query", "list"}:
        person: CognitiveRef | None = None
        if command.person_id:
            from nous_sdk.amemorix.contracts import ProfileInput

            from .profile_read import select_person

            person = (
                await select_person(self, ctx, ProfileInput(person_id=command.person_id))
            ).reference
        start, end = point(command.time_start), point(command.time_end)
        views = await owner.query(
            ctx,
            EpisodeQuery(
                query=command.query,
                source=command.source or None,
                person=person,
                experience=TimeRange(
                    start=start.coordinate if start else None, end=end.coordinate if end else None
                )
                if start is not None or end is not None
                else None,
                limit=command.limit,
            ),
        )
        items = [item(view) for view in views]
        return json_object({"success": True, "items": items, "count": len(items)})
    if command.action == "get":
        if not command.episode_id:
            raise DomainError("episode_required", "get requires the exact episode_id")
        address = (
            command.episode_id
            if command.episode_id.startswith("episode:")
            else "episode:" + command.episode_id
        )
        resolved = await self.app.references.resolve(ctx, address)
        from nous_sdk.contracts.context import CognitiveTarget

        if (
            not isinstance(resolved.reference, CognitiveTarget)
            or resolved.reference.reference.kind != "episode"
        ):
            raise DomainError("episode_required", "The address must identify an Episode")
        view = await owner.get(ctx, resolved.reference.reference, command.paragraph_limit)
        return {"success": True, "episode": item(view, paragraphs=True)}
    if command.action == "status":
        return {"success": True, **await owner.status(ctx, command.limit)}
    if command.action == "rebuild":
        if not command.operation_id:
            raise DomainError(
                "operation_id_required", "Episode rebuild requires a stable operation_id"
            )
        sources = command.sources or ((command.source,) if command.source else ())
        if not sources and command.all:
            sources = await owner.source_names(ctx)
        if not sources:
            raise DomainError("episode_source_required", "No source was selected for rebuilding")
        submission = await owner.submit_rebuild(
            ctx, RebuildEpisodes(operation_id=command.operation_id, sources=sources)
        )
        result = await self.app.execution.wait(ctx, submission.job_id)
        if not isinstance(result, dict):
            raise DomainError(
                "episode_result_invalid", "Episode rebuild returned no structured result"
            )
        return json_object({"success": True, **result, "job_id": submission.job_id})
    if command.action == "process_sources":
        result = await owner.process_sources(
            ctx, ProcessEpisodeSources(limit=command.limit, max_retry=command.max_retry)
        )
        return {"success": result["failed"] == 0 and result.get("unfinished", 0) == 0, **result}
    raise DomainError(
        "capability_unavailable",
        "This Episode action has no Native persistent facts",
        action=command.action,
    )
