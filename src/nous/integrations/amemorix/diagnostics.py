"""Runtime diagnostics compose current native configuration, coverage and durable jobs."""

from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.namespace_policy import NamespaceFeatures
from pydantic import JsonValue

from nous.identity.access import require_namespace_in
from nous.platform.configuration.namespace_policy import policy_in

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


async def diagnostics(service: AMemorixService, ctx: CallContext) -> dict[str, JsonValue]:
    app = service.app
    observed = await app.diagnostics.read(ctx)
    inventory = await app.vector_spaces.catalog(ctx)
    last_check = await app.vector_spaces.last_check(ctx)
    try:
        backfill = await app.vector_spaces.backfill_status(ctx)
    except DomainError as error:
        if error.code != "model_unavailable":
            raise
        backfill = {"pending": inventory.eligible_references, "running": 0, "failed": 0, "done": 0}
    integration = await service.policy(ctx)
    retrieval_configuration = await app.configuration.effective(ctx, "retrieval")
    async with app.authority.read(ctx) as view:
        subject = await require_namespace_in(view, ctx)
        features = await policy_in(view, ctx, "namespace.features", NamespaceFeatures)
    roles = observed.models.get("roles")
    embedding = roles.get("embedding") if isinstance(roles, dict) else None
    active_route = embedding.get("active_route") if isinstance(embedding, dict) else None
    routes = embedding.get("routes") if isinstance(embedding, dict) else None
    credential_ready = any(isinstance(route, dict) and route.get("credential_ready") is True for route in routes) if isinstance(routes, list) else False
    primary = next((space for space in inventory.spaces if active_route in space.route_ids), None)
    fingerprint = {"hash": primary.space_id, "model": primary.model, "dimension": primary.dimensions} if primary else None
    stored = [space for space in inventory.spaces if space.vector_count]
    spaces = [{"space_id": space.space_id, "space": space, "embedding_fingerprint": {
        "hash": space.space_id, "model": space.model, "dimension": space.dimensions},
        "state": "saved" if space.vector_count else "missing", "can_delete": space.can_delete,
        "delete_scope": space.delete_scope, "vector_count": space.vector_count,
        "payload_bytes": space.payload_bytes, "missing_references": space.missing_references} for space in inventory.spaces]
    channels = ["metadata"]
    if features.sparse_retrieval:
        channels.append("sparse")
    if stored:
        channels.append("vector_read")
    if inventory.configured and credential_ready:
        channels.append("embedding")
    reason = "not_configured" if not inventory.configured else "credential_unavailable" if not credential_ready else "missing_current_space" if inventory.configured_space_complete is False else None
    state = "ready" if inventory.configured_space_complete else "partial" if stored else "empty"
    actions = ["get_config", "list_vector_spaces", "save", "self_check", "refresh_self_check", "recover_embedding", "rebuild_all_vectors", "paragraph_backfill_once", "set_auto_save", "delete_vector_space", "projection_progress", "retry_projection"]
    return json_object({
        "success": True,
        "config": {**integration.model_dump(mode="json"), "retrieval": retrieval_configuration["value"], "namespace_features": features},
        "integration": integration, "models": observed.models, "retrieval": observed.retrieval,
        "imports": observed.imports, "cognition": observed.cognition, "tasks": observed.tasks,
        "tasks_recent": observed.tasks_recent, "model_calls": observed.model_calls, "model_usage": observed.model_usage,
        "items": spaces, "spaces": spaces, "vector_spaces": spaces,
        "vector_catalog": inventory,
        "watermarks": {"retrieval": observed.retrieval.authority_commit_seq, "imports": observed.imports.authority_commit_seq, "spaces": inventory.authority_commit_seq},
        "state": state, "embedding_dimension": primary.dimensions if primary else None,
        "stored_vector_dimension": stored[0].dimensions if len(stored) == 1 else None,
        "embedding_fingerprint": fingerprint,
        "stored_embedding_fingerprint": {"hash": stored[0].space_id, "model": stored[0].model, "dimension": stored[0].dimensions} if len(stored) == 1 else None,
        "embedding_fingerprint_status": "configured" if fingerprint else "not_configured",
        "vector_rebuild_required": inventory.configured_space_complete is False,
        "vector_rebuild_message": reason or "", "auto_save": True,
        "persistence": await app.vector_spaces.persistence(ctx),
        "relation_vectors_enabled": features.relation_vectors,
        "vector_pools": {"layout": "model_spaces", "spaces": len(stored), "current_space": primary.space_id if primary else None},
        "vector_pools_ready": bool(stored), "vector_pools_effective_mode": "model_spaces",
        "memory_enabled": "memory" in subject.enabled_features, "runtime_ready": True,
        "retrieval_ready": features.sparse_retrieval or bool(stored),
        "available_channels": channels,
        "unavailable_channels": [name for name in ("sparse", "vector_read", "embedding") if name not in channels],
        "provider_probed": False, "provider_ready": None,
        "last_self_check": last_check,
        "embedding_last_check": last_check.checked_at.coordinate / 1000 if last_check else None,
        "provider_ready_at_last_check": last_check.provider_ready if last_check else None,
        "embedding_degraded": reason is not None, "embedding_degraded_reason": reason,
        "paragraph_vector_backfill_pending": backfill["pending"],
        "paragraph_vector_backfill_running": backfill["running"],
        "paragraph_vector_backfill_failed": backfill["failed"],
        "paragraph_vector_backfill_done": backfill["done"],
        "backfill_count_scope": "current_space_memory_references",
        "available_actions": actions,
    })
