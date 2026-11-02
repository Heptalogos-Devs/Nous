"""A-Memorix searching through the formal domain owners."""

from __future__ import annotations

from nous_sdk.presentation import body_text

from typing import TYPE_CHECKING

from nous_sdk.amemorix.contracts import (
    SearchInput,
)
from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    EntityBody,
    QueryRequest,
    TimeFilter,
    TimeRange,
)
from pydantic import JsonValue


from .helpers import json_object, point

if TYPE_CHECKING:
    from .service import AMemorixService


async def search(
    self: AMemorixService, ctx: CallContext, command: SearchInput
) -> dict[str, JsonValue]:
    policy = await self.policy(ctx)
    if not policy.permits(
        command.respect_filter, command.chat_id, command.group_id, command.user_id
    ):
        return {"success": True, "summary": "", "hits": [], "filtered": True}
    sources = await self.app.sources.list(ctx)
    chats = {value for value in (command.chat_id, *command.shared_chat_ids) if value}
    selected = tuple(
        source["id"]
        for source in sources
        if (
            not chats
            or source["origin"].get("conversation_ref") in chats
            or source["source_type"] == "web_import"
        )
        and (
            not command.respect_filter or source["source_type"] not in policy.excluded_source_types
        )
    )
    text = command.query
    if command.person_id:
        names = [
            record.body.label
            for record in await self.records(ctx, ("entity",))
            if isinstance(record.body, EntityBody)
            and record.body.identifiers.get("maibot:person") == command.person_id
        ]
        if not names:
            return {"success": True, "summary": "", "hits": [], "filtered": False}
        import json

        text += " @entity(" + json.dumps(names[0], ensure_ascii=False) + ")"
    start, end = point(command.time_start), point(command.time_end)
    times = (
        (
            TimeFilter(
                axis="event",
                range=TimeRange(
                    start=start.coordinate if start else None,
                    end=end.coordinate if end else None,
                ),
            ),
        )
        if start or end
        else ()
    )
    if command.mode == "time":
        text += " $sort(occurred,desc)"
    elif command.mode == "aggregate":
        import json

        branches = (
            text + " $return(memory)",
            text + " $return(episode)",
            text + " $sort(occurred,desc) $mode(lexical)",
        )
        text = (
            "$any(" + ",".join(json.dumps(branch, ensure_ascii=False) for branch in branches) + ")"
        )
    kinds = ("episode",) if command.mode == "episode" else ()
    mode = (
        "hybrid"
        if command.mode == "hybrid"
        else "lexical"
        if command.mode == "time"
        else policy.search_mode
    )
    query = QueryRequest.model_validate(
        {
            "text": text,
            "mode": mode,
            "kinds": kinds,
            "limit": command.limit,
            "capabilities": {"semantic": "optional"},
            "source_ids": selected,
            "times": [value.model_dump(mode="json") for value in times],
        }
    )
    result = await self.app.retrieval.query(ctx, query)
    execution = result.execution
    effective_mode = execution.effective_mode if execution is not None else mode
    available = ["metadata"]
    if effective_mode in {"lexical", "hybrid", "vcp"}:
        available.append("sparse")
    if effective_mode in {"dense", "hybrid", "vcp"}:
        available.extend(("vector_read", "embedding"))
    if effective_mode == "vcp":
        available.append("graph")
    unavailable: list[str] = []
    if execution is not None:
        if "model_unavailable" in execution.reasons:
            unavailable.append("embedding")
        if any(
            "vectors" in reason or "semantic_unavailable" in reason for reason in execution.reasons
        ):
            unavailable.append("vector_read")
    hits: list[dict[str, JsonValue]] = []
    for hit in result.hits:
        if not isinstance(hit.object, CognitiveRecord) or hit.object.body is None:
            continue
        record = hit.object
        content = body_text(record.body)
        hits.append(
            json_object(
                {
                    "hash": record.reference.id
                    if record.reference.kind == "memory"
                    else record.reference.exact(),
                    "type": "paragraph"
                    if record.reference.kind == "memory"
                    else record.reference.kind,
                    "title": content[:80],
                    "content": content,
                    "score": hit.score,
                    "source": next(
                        (
                            source["source_type"]
                            for source in sources
                            if source["id"] in record.source_roots
                        ),
                        "",
                    ),
                    "metadata": {
                        "reference": record.reference,
                        "source_roots": record.source_roots,
                    },
                    "episode_id": record.reference.exact()
                    if record.reference.kind == "episode"
                    else "",
                }
            )
        )
    return json_object(
        {
            "success": True,
            "summary": "\n".join(
                f"{index + 1}. {str(hit['content'])[:120]}" for index, hit in enumerate(hits[:5])
            ),
            "hits": hits,
            "filtered": False,
            "coverage": result.coverage,
            "retrieval_mode": effective_mode,
            "degraded": not result.coverage.complete or bool(execution and execution.degraded),
            "retrieval_ready": bool(result.coverage.complete),
            "available_channels": available,
            "unavailable_channels": unavailable,
            "execution": execution,
            "time_start": command.time_start,
            "time_end": command.time_end,
        }
    )
