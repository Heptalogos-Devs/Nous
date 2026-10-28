from typing import TYPE_CHECKING

import anyio
from pydantic import JsonValue

from ..errors import NousError
from ..files import file_name
from ..types import JsonObject

if TYPE_CHECKING:
    from .client import AMemorixAdapter


async def bundle_admin(
    adapter: "AMemorixAdapter", *, action: str, **kwargs: JsonValue
) -> JsonObject:
    arguments = {"action": action.strip().lower(), **kwargs}
    if action in {"import", "inspect"} and "path" in arguments:
        if "material_id" in arguments:
            raise ValueError("Supply path or material_id, not both")
        path = arguments.pop("path")
        if not isinstance(path, str):
            raise ValueError("Bundle path must be text")
        local = adapter.managed_files.local_file(path)
        receipt = await adapter.client.upload_file(
            adapter.namespace_id,
            local,
            owner_subject_id=adapter.subject_id,
            media_type="application/vnd.nous-wave.bundle+zip",
            source=local.name,
        )
        arguments["material_id"] = receipt.result.id
    if action == "resolve_file":
        arguments["file_name"] = file_name(arguments.get("file_name"))
    result = await adapter.call("memory_bundle_admin", arguments)
    if action != "resolve_file" or not result.get("success"):
        return result
    name = file_name(result.get("file_name"))
    material_id = result.get("material_id")
    if not isinstance(material_id, str):
        raise NousError(
            {"code": "invalid_response", "message": "Bundle resolution has no material_id"}
        )
    destination = await adapter.managed_files.download_destination(name)
    try:
        downloaded = await adapter.client.download_material(
            material_id,
            destination,
            namespace_id=adapter.namespace_id,
            subject_id=adapter.subject_id,
        )
        expected = result.get("size")
        if expected is not None and downloaded.byte_len != expected:
            raise NousError(
                {"code": "invalid_response", "message": "Bundle download size mismatch"}
            )
    except BaseException:
        with anyio.CancelScope(shield=True):
            await anyio.Path(destination).unlink(missing_ok=True)
            await anyio.Path(destination.parent).rmdir()
        raise
    return {**result, "path": str(destination)}
