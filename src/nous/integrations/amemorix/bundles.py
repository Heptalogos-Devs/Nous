"""Bundle host actions use domain archives and managed immutable originals."""

import asyncio
from hashlib import sha256
from io import BytesIO
from typing import TYPE_CHECKING
from uuid import uuid4
from zipfile import ZipFile

from nous_wave.contracts import BlobDocument, BundleRequest, CallContext, Contract, DomainError
from nous_wave.contracts.management import SubjectBundle
from pydantic import JsonValue

from nous.platform.content import ContentObject

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


class BundleInput(Contract):
    action: str
    material_id: str | None = None
    file_name: str | None = None
    installation_id: str | None = None
    operation_id: str | None = None
    preview: bool = False


async def bundle_admin(
    service: AMemorixService, ctx: CallContext, arguments: dict[str, JsonValue]
) -> dict[str, JsonValue]:
    command = BundleInput.model_validate(arguments)
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
        data = await service.app.bundles.export_zip(ctx, BundleRequest())

        def exported_closure() -> tuple[str, ...]:
            with ZipFile(BytesIO(data)) as archive:
                bundle = SubjectBundle.model_validate_json(archive.read("bundle.json"))
            return tuple(
                sorted(
                    {
                        *(record.reference.exact() for record in bundle.cognition),
                        *(material.id for material in bundle.materials),
                    }
                )
            )

        required_source_ids = await asyncio.to_thread(exported_closure)
        summary = await service.app.bundles.inspect_zip(ctx, data)
        if command.preview:
            return json_object({"success": True, "preview": True, "counts": summary})
        lease = await service.app.content.publish_bytes(
            data, "application/vnd.nous-wave.bundle+zip"
        )
        filename = "nous-" + sha256(data).hexdigest()[:16] + ".zip"
        try:
            async with service.app.authority.write(
                ctx,
                command.operation_id or str(uuid4()),
                operation_kind="bundle.export.material",
                scope=ctx.require_subject(),
                payload={"hash": lease.content_hash},
            ) as tx:
                if tx.replayed:
                    return json_object(tx.replay_result)
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
    content = ContentObject(material["content_hash"], material["byte_len"], material["media_type"])
    async with service.app.content.protect(content) as path:
        data = await asyncio.to_thread(path.read_bytes)
    if command.action == "inspect":
        return json_object(
            {
                "success": True,
                "manifest": await service.app.bundles.inspect_zip(ctx, data),
                "file_name": material["filename"],
                "material_id": material_id,
                "size": material["byte_len"],
            }
        )
    if command.action == "import":
        return json_object(
            {
                "success": True,
                "installation": await service.app.bundles.install_zip(
                    ctx, data, command.operation_id or str(uuid4())
                ),
            }
        )
    raise DomainError("action_invalid", "Unknown bundle administration action")
