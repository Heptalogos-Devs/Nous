"""A-Memorix uses the same caller, namespace, source and domain boundaries."""

from fastapi import APIRouter, Request
from nous_wave.amemorix.contracts import AMemorixRequest

from nous.integrations.amemorix import AMemorixService

from .security import application, authorize, context

router = APIRouter(prefix="/api/integrations")


@router.post("/amemorix")
async def amemorix(request: Request, command: AMemorixRequest):
    if (
        command.operation.tool == "memory_runtime_admin"
        and command.operation.arguments.get("action") == "set_config"
    ):
        (await authorize(request, capability="manage")).require_instance_management()
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
            command.operation.tool == "memory_episode_admin"
            and command.operation.arguments.get("action") in {"query", "list", "get", "status"}
        )
        or (
            command.operation.tool == "memory_profile_admin"
            and command.operation.arguments.get("action")
            in {"list", "query", "evidence", "get_aliases"}
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
        "manage"
        if command.operation.tool == "memory_runtime_admin"
        and command.operation.arguments.get("action") == "set_config"
        else "read"
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
    return await AMemorixService(application(request)).execute(
        await context(request, command.namespace_id, command.subject_id, capability=capability),
        command.operation,
    )
