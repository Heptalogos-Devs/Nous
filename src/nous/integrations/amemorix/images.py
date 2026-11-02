"""Image compatibility uses original Materials, actual contacts and native cognition."""

from __future__ import annotations

import base64
from dataclasses import dataclass, replace
from datetime import datetime
from email.encoders import encode_noop
from email.mime.image import MIMEImage
from hashlib import file_digest, sha256
from pathlib import Path
from typing import TYPE_CHECKING

from nous_sdk.amemorix.contracts import ImageInput
from nous_sdk.contracts import (
    BlobDocument,
    CallContext,
    ContactMaterial,
    DomainError,
    MediaReference,
    MemoryBody,
    QueryRequest,
    TimePoint,
)
from nous_sdk.contracts.context import CognitiveTarget
from nous_sdk.contracts.management import SourceCommand
from nous_sdk.contracts.namespace_policy import NamespaceFeatures
from nous_sdk.contracts.query_conditions import QueryCapabilities
from nous_sdk.contracts.source_objects import EvidenceReference
from nous_sdk.contracts.vector_projection import ProjectionResult, ProjectVectors
from pydantic import JsonValue, TypeAdapter

from nous.material.service import MaterialRecord, SourceRecord, require_observation_in
from nous.platform.concurrency import owned_thread
from nous.platform.content import ContentObject

from .helpers import json_object, point

if TYPE_CHECKING:
    from .service import AMemorixService


@dataclass(frozen=True)
class ImageAsset:
    material: MaterialRecord
    events: tuple[SourceRecord, ...]
    legacy_hash: str | None = None


def point_seconds(value: object) -> float | None:
    if value is None:
        return None
    point = TimePoint.model_validate(value)
    return point.coordinate / 1000 if point.domain == "unix_ms" else None


def legacy_digest(path: Path) -> str:
    with path.open("rb") as stream:
        return file_digest(stream, "md5").hexdigest()


async def inventory(
    service: AMemorixService, ctx: CallContext, *, legacy: bool = False
) -> tuple[ImageAsset, ...]:
    """Read through the source and material owners, retaining their current permissions."""
    async with service.app.authority.read(ctx) as view:
        grouped: dict[str, list[SourceRecord]] = {}
        for raw in await service.app.sources.list_in(view, ctx):
            if raw["active"] and not raw.get("purged") and raw["kind"] == "contact":
                event = TypeAdapter(SourceRecord).validate_python(raw)
                grouped.setdefault(event["material_id"], []).append(event)
        assets: list[ImageAsset] = []
        for identifier, events in grouped.items():
            try:
                material = await service.app.material.read_in(view, ctx, identifier)
            except DomainError as error:
                if error.code != "source_unavailable":
                    raise
                continue
            if material["media_type"].startswith("image/"):
                legacy_hash = None
                if legacy:
                    async with service.app.content.protect(
                        ContentObject(
                            material["content_hash"], material["byte_len"], material["media_type"]
                        )
                    ) as path:
                        legacy_hash = await owned_thread(lambda: legacy_digest(path))
                assets.append(ImageAsset(material, tuple(events), legacy_hash))
    by_content: dict[str, ImageAsset] = {}
    for asset in sorted(assets, key=lambda item: item.material["commit_seq"]):
        key = asset.material["content_hash"]
        previous = by_content.get(key)
        by_content[key] = (
            ImageAsset(previous.material, (*previous.events, *asset.events), previous.legacy_hash)
            if previous
            else asset
        )
    return tuple(
        sorted(
            by_content.values(),
            key=lambda asset: max(event["commit_seq"] for event in asset.events),
            reverse=True,
        )
    )


def selected(asset: ImageAsset, command: ImageInput) -> bool:
    material = asset.material
    return bool(
        command.asset_id in {event["material_id"] for event in asset.events}
        or command.material_id in {event["material_id"] for event in asset.events}
        or command.occurrence_id in {event["id"] for event in asset.events}
        or command.content_hash
        and command.content_hash.casefold() in {material["content_hash"], asset.legacy_hash}
    )


