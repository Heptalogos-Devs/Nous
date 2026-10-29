"""Bundle host actions use domain archives and managed immutable originals."""

from hashlib import sha256
from typing import TYPE_CHECKING, Literal
from uuid import uuid4

from nous_wave.contracts import BlobDocument, BundleRequest, CallContext, DomainError
from nous_wave.contracts.bundles import BundlePlacement
from pydantic import JsonValue

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


class BundleInput(BundleRequest):
    action: str
    material_id: str | None = None
    file_name: str | None = None
    installation_id: str | None = None
    operation_id: str | None = None
    preview: bool = False
    mode: Literal["merge", "restore"] = "merge"
    placement: BundlePlacement | None = None


async def bundle_admin(
    service: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = BundleInput.model_validate(arguments)
    if command.action == "get":
        if command.installation_id is None:
            raise DomainError("installation_required", "Select a bundle installation")
        return json_object(
            {
                "success": True,
                "installation": await service.app.bundles.get_installation(
                    ctx, command.installation_id
                ),
            }
        )
    if command.action == "list":
        return json_object(
            {"success": True, "installations": await service.app.bundles.list_installs(ctx)}
        )
    if command.action == "uninstall":
        if command.installation_id is None:
            raise DomainError("installation_required", "Select a bundle installation")
        return json_object(
            {
                "success": True,
                "installation": await service.app.bundles.uninstall(
                    ctx, command.installation_id, command.operation_id or str(uuid4())
                ),
            }
        )
    if command.action == "export":
        operation_id = command.operation_id or str(uuid4())
        if not command.preview:
            receipt = await service.app.authority.receipt(
                ctx,
                operation_id,
                operation_kind="bundle.export.material",
                scope=ctx.require_subject(),
                payload=command,
            )
            if receipt is not None:
                saved = json_object(receipt.result)
                identity = saved.get("material_id")
                if not isinstance(identity, str):
                    raise DomainError("bundle_material_missing", "Export receipt has no material")
                await service.app.material.read(ctx, identity)
                return saved
        archive = await service.app.bundles.export_archive(
            ctx,
            BundleRequest(
                package=command.package,
                selector=command.selector,
                content_level=command.content_level,
                format=command.format,
                assets=command.assets,
                references=command.references,
            ),
        )
        data, required_source_ids, summary = (
            archive.data,
            archive.required_source_ids,
            archive.summary,
        )
        if command.preview:
            return json_object({"success": True, "preview": True, "counts": summary})
        lease = await service.app.content.publish_bytes(
            data, "application/vnd.nous-wave.bundle+zip"
        )
        filename = "nous-" + sha256(data).hexdigest()[:16] + ".zip"
        try:
            async with service.app.authority.write(
                ctx,
                operation_id,
                operation_kind="bundle.export.material",
                scope=ctx.require_subject(),
                payload=command,
            ) as tx:
                if tx.replayed:
                    saved = json_object(tx.replay_result)
                    identity = saved.get("material_id")
                    if not isinstance(identity, str):
                        raise DomainError(
                            "bundle_material_missing", "Export receipt has no material"
                        )
                    await service.app.material.read_in(tx, ctx, identity)
                    return saved
                document = BlobDocument(
                    filename=filename, media_type=lease.media_type, source_type="bundle_export"
                )
                info = await service.app.media.probe(lease)
                material = await service.app.material.put_blob_in(
                    tx, ctx, lease, document, info, required_source_ids=required_source_ids
                )
                await service.app.material.observe_in(
                    tx, ctx, material["id"], document, kind="contact"
                )
                return tx.result(
                    json_object(
                        {
                            "success": True,
                            "file_name": filename,
                            "material_id": material["id"],
                            "size": len(data),
                            "counts": summary,
                        }
                    )
                )
        finally:
            await service.app.content.release(lease)
    material_id = command.material_id
    if command.action == "resolve_file":
        from sqlalchemy import select

        from nous.material.service import materials, observations

        if not command.file_name:
            raise DomainError("filename_required", "Select an exported bundle file")
        async with service.app.authority.read(ctx) as view:
            rows = (
                await view.connection.execute(
                    select(materials.c.record)
                    .join(observations, observations.c.material_id == materials.c.id)
                    .where(
                        materials.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.subject_id,
                        observations.c.active.is_(True),
                    )
                )
            ).scalars()
            matches = [
                value
                for value in rows
                if value["filename"] == command.file_name
                and value["source_type"] == "bundle_export"
            ]
        if not matches:
            raise DomainError("bundle_file_not_found", "Exported bundle is unavailable")
        material_id = str(matches[0]["id"])
    if material_id is None:
        raise DomainError(
            "material_required", "Bundle inspection and installation require an uploaded original"
        )
    if command.action == "import":
        return json_object(
            {
                "success": True,
                "installation": await service.app.bundles.install_material(
                    ctx,
                    material_id,
                    command.operation_id or str(uuid4()),
                    command.mode,
                    command.placement,
                ),
            }
        )
    if command.action == "inspect":
        original = await service.app.material.read(ctx, material_id)
        return json_object(
            {
                "success": True,
                "manifest": await service.app.bundles.inspect_material(ctx, material_id),
                "material_id": material_id,
                "file_name": original["filename"],
                "size": original["byte_len"],
            }
        )
    material = await service.app.material.read(ctx, material_id)
    if command.action == "resolve_file":
        return json_object(
            {
                "success": True,
                "file_name": material["filename"],
                "material_id": material_id,
                "size": material["byte_len"],
            }
        )
    raise DomainError("action_invalid", "Unknown bundle administration action")
