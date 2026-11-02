"""A-Memorix memory messages project the native material and cognitive contracts."""

from __future__ import annotations

from datetime import UTC, datetime

from nous_sdk.amemorix.contracts import IngestInput, SearchInput
from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    MemoryBody,
    MemoryChanges,
    TimePoint,
    TimeRange,
)
from nous_sdk.types import JsonObject
from pydantic import JsonValue, TypeAdapter

from nous.integrations.amemorix.helpers import json_object, point
from nous.integrations.amemorix.service import AMemorixService

from .context import RPCContext, object_field, required_text, text_field
from .server import public_error


def timestamp(coordinate: int) -> str:
    return datetime.fromtimestamp(coordinate / 1000, UTC).isoformat().replace("+00:00", "Z")


def point_timestamp(value: TimePoint | None) -> str | None:
    return timestamp(value.coordinate) if value and value.domain == "unix_ms" else None


async def selected_memory(
    service: AMemorixService, ctx: CallContext, values: JsonObject
) -> CognitiveRecord:
    memory_id, external_id = text_field(values, "memory_id"), text_field(values, "external_id")
    if bool(memory_id) == bool(external_id):
        raise DomainError("invalid_argument", "Select exactly one memory_id or external_id")
    if memory_id:
        reference = await service.memory_ref(ctx, memory_id)
        async with service.app.authority.read(ctx) as view:
            return await service.app.cognition.get_in(view, ctx, reference, purpose="current")
    candidates = [
        record
        for record in await service.records(ctx, ("memory",))
        if isinstance(record.body, MemoryBody) and external_id in record.body.external_ids.values()
    ]
    if len(candidates) != 1:
        raise DomainError(
            "memory_not_found" if not candidates else "memory_selector_conflict",
            "External identity must identify one available Memory; use memory_id for chunked input",
            external_id=external_id,
            memory_ids=[item.reference.id for item in candidates],
        )
    return candidates[0]


async def memory_record(
    service: AMemorixService, ctx: CallContext, record: CognitiveRecord
) -> JsonObject:
    if not isinstance(record.body, MemoryBody):
        raise DomainError("memory_required", "The selected object is not a Memory")
    roots = set(record.source_roots)
    sources = [item for item in await service.app.sources.list(ctx) if item["id"] in roots]
    primary = sources[0] if len(sources) == 1 else None
    result: JsonObject = {
        "memory_id": record.reference.id,
        "external_id": next(iter(record.body.external_ids.values()), ""),
        "source_type": primary["source_type"] if primary else "",
        "source": primary.get("source", "") or "" if primary else "",
        "content": record.body.text,
        "metadata": json_object(
            {
                **record.body.metadata,
                "reference": record.reference,
                "source_roots": record.source_roots,
            }
        ),
    }
    for name, value in (
        ("created_at", point_timestamp(record.times.formation)),
        ("updated_at", point_timestamp(record.times.recorded)),
        ("observed_at", point_timestamp(record.times.observation)),
    ):
        if value is not None:
            result[name] = value
    validity = record.times.validity
    if validity is not None and validity.domain == "unix_ms":
        for name, coordinate in (("valid_from", validity.start), ("valid_to", validity.end)):
            if coordinate is not None:
                result[name] = timestamp(coordinate)
    return result


def ingest_command(values: JsonObject, *, batch_index: int | None = None) -> IngestInput:
    context = object_field(values, "context")
    mode = text_field(values, "relation_extraction", "RELATION_EXTRACTION_MODE_UNSPECIFIED")
    modes = {
        "RELATION_EXTRACTION_MODE_UNSPECIFIED": "default",
        "RELATION_EXTRACTION_MODE_ENABLED": "enabled",
        "RELATION_EXTRACTION_MODE_DISABLED": "disabled",
    }
    if mode not in modes:
        raise DomainError("invalid_argument", "Unknown relation extraction mode")
    payload: JsonObject = {
        name: value
        for name, value in values.items()
        if name not in {"context", "valid_from", "valid_to", "relation_extraction"}
    }
    payload.update(
        chat_id=text_field(context, "conversation_id"),
        user_id=text_field(context, "user_id"),
        group_id=text_field(context, "group_id"),
        relation_extraction=modes[mode],
        deferred_formation=True,
    )
    supplied_key = text_field(context, "idempotency_key") or text_field(context, "request_id")
    if supplied_key:
        payload["operation_id"] = (
            "rpc-ingest:" + supplied_key + (f":{batch_index}" if batch_index is not None else "")
        )
    if "valid_from" in values or "valid_to" in values:
        start = point(text_field(values, "valid_from") or None)
        end = point(text_field(values, "valid_to") or None)
        payload["validity"] = TimeRange(
            start=start.coordinate if start else None, end=end.coordinate if end else None
        ).model_dump(mode="json")
    return IngestInput.model_validate(payload)