def encounters(asset: ImageAsset, command: ImageInput) -> tuple[SourceRecord, ...]:
    allowed = (
        command.chat_ids
        if "chat_ids" in command.model_fields_set
        else (command.chat_id,)
        if command.chat_id
        else None
    )
    return tuple(
        event
        for event in asset.events
        if event["kind"] == "contact"
        and (allowed is None or event["origin"].get("conversation_ref", "") in allowed)
        and (
            not command.current_external_ref
            or event.get("external_event_id") != command.current_external_ref
        )
        and event["id"] != command.current_occurrence_id
    )


async def detail(
    service: AMemorixService, ctx: CallContext, asset: ImageAsset, command: ImageInput
) -> dict[str, JsonValue]:
    async with service.app.authority.read(ctx) as view:
        live: list[SourceRecord] = []
        for event in asset.events:
            try:
                await require_observation_in(view, ctx, event["id"])
            except DomainError as error:
                if error.code != "source_unavailable":
                    raise
            else:
                live.append(event)
        asset = replace(asset, events=tuple(live))
        await service.app.material.read_in(view, ctx, asset.material["id"])
    occurrences = encounters(asset, command)
    if (command.chat_ids is not None or command.chat_id) and not occurrences:
        raise DomainError(
            "source_unavailable", "Image has no occurrence in the selected conversation scope"
        )
    occurrence_ids = {event["id"] for event in occurrences}
    event_ids = {event["id"] for event in asset.events}
    related = [
        record
        for record in await service.records(ctx, ("memory",))
        if event_ids.intersection(record.source_roots)
        and (
            not isinstance(record.body, MemoryBody)
            or record.body.contact_observation_id is None
            or record.body.contact_observation_id in occurrence_ids
        )
    ]
    links = [
        {
            "link_id": EvidenceReference(id=f"{record.reference.exact()}#{index}").exact(),
            "occurrence_id": link.observation_id,
            "target_type": record.reference.kind,
            "target_id": record.reference.id,
            "target": record.reference,
            "link_kind": link.role.removeprefix("media.link:"),
            "evidence": link.details,
        }
        for record in await service.records(ctx, ("memory", "entity", "association", "fact_claim"))
        for index, link in enumerate(record.evidence)
        if link.observation_id in occurrence_ids and link.role.startswith("media.link:")
    ]
    observations: list[dict[str, JsonValue]] = [
        {
            "observation_id": event["id"],
            "occurrence_id": event["id"],
            "text": event.get("statement", ""),
            "source_kind": "user_statement",
            "confirm_status": "unconfirmed",
            "version": 1,
        }
        for event in occurrences
        if event.get("statement")
    ]
    observations.extend(
        {
            "observation_id": record.reference.exact(),
            "text": record.body.text,
            "source_kind": record.body.structured.get("source_kind", "native_memory")
            if isinstance(record.body.structured, dict)
            else "native_memory",
            "confirm_status": record.body.structured.get("confirmation", "unconfirmed")
            if isinstance(record.body.structured, dict)
            else "unconfirmed",
            "version": record.reference.revision,
            "reference": json_object(record.reference),
        }
        for record in related
        if isinstance(record.body, MemoryBody)
        and not any(
            "contact" in record.body.external_ids
            and record.body.contact_observation_id == event["id"]
            and record.body.text == event.get("statement")
            for event in occurrences
        )
    )
    info = asset.material.get("media_info") or {}
    return json_object(
        {
            "success": True,
            "asset": {
                "asset_id": asset.material["id"],
                "material_ids": sorted({event["material_id"] for event in asset.events}),
                "content_hash": asset.material["content_hash"],
                "mime_type": asset.material["media_type"],
                "byte_len": asset.material["byte_len"],
                "byte_size": asset.material["byte_len"],
                "width": info.get("width"),
                "height": info.get("height"),
                "status": "active",
                "created_at": datetime.fromisoformat(asset.material["recorded_at"]).timestamp(),
                "updated_at": max(
                    (
                        datetime.fromisoformat(event["recorded_at"]).timestamp()
                        for event in asset.events
                    ),
                    default=datetime.fromisoformat(asset.material["recorded_at"]).timestamp(),
                ),
                "occurrence_count": len(occurrences),
                "filename": asset.material["filename"],
                "media_info": asset.material.get("media_info"),
                "reference": MediaReference(kind="material", id=asset.material["id"]),
            },
            "occurrences": [
                {
                    "occurrence_id": event["id"],
                    "asset_id": asset.material["id"],
                    "external_ref": event.get("external_event_id"),
                    "chat_id": event["origin"].get("conversation_ref", ""),
                    "message_id": event["origin"].get("message_id", ""),
                    "component_path": event["origin"].get("component_path", ""),
                    "source_kind": event["origin"].get("source_kind", ""),
                    "occurred_at": point_seconds(event.get("event_time")),
                    "status": "active",
                    "scope_type": "chat" if event["origin"].get("conversation_ref") else "global",
                    "installation_id": ctx.namespace_id,
                    "user_statement": event.get("statement", ""),
                }
                for event in occurrences
            ],
            "observations": observations,
            "links": links,
            "related_memories": [
                {
                    "memory_id": record.reference.id,
                    "reference": record.reference,
                    "content": record.body.text,
                }
                for record in related
                if isinstance(record.body, MemoryBody)
            ],
        }
    )


