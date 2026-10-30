"""An archive, its real source closure and summary come from one export snapshot."""

from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass
from io import BytesIO
from typing import TYPE_CHECKING
from zipfile import ZIP_DEFLATED, ZipFile

from nous_sdk.contracts import BundleRequest, CallContext, DomainError, SubjectBundle
from pydantic import JsonValue

from nous.platform.content import ContentObject

if TYPE_CHECKING:
    from .bundles import BundleService


@dataclass(frozen=True)
class BundleArchive:
    data: bytes
    required_source_ids: tuple[str, ...]
    summary: dict[str, JsonValue]


def summary(bundle: SubjectBundle) -> dict[str, JsonValue]:
    return {
        "format": bundle.format,
        "version": bundle.version,
        "id": bundle.id,
        "package": bundle.package.model_dump(mode="json") if bundle.package else None,
        "content_level": bundle.content_level,
        "cognition": len(bundle.cognition),
        "materials": len(bundle.materials),
        "observations": len(bundle.observations),
        "units": len(bundle.units),
        "outputs": len(bundle.outputs),
        "file_successes": len(bundle.file_successes),
        "regions": len(bundle.regions),
        "representations": len(bundle.derivations.representations),
        "vectors": len(bundle.vectors),
    }


async def export_archive(
    self: BundleService, ctx: CallContext, command: BundleRequest | None = None
) -> BundleArchive:
    command = command or BundleRequest()
    bundle = SubjectBundle.model_validate(await self.export(ctx, command))
    if command.format == "amemorix":
        from .portable import write_portable

        data = await asyncio.to_thread(write_portable, bundle, command)
        with ZipFile(BytesIO(data)) as exported:
            portable_manifest = json.loads(exported.read("manifest.json"))
        actual_summary = {
            "format": portable_manifest["format"],
            "package": portable_manifest["package"],
            "content_level": portable_manifest["content_level"],
            **portable_manifest["counts"],
        }
    else:
        if command.content_level != "full" or (
            command.assets is not None and not command.assets.include_images
        ):
            raise DomainError(
                "bundle_native_full_required",
                "Native archives preserve full history and original source assets",
            )
        output = BytesIO()
        with ZipFile(output, "w", ZIP_DEFLATED) as archive:
            archive.writestr(
                "bundle.json", json.dumps(bundle.model_dump(mode="json"), ensure_ascii=False)
            )
        data = output.getvalue()
        actual_summary = summary(bundle)
    roots = tuple(
        sorted(
            {
                *(record.reference.exact() for record in bundle.cognition),
                *(material.id for material in bundle.materials),
                *(observation.id for observation in bundle.observations),
            }
        )
    )
    return BundleArchive(data, roots, actual_summary)


async def inspect_material(self: BundleService, ctx: CallContext, material_id: str):
    if self.content_store is None:
        raise DomainError("content_unavailable", "Bundle archive bytes are unavailable")
    material = await self.cognition.materials.read(ctx, material_id)
    async with self.content_store.protect(
        ContentObject(material["content_hash"], material["byte_len"], material["media_type"])
    ) as path:
        data = await asyncio.to_thread(path.read_bytes)
    return await self.inspect_zip(ctx, data)
