"""A-Memorix uses the same caller, namespace, source and domain boundaries."""

from fastapi import APIRouter, Request
from nous_sdk.amemorix.contracts import AMemorixRequest

from .security import application, context

router = APIRouter(prefix="/api/v1/integrations")


@router.post("/amemorix")
async def amemorix(request: Request, command: AMemorixRequest):
    if (
        command.operation.tool == "memory_runtime_admin"
        and command.operation.arguments.get("action") == "delete_vector_space"
    ):
        return await application(request).amemorix.execute(
            await context(request, command.namespace_id, capability="manage"), command.operation
        )
    if command.operation.tool == "memory_tuning_admin":
        capability = (
            "manage"
            if command.operation.arguments.get("action")
            in {"create_task", "cancel", "apply_profile", "apply_best", "rollback_profile"}
            else "read"
        )
        return await application(request).amemorix.execute(
            await context(request, command.namespace_id,  capability=capability),
            command.operation,
        )
    if command.operation.tool == "image_memory" and command.operation.arguments.get("action") in {
        "delete_occurrence",
        "trash",
        "restore",
        "purge",
        "link",
        "unlink",
        "retry_failed_jobs",
        "process_jobs",
    }:
        return await application(request).amemorix.execute(
            await context(request, command.namespace_id,  capability="manage"),
            command.operation,
        )
    if command.operation.tool in {
        "memory_correction_admin",
        "memory_fuzzy_modify_admin",
        "memory_v5_admin",
    } or (
        command.operation.tool == "memory_feedback_admin"
        and command.operation.arguments.get("action") == "rollback"
    ):
        return await application(request).amemorix.execute(
            await context(request, command.namespace_id,  capability="manage"),
            command.operation,
        )
    read_tools = {"search_memory", "get_person_profile", "memory_source", "memory_stats"}
    read_actions = {
        "list",
        "get",
        "get_chunks",
        "chunks",
        "resolve_file",
        "preview",
        "settings",
        "get_settings",
        "get_guide",
        "list_operations",
        "recycle_bin",
        "get_operation",
        "inspect",
        "get_config",
        "list_vector_spaces",
    }
    read_preview = (
        (
            command.operation.tool == "image_memory"
            and command.operation.arguments.get("action")
            in {"search", "list", "list_chats", "get", "status", "jobs"}
            and not command.operation.arguments.get("image_base64")
        )
        or (
            command.operation.tool == "memory_episode_admin"
            and command.operation.arguments.get("action") in {"query", "list", "get", "status"}
        )
        or (
            command.operation.tool == "memory_profile_admin"
            and command.operation.arguments.get("action")
            in {"list", "query", "evidence", "get_aliases", "status"}
        )
        or (
            command.operation.tool == "memory_graph_admin"
            and command.operation.arguments.get("action")
            in {"search", "get_graph", "node_detail", "edge_detail"}
        )
        or (
            command.operation.tool == "memory_bundle_admin"
            and command.operation.arguments.get("action") == "export"
            and command.operation.arguments.get("preview") is True
        )
        or (
            command.operation.tool == "memory_delete_admin"
            and command.operation.arguments.get("action") == "execute"
            and command.operation.arguments.get("dry_run") is True
        )
    )
    capability = (
        "read"
        if command.operation.tool in read_tools
        or command.operation.arguments.get("action") in read_actions
        or read_preview
        else "write"
    )
    if command.operation.tool in {
        "get_person_profile",
        "memory_profile_admin",
    } and command.operation.arguments.get("force_refresh"):
        capability = "write"
    return await application(request).amemorix.execute(
        await context(request, command.namespace_id,  capability=capability),
        command.operation,
    )
