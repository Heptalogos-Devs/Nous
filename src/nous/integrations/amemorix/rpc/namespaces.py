"""Namespace lifecycle and status come from native identity/configuration owners."""

from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.identity import ChangeNamespace, IdentityDirectoryQuery, Namespace
from nous_sdk.types import JsonObject
from pydantic import TypeAdapter

from nous.integrations.amemorix.namespace_config import (
    create_namespace_configured,
    read_namespace_config,
    update_namespace_config,
)

from .context import RPCContext, integer_field, object_field, page_size, required_text, text_field


async def namespace_info(rpc: RPCContext, value: Namespace) -> JsonObject:
    ctx = await rpc.scope(value.id)
    configuration = await read_namespace_config(rpc.app, ctx)
    result: JsonObject = {
        "namespace_id": value.id,
        "status": "NAMESPACE_STATUS_QUARANTINED"
        if value.deleted
        else "NAMESPACE_STATUS_ACTIVE"
        if value.enabled
        else "NAMESPACE_STATUS_INACTIVE",
        "version": value.revision,
        **{key: configuration[key] for key in ("config", "quota", "config_version")},
    }
    for key in ("created_at", "updated_at"):
        timestamp = getattr(value, key)
        if isinstance(timestamp, str):
            result[key] = timestamp
    return result


def provider_status(status: JsonObject, role: str) -> JsonObject:
    roles = object_field(status, "roles")
    assigned = object_field(roles, role)
    routes = TypeAdapter(tuple[JsonObject, ...]).validate_python(assigned.get("routes", []))
    primary = routes[0] if routes else {}
    error = object_field(primary, "unavailable") if primary.get("unavailable") else {}
    value: JsonObject = {
        "configured": primary.get("model_configured", False),
        "available": primary.get("credential_ready", False),
        "provider": primary.get("provider", ""),
        "model": primary.get("model", ""),
        "last_error": error.get("message", ""),
    }
    if isinstance(primary.get("dimensions"), int):
        value["dimension"] = primary["dimensions"]
    return value


async def capabilities(rpc: RPCContext, ctx: CallContext) -> JsonObject:
    config = await read_namespace_config(rpc.app, ctx)
    features = object_field(object_field(config, "config"), "features")
    status = TypeAdapter(JsonObject).validate_python(await rpc.app.models.status(ctx))
    embedding, llm = provider_status(status, "embedding"), provider_status(status, "formation")
    sparse = features.get("sparse_retrieval", True) is True
    dense = embedding["available"] is True
    active = await rpc.app.identity.namespace(ctx, purpose="management")
    usable = active.enabled and not active.deleted and active.memory_subject_id is not None
    flags: JsonObject = {
        "ingest": usable and (dense or features.get("allow_metadata_only_write") is True),
        "search": usable and (sparse or dense),
        "sparse_retrieval": usable and sparse,
        "embedding": dense,
        "llm": llm["available"],
        "episodes": usable and features.get("episodes", True) is True,
        "person_profiles": usable and features.get("person_profiles", True) is True,
        "relation_vectors": dense and features.get("relation_vectors", True) is True,
    }
    unavailable = [name for name, ready in flags.items() if ready is False]
    modes: list[str] = []
    if usable:
        if sparse or dense:
            modes.extend(("search", "time"))
        if dense:
            modes.extend(("hybrid", "aggregate"))
        if flags["episodes"] and (sparse or dense):
            modes.append("episode")
    return TypeAdapter(JsonObject).validate_python(
        {
            "namespace_id": ctx.namespace_id,
            "config_version": config["config_version"],
            "capabilities": flags,
            "search_modes": modes,
            "degraded": bool(unavailable),
            "unavailable": unavailable,
            "operations": [
                "get_memory",
                "delete_memory",
                "delete_by_source",
                "jobs",
                "backup",
                "restore",
            ]
            + (["ingest_text", "batch_ingest_text"] if flags["ingest"] else [])
            + (["search_memory"] if flags["search"] else []),
        }
    )