async def describe(
    service: AMemorixService, ctx: CallContext, asset: ImageAsset, command: ImageInput
) -> dict[str, JsonValue]:
    event = next(
        (
            event
            for event in encounters(asset, command)
            if not command.occurrence_id or event["id"] == command.occurrence_id
        ),
        None,
    )
    if event is None:
        raise DomainError("source_unavailable", "Image occurrence is unavailable")
    supersedes = None
    if command.supersedes_id:
        async with service.app.authority.read(ctx) as view:
            target = (
                await service.app.references.resolve_in(view, ctx, command.supersedes_id)
            ).reference
            if not isinstance(target, CognitiveTarget):
                raise DomainError("memory_required", "Superseded description requires a Memory")
            supersedes = target.reference
    value = await service.app.description_deliveries.record_external(
        ctx,
        content_hash=command.content_hash or asset.material["content_hash"],
        text=command.text,
        source_kind=command.source_kind,
        confirmation=command.confirm_status,
        details=command.evidence,
        conversation_refs=command.chat_ids
        if "chat_ids" in command.model_fields_set
        else (command.chat_id,)
        if command.chat_id
        else None,
    )
    return await service.app.description_deliveries.adopt_external(
        ctx, value.id, event["id"], supersedes=supersedes
    )


async def image_memory(
    service: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = ImageInput.model_validate(arguments)
    if command.action in {"describe", "observe"} and "source_kind" not in arguments:
        command = command.model_copy(
            update={
                "source_kind": "model_description" if command.action == "describe" else "manual"
            }
        )
    if command.image_base64 is not None:
        if command.action not in {"ingest", "search"} or command.material_id:
            raise DomainError(
                "image_input_ambiguous",
                "Supply image base64 for ingest or search without material_id",
            )
        binary = base64.b64decode(command.image_base64, validate=True)
        mime = MIMEImage(binary, _encoder=encode_noop).get_content_type()
        digest_ = sha256(binary).hexdigest()
        lease = await service.app.content.publish_bytes(binary, mime)
        try:
            async with service.app.authority.write(
                ctx,
                "image-inline:" + digest_,
                operation_kind="media.image.original",
                scope=ctx.namespace_id,
                payload={"hash": digest_, "media_type": mime},
            ) as tx:
                if tx.replayed:
                    identifier = str(tx.replay_result)
                    await service.app.material.read_in(tx, ctx, identifier)
                else:
                    admitted = await service.app.material.admit_blob_in(
                        tx, ctx, lease, BlobDocument(filename="image-" + digest_, media_type=mime)
                    )
                    identifier = tx.result(admitted.material["id"])
            command = command.model_copy(
                update={"material_id": identifier, "content_hash": digest_, "image_base64": None}
            )
        finally:
            await service.app.content.release(lease)
    if command.action in {"link", "unlink"}:
        from .image_links import change_link

        return await change_link(service, ctx, command)
    if command.action == "claim_description_compensations":
        return await service.app.description_deliveries.claim(
            ctx, limit=command.limit, operation_id=command.operation_id
        )
    if command.action == "complete_description_compensation":
        if not command.occurrence_id:
            raise DomainError("occurrence_required", "Select the claimed actual image occurrence")
        complete = await service.app.description_deliveries.complete(
            ctx,
            observation_id=command.occurrence_id,
            description_hash=command.description_hash,
            lease_token=command.lease_token,
            success=command.success,
            error=command.error,
            operation_id=command.operation_id,
        )
        return {"success": complete}
    if command.action == "describe":
        if not command.content_hash:
            current = await inventory(service, ctx)
            matches = [asset for asset in current if selected(asset, command)]
            if len(matches) != 1:
                raise DomainError(
                    "image_not_found", "Supply a content hash or an available original"
                )
            command = command.model_copy(
                update={"content_hash": matches[0].material["content_hash"]}
            )
        value = await service.app.description_deliveries.record_external(
            ctx,
            content_hash=command.content_hash or "",
            text=command.text,
            source_kind=command.source_kind,
            confirmation=command.confirm_status,
            details=command.evidence,
            conversation_refs=command.chat_ids
            if "chat_ids" in command.model_fields_set
            else (command.chat_id,)
            if command.chat_id
            else None,
        )
        # Capture precedes this read. A concurrent contact either appears here,
        # or its post-commit binding reads this now-committed source report.
        current = await inventory(service, ctx, legacy=len(command.content_hash or "") == 32)
        events = [
            (asset, event)
            for asset in current
            if selected(asset, command)
            for event in encounters(asset, command)
            if not command.occurrence_id or command.occurrence_id == event["id"]
        ]
        if command.supersedes_id and len(events) != 1:
            raise DomainError(
                "occurrence_required", "Supersession requires one exact encountered occurrence"
            )
        results: list[dict[str, JsonValue]] = []
        for asset, event in events:
            results.append(
                await describe(
                    service, ctx, asset, command.model_copy(update={"occurrence_id": event["id"]})
                )
            )
        return json_object(
            {
                **(results[0] if results else {}),
                "success": True,
                "description_id": value.id,
                "pending": not bool(events),
                "updated_occurrences": len(results),
                "compensation_targets": [
                    {
                        "chat_id": event["origin"].get("conversation_ref", ""),
                        "message_id": event["origin"].get("message_id", ""),
                        "occurred_at": point_seconds(event.get("event_time")),
                    }
                    for (_, event), result in zip(events, results, strict=True)
                    if result.get("delivery_id")
                ],
                "observations": results,
            }
        )
    if command.action == "ingest":
        if (command.scope_type == "chat" and not (command.chat_id or command.session_id)) or (
            command.scope_type == "global" and (command.chat_id or command.session_id)
        ):
            raise DomainError(
                "image_scope_invalid",
                "Chat occurrences require their actual conversation; global occurrences omit it",
            )
        if not command.material_id or not command.external_ref:
            raise DomainError(
                "image_source_required",
                "Image ingest requires an uploaded material_id and actual external_ref",
            )
        material = await service.app.material.read(ctx, command.material_id)
        if not material["media_type"].startswith("image/"):
            raise DomainError("image_required", "Material is not an image")
        contact = await service.app.imports.contact(
            ctx,
            ContactMaterial(
                operation_id=command.operation_id,
                source=MediaReference(kind="material", id=command.material_id),
                external_event_id=command.external_ref,
                event_time=point(command.occurred_at),
                origin={
                    "conversation_ref": command.chat_id or command.session_id,
                    "message_id": command.message_id,
                    "component_path": command.component_path,
                    "source_kind": command.source_kind,
                },
                statement=command.user_statement or None,
                form_long_term=command.form_long_term,
            ),
        )
        descriptions: list[dict[str, JsonValue]] = []
        for value in await service.app.description_deliveries.matching_external(
            ctx, contact.observation_id
        ):
            descriptions.append(
                await service.app.description_deliveries.adopt_external(
                    ctx, value.id, contact.observation_id
                )
            )
        features = NamespaceFeatures.model_validate(
            (await service.app.configuration.effective(ctx, "namespace.features"))["value"]
        )
        profile = await service.app.models.prepare_embedding(ctx)
        configured = any(
            route.model and route.base_url for route in (profile.route, *profile.fallbacks)
        )
        projection = None
        ready = False
        if configured:
            projection = await service.app.vector_projection.submit(
                ctx,
                ProjectVectors(
                    operation_id="image-projection:"
                    + material["id"]
                    + ":"
                    + profile.resource_key(),
                    targets=(contact.source,),
                ),
            )
            if not features.allow_metadata_only_write:
                result = ProjectionResult.model_validate(
                    await service.app.execution.wait(ctx, projection.job_id)
                )
                ready = contact.source.exact() in result.references
                if not ready:
                    raise DomainError(
                        "image_projection_incomplete",
                        "Native projection did not complete the selected image",
                    )
            elif (await service.app.execution.status(ctx, projection.job_id)).status == "SUCCESS":
                result = ProjectionResult.model_validate(
                    await service.app.execution.wait(ctx, projection.job_id)
                )
                ready = contact.source.exact() in result.references
        elif not features.allow_metadata_only_write:
            raise DomainError(
                "model_unavailable", "Image completion requires configured native embeddings"
            )
        return json_object(
            {
                "success": True,
                "asset_id": material["id"],
                "content_hash": material["content_hash"],
                "occurrence_id": contact.observation_id,
                "stored_ids": [reference.id for reference in contact.cognition],
                "contact": contact,
                "descriptions": descriptions,
                "projection": projection,
                "queued": projection is not None,
                "retrieval_ready": ready,
            }
        )
    if command.action in {"delete_occurrence", "trash", "restore", "purge"}:
        events = (
            [
                event
                for event in await service.app.sources.list(ctx)
                if event["id"] == command.occurrence_id
            ]
            if command.occurrence_id
            else [
                event
                for event in await service.app.sources.list(ctx)
                if event["material_id"] == (command.asset_id or command.material_id)
            ]
        )
        if not events:
            raise DomainError("source_unavailable", "Image source is unavailable")
        change = SourceCommand(
            operation_id=command.operation_id,
            observation_ids=tuple(str(event["id"]) for event in events),
            reason=command.reason,
        )
        owner = service.app.sources
        result = await (
            owner.restore(ctx, change)
            if command.action == "restore"
            else owner.purge(ctx, change)
            if command.action == "purge"
            else owner.trash(ctx, change)
        )
        return json_object({"success": True, "result": result})
    assets = await inventory(
        service, ctx, legacy=command.content_hash is not None and len(command.content_hash) == 32
    )
    if command.action in {"jobs", "retry_failed_jobs", "process_jobs"}:
        from .image_jobs import image_jobs

        return await image_jobs(service, ctx, command, assets)
    if command.action in {"observe", "get"}:
        matched = [asset for asset in assets if selected(asset, command)]
        if len(matched) != 1:
            raise DomainError(
                "image_not_found" if not matched else "image_ambiguous",
                "Select one authorized image asset",
            )
        return (
            await detail(service, ctx, matched[0], command)
            if command.action == "get"
            else await describe(service, ctx, matched[0], command)
        )
    if command.action == "list_chats":
        chats: dict[str, set[str]] = {}
        for asset in assets:
            for event in encounters(asset, command):
                chats.setdefault(event["origin"].get("conversation_ref", ""), set()).add(
                    asset.material["id"]
                )
        return json_object(
            {
                "success": True,
                "items": [
                    {"chat_id": chat, "chat_name": chat, "asset_count": len(ids)}
                    for chat, ids in sorted(chats.items())
                ],
            }
        )
    if command.action in {"list", "status"}:
        filtered = [
            asset
            for asset in assets
            if encounters(asset, command) or not command.chat_id and command.chat_ids is None
        ]
        if command.action == "status":
            from .image_jobs import job_groups

            profile = await service.app.models.prepare_embedding(ctx)
            configured = bool(profile.route.base_url and profile.route.model)
            targets = tuple(
                MediaReference(
                    kind="material", id=asset.material["id"], observation_id=asset.events[0]["id"]
                )
                for asset in filtered
            )
            missing = (
                await service.app.vector_projection.missing(ctx, targets, profile=profile)
                if configured
                else targets
            )
            pending_jobs = failed_jobs = 0
            for kind, identifiers in (await job_groups(service, ctx, tuple(filtered))).items():
                summary = await service.app.execution.summarize_jobs(
                    ctx, kind=kind, workflow_ids=identifiers
                )
                counts = TypeAdapter(dict[str, int]).validate_python(summary["counts"])
                pending_jobs += sum(
                    counts.get(state, 0) for state in ("PENDING", "ENQUEUED", "DELAYED")
                )
                failed_jobs += sum(
                    counts.get(state, 0) for state in ("ERROR", "MAX_RECOVERY_ATTEMPTS_EXCEEDED")
                )
            events = {event["id"] for asset in filtered for event in asset.events}
            records = await service.records(ctx, ("memory", "entity", "association", "fact_claim"))
            deliveries = [
                item
                for item in await service.app.description_deliveries.items(ctx)
                if item["occurrence_id"] in events
            ]
            stats = {
                "asset_count": len(filtered),
                "occurrence_count": sum(len(encounters(asset, command)) for asset in filtered),
                "storage_bytes": sum(asset.material["byte_len"] for asset in filtered),
                "observation_count": sum(
                    isinstance(record.body, MemoryBody)
                    and bool(events.intersection(record.source_roots))
                    for record in records
                ),
                "link_count": sum(
                    link.observation_id in events and link.role.startswith("media.link:")
                    for record in records
                    for link in record.evidence
                ),
                "ready_vector_count": len(targets) - len(missing),
                "pending_job_count": pending_jobs,
                "failed_job_count": failed_jobs,
                "pending_description_count": sum(
                    item["status"] in {"pending", "running"} for item in deliveries
                ),
                "failed_description_count": sum(item["status"] == "failed" for item in deliveries),
                "pending_unbound_description_count": await service.app.description_deliveries.pending_external_count(
                    ctx
                ),
            }
            return json_object(
                {
                    "success": True,
                    **stats,
                    "stats": stats,
                    "status": "unavailable"
                    if not configured
                    else "indexing"
                    if missing
                    else "ready",
                    "dimension": profile.route.dimensions or 0,
                    "fingerprint": {"space": profile.resource_key(), "model": profile.route.model}
                    if configured
                    else {},
                    "index_progress": {
                        "total": len(targets),
                        "ready": len(targets) - len(missing),
                        "remaining": len(missing),
                    },
                    "native_retrieval": await service.app.retrieval.status(ctx),
                }
            )
        items = [
            await detail(service, ctx, asset, command)
            for asset in filtered[command.offset : command.offset + command.limit]
        ]
        items = [TypeAdapter(dict[str, JsonValue]).validate_python(item["asset"]) for item in items]
        return json_object(
            {
                "success": True,
                "items": items,
                "total": len(filtered),
                "offset": command.offset,
                "limit": command.limit,
            }
        )
    if command.action == "search":
        scores: dict[str, tuple[float, str]] = {
            asset.material["id"]: (1.0, "exact_hash")
            for asset in assets
            if selected(asset, command)
        }
        matched = next((asset for asset in assets if selected(asset, command)), None)
        query_media = (
            MediaReference(kind="material", id=command.material_id)
            if command.material_id
            else MediaReference(kind="material", id=matched.material["id"])
            if matched
            else None
        )
        if query_media is not None:
            await service.app.material.read(ctx, query_media.id)
        degraded: list[str] = []
        if query_media is not None or command.text:
            try:
                result = await service.app.retrieval.query(
                    ctx,
                    QueryRequest(
                        text=command.text,
                        mode="dense" if query_media else "hybrid",
                        domains=("media", "cognition"),
                        kinds=("memory",),
                        media=(query_media,) if query_media else (),
                        limit=command.candidate_limit,
                        capabilities=QueryCapabilities(semantic="optional"),
                    ),
                )
                for hit in result.hits:
                    roots = set(hit.object.source_roots)
                    for asset in assets:
                        if roots.intersection(event["id"] for event in asset.events):
                            scores.setdefault(asset.material["id"], (hit.score, "semantic"))
                if result.execution:
                    degraded.extend(result.execution.reasons)
            except DomainError as error:
                if error.code not in {
                    "model_unavailable",
                    "capability_unavailable",
                    "embedding_unavailable",
                }:
                    raise
                degraded.append(error.code)
        hits: list[dict[str, JsonValue]] = []
        for asset in assets:
            score = scores.get(asset.material["id"])
            if (
                score is None
                or not encounters(asset, command)
                or command.similarity_threshold is not None
                and score[0] < command.similarity_threshold
            ):
                continue
            item = await detail(service, ctx, asset, command)
            hits.append(
                {
                    **item,
                    "asset_id": asset.material["id"],
                    "content_hash": asset.material["content_hash"],
                    "similarity": score[0],
                    "match_kind": score[1],
                }
            )
        hits.sort(key=lambda hit: float(str(hit["similarity"])), reverse=True)
        return json_object(
            {
                "success": True,
                "hits": hits[: command.candidate_limit],
                "degraded": bool(degraded),
                "degraded_reasons": degraded,
            }
        )
    raise DomainError(
        "capability_unavailable", "Image operation is unavailable", action=command.action
    )