def ingest_response(result: JsonObject) -> JsonObject:
    return {
        name: result[name]
        for name in (
            "stored_ids",
            "skipped_ids",
            "fact_claim_ids",
            "warnings",
            "detail",
            "relation_extraction_job_id",
        )
        if name in result and result[name] is not None
    }


async def execute(method: str, values: JsonObject, rpc: RPCContext) -> JsonObject:
    context = object_field(values, "context")
    namespace_id = required_text(context, "namespace_id")
    ctx = await rpc.scope(
        namespace_id,
        "read" if method in {"GetMemory", "SearchMemory"} else "write",
        subject=True,
        request=values,
    )
    service = rpc.app.amemorix
    if method == "IngestText":
        return ingest_response(await service.ingest(ctx, ingest_command(values)))
    if method == "BatchIngestText":
        items = TypeAdapter(tuple[JsonObject, ...]).validate_python(values.get("items", []))
        if not 1 <= len(items) <= 1000:
            raise DomainError("invalid_argument", "A batch requires 1 to 1000 items")
        results: list[JsonValue] = []
        succeeded = 0
        for index, item in enumerate(items):
            try:
                response = await service.ingest(
                    ctx, ingest_command({**item, "context": context}, batch_index=index)
                )
                results.append({"index": index, "response": ingest_response(response)})
                succeeded += 1
            except Exception as error:
                code, _, message, details = public_error(error)
                results.append(
                    {
                        "index": index,
                        "error": {
                            "code": code,
                            "message": message,
                            "details": details,
                            "request_id": text_field(context, "request_id"),
                            "trace_id": text_field(context, "trace_id"),
                        },
                    }
                )
        return {"results": results, "succeeded": succeeded, "failed": len(items) - succeeded}
    if method == "GetMemory":
        return {
            "memory": await memory_record(service, ctx, await selected_memory(service, ctx, values))
        }
    if method == "DeleteMemory":
        operation = rpc.operation_id(values, method)
        payload = {key: values[key] for key in ("memory_id", "external_id", "reason") if key in values}
        async with rpc.app.authority.write(
            ctx, operation, operation_kind="amemorix.rpc.delete_memory",
            scope=ctx.namespace_id, payload=payload,
        ) as tx:
            if tx.replayed:
                changed = await rpc.app.cognition.replay_records_in(
                    tx, ctx, TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(tx.replay_result)
                )
            else:
                record = await selected_memory(service, ctx, values)
                changed = tx.result(await rpc.app.memories.change_memories_in(
                    tx, ctx, MemoryChanges(
                        operation_id=operation, references=(record.reference,), action="trash",
                        reason=text_field(values, "reason"),
                    ),
                ))
        await rpc.app.retrieval.invalidate()
        return {
            "operation_id": operation,
            "deleted_count": len(changed),
            "deleted_memory_ids": [item.reference.id for item in changed],
        }
    if method == "SearchMemory":
        modes = {
            "SEARCH_MODE_UNSPECIFIED": "search",
            "SEARCH_MODE_SEARCH": "search",
            "SEARCH_MODE_TIME": "time",
            "SEARCH_MODE_HYBRID": "hybrid",
            "SEARCH_MODE_EPISODE": "episode",
            "SEARCH_MODE_AGGREGATE": "aggregate",
        }
        mode = text_field(values, "mode", "SEARCH_MODE_UNSPECIFIED")
        if mode not in modes:
            raise DomainError("invalid_argument", "Unknown search mode")
        command = SearchInput.model_validate(
            {
                **{
                    name: value
                    for name, value in values.items()
                    if name not in {"context", "mode", "shared_conversation_ids"}
                },
                "mode": modes[mode],
                "shared_chat_ids": values.get("shared_conversation_ids", []),
                "chat_id": text_field(context, "conversation_id"),
                "user_id": text_field(context, "user_id"),
                "group_id": text_field(context, "group_id"),
            }
        )
        result = await service.search(ctx, command)
        coverage = object_field(result, "coverage")
        hits = TypeAdapter(tuple[JsonObject, ...]).validate_python(result.get("hits", []))
        return {
            "summary": result.get("summary", ""),
            "hits": [
                {
                    "memory_id": hit.get("hash", ""),
                    "kind": hit.get("type", ""),
                    **{
                        key: value
                        for key, value in hit.items()
                        if key in {"title", "content", "score", "source", "metadata"}
                    },
                }
                for hit in hits
            ],
            "filtered": result.get("filtered", False),
            "degraded": result.get("degraded", coverage.get("complete") is False),
            "retrieval_ready": result.get("retrieval_ready", False),
            "retrieval_mode": result.get("retrieval_mode", command.mode),
            "available_channels": result.get("available_channels", []),
            "unavailable_channels": result.get("unavailable_channels", []),
        }
    raise DomainError("rpc_method_not_found", "Unknown memory operation")
