from typing import TYPE_CHECKING, Any

import anyio

from ..errors import NousError
from ..files import file_name

if TYPE_CHECKING:
    from .client import AMemorixAdapter


async def bundle_admin(adapter: "AMemorixAdapter", *, action: str, **kwargs: Any) -> dict:
    action = action.strip().lower()
    arguments = {"action": action, **kwargs}
    if action in {"import", "inspect"} and "path" in arguments:
        if "material_id" in arguments:
            raise ValueError("Supply path or material_id, not both")
        path = adapter.managed_files.local_file(arguments.pop("path"))
        receipt = await adapter.client.upload_file(
            adapter.namespace_id,
            path,
            owner_subject_id=adapter.subject_id,
            media_type="application/vnd.a-memorix.bundle+zip",
            source=path.name,
        )
        arguments["material_id"] = receipt["result"]["id"]
    if action == "resolve_file":
        _ = adapter.managed_files
        arguments["file_name"] = file_name(arguments.get("file_name"))
    result = await adapter.call("memory_bundle_admin", arguments)
    if action != "resolve_file" or not result.get("success"):
        return result
    name = file_name(result.get("file_name"))
    material_id = result.get("material_id")
    if not isinstance(material_id, str) or not material_id:
        raise NousError({"code": "Internal", "message": "Bundle resolution has no material_id"})
    destination = await adapter.managed_files.download_destination(name)
    directory = destination.parent
    try:
        downloaded = await adapter.client.download_material(material_id, destination)
        expected_size = result.get("size")
        if expected_size is not None and downloaded["byte_len"] != expected_size:
            raise NousError({"code": "DependencyUnavailable", "message": "Bundle download size mismatch"})
    except BaseException:
        with anyio.CancelScope(shield=True):
            await anyio.Path(destination).unlink(missing_ok=True)
            await anyio.Path(directory).rmdir()
        raise
    return {**result, "path": str(destination)}
