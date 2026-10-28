"""A-Memorix uses the same caller, namespace, source and domain boundaries."""

from fastapi import APIRouter, Request
from nous_wave.amemorix.contracts import AMemorixRequest

from nous.integrations.amemorix import AMemorixService

from .security import application, context

router = APIRouter(prefix="/api/integrations")


@router.post("/amemorix")
async def amemorix(request: Request, command: AMemorixRequest):
    read_tools = {"search_memory", "get_person_profile", "memory_source", "memory_stats"}
    read_actions = {
        "list",
        "get",
        "preview",
        "settings",
        "get_settings",
        "get_guide",
        "list_operations",
        "get_operation",
        "inspect",
        "get_config",
        "list_vector_spaces",
    }
    capability = (
        "manage"
        if command.operation.tool == "memory_runtime_admin"
        and command.operation.arguments.get("action") == "set_config"
        else "read"
        if command.operation.tool in read_tools
        or command.operation.arguments.get("action") in read_actions
        else "write"
    )
    return await AMemorixService(application(request)).execute(
        await context(request, command.namespace_id, command.subject_id, capability=capability),
        command.operation,
    )
