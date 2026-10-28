from pathlib import Path, PureWindowsPath
from typing import TYPE_CHECKING, Any
from uuid import uuid4

import anyio

from ..errors import NousError

if TYPE_CHECKING:
    from .client import AMemorixAdapter


def transfer_root(adapter: "AMemorixAdapter") -> Path:
    if adapter.transfer_root is None:
        raise ValueError("Bundle file transfer requires an explicit transfer_root")
    return adapter.transfer_root


def local_upload(adapter: "AMemorixAdapter", raw_path: str) -> Path:
    root = transfer_root(adapter)
    path = Path(raw_path)
    path = (path if path.is_absolute() else root / path).resolve(strict=True)
    if not path.is_relative_to(root) or not path.is_file():
        raise ValueError("Bundle upload must be a file within transfer_root")
    return path


def file_name(value: Any) -> str:
    if (
        not isinstance(value, str)
        or not value
        or value in {".", ".."}
        or any(char in value for char in '/\\:\x00')
        or Path(value).name != value
        or PureWindowsPath(value).is_reserved()
        or value.endswith((".", " "))
    ):
        raise ValueError("Bundle file_name must be a plain file name")
    return value


async def bundle_admin(adapter: "AMemorixAdapter", *, action: str, **kwargs: Any) -> dict:
    action = action.strip().lower()
    arguments = {"action": action, **kwargs}
    if action in {"import", "inspect"} and "path" in arguments:
        if "material_id" in arguments:
            raise ValueError("Supply path or material_id, not both")
        path = local_upload(adapter, arguments.pop("path"))
        receipt = await adapter.client.upload_file(
            adapter.namespace_id,
            path,
            owner_subject_id=adapter.subject_id,
            media_type="application/vnd.a-memorix.bundle+zip",
            source=path.name,
        )
        arguments["material_id"] = receipt["result"]["id"]
    if action == "resolve_file":
        transfer_root(adapter)
        arguments["file_name"] = file_name(arguments.get("file_name"))
    result = await adapter.call("memory_bundle_admin", arguments)
    if action != "resolve_file" or not result.get("success"):
        return result
    name = file_name(result.get("file_name"))
    material_id = result.get("material_id")
    if not isinstance(material_id, str) or not material_id:
        raise NousError({"code": "Internal", "message": "Bundle resolution has no material_id"})
    root = transfer_root(adapter)
    downloads = (root / "downloads").resolve()
    if not downloads.is_relative_to(root):
        raise ValueError("Bundle download directory escapes transfer_root")
    await anyio.Path(downloads).mkdir(parents=True, exist_ok=True)
    directory = downloads / uuid4().hex
    await anyio.Path(directory).mkdir(exist_ok=False)
    destination = directory / name
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