async def execute(method: str, values: JsonObject, rpc: RPCContext) -> JsonObject:
    if method == "ListNamespaces":
        rpc.administrator()
        ctx = await rpc.scope("instance", "manage")
        page = await rpc.app.identity.namespace_page(
            ctx,
            IdentityDirectoryQuery(
                purpose="management",
                limit=page_size(values),
                cursor=text_field(values, "page_token") or None,
            ),
        )
        return {
            "namespaces": [await namespace_info(rpc, item) for item in page.items],
            "next_page_token": page.next_cursor or "",
        }
    namespace_id = required_text(values, "namespace_id")
    readonly = method in {"GetNamespace", "GetNamespaceCapabilities", "GetNamespaceHealth"}
    if not readonly:
        rpc.administrator()
    ctx = await rpc.scope(namespace_id, "read" if readonly else "manage")
    if method == "CreateNamespace":
        value = await create_namespace_configured(
            rpc.app,
            ctx,
            namespace_id,
            object_field(values, "config"),
            object_field(values, "quota") if "quota" in values else None,
            rpc.operation_id(values, method),
        )
        return {"namespace": await namespace_info(rpc, value)}
    value = await rpc.app.identity.namespace(ctx, purpose="management")
    if method == "GetNamespace":
        return {"namespace": await namespace_info(rpc, value)}
    if method == "UpdateNamespaceConfig":
        await update_namespace_config(
            rpc.app,
            ctx,
            object_field(values, "config"),
            rpc.operation_id(values, method),
            integer_field(values, "expected_config_version")
            if "expected_config_version" in values
            else None,
        )
        return {"namespace": await namespace_info(rpc, value)}
    actions = {
        "DisableNamespace": "disable",
        "EnableNamespace": "enable",
        "DeleteNamespace": "trash",
        "RestoreNamespace": "restore",
    }
    if method in actions:
        value = await rpc.app.identity.change_namespace(
            ctx,
            ChangeNamespace.model_validate(
                {"action": actions[method], "operation_id": rpc.operation_id(values, method)}
            ),
        )
        return {"namespace": await namespace_info(rpc, value)}
    if method == "PurgeNamespace":
        from nous_sdk.contracts.identity import PurgeNamespace

        await rpc.app.namespace_data.purge_namespace(
            ctx, PurgeNamespace(operation_id=rpc.operation_id(values, method))
        )
        return {}
    if method == "GetNamespaceCapabilities":
        return {"capabilities": await capabilities(rpc, ctx)}
    if method == "GetNamespaceHealth":
        config = await read_namespace_config(rpc.app, ctx)
        status = TypeAdapter(JsonObject).validate_python(await rpc.app.models.status(ctx))
        embedding, llm = provider_status(status, "embedding"), provider_status(status, "formation")
        reasons: list[str] = []
        if not value.enabled or value.deleted:
            reasons.append("namespace_deleted" if value.deleted else "namespace_disabled")
        if not embedding["available"]:
            reasons.append("embedding_unavailable")
        if not llm["available"]:
            reasons.append("llm_unavailable")
        vectors_ready = False
        if value.memory_subject_id and value.enabled and not value.deleted:
            scoped = ctx.model_copy(update={})
            inventory = await rpc.app.retrieval.status(scoped)
            vectors_ready = inventory.configured_space_complete is True
        elif value.memory_subject_id is None:
            reasons.append("memory_subject_unbound")
        usage = object_field(config, "resource_usage")
        return TypeAdapter(JsonObject).validate_python(
            {
                "health": {
                    "namespace": await namespace_info(rpc, value),
                    "healthy": not reasons,
                    "runtime_state": "NAMESPACE_RUNTIME_STATE_DEGRADED"
                    if reasons
                    else "NAMESPACE_RUNTIME_STATE_READY",
                    "resource_usage": {
                        key: usage[key] for key in ("active_requests", "storage_bytes")
                    },
                    "degraded": bool(reasons),
                    "degraded_reasons": reasons,
                    "embedding": embedding,
                    "llm": llm,
                    "paragraph_vector_pool_ready": vectors_ready,
                    "relation_vector_pool_ready": vectors_ready
                    and object_field(object_field(config, "config"), "features").get(
                        "relation_vectors", True
                    )
                    is True,
                }
            }
        )
    raise DomainError("rpc_method_not_found", "Unknown Namespace operation")
