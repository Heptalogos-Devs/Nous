"""An archive, its real source closure and summary come from one export snapshot."""

from __future__ import annotations

import json
from dataclasses import dataclass
from io import BytesIO
from typing import TYPE_CHECKING
from zipfile import ZIP_DEFLATED, ZipFile

from nous_sdk.contracts import BundleRequest, NamespaceBundle
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from pydantic import JsonValue

if TYPE_CHECKING:
    from .bundles import BundleService


@dataclass(frozen=True)
class BundleArchive:
    data: bytes
    summary: dict[str, JsonValue]


def summary(bundle: NamespaceBundle) -> dict[str, JsonValue]:
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
    bundle = NamespaceBundle.model_validate(await self.export(ctx, command))
    if command.content_level != "full" or (
        command.assets is not None and not command.assets.include_images
    ):
        raise DomainError(
            "bundle_full_required",
            "Archives require full content and original source assets",
        )
    output = BytesIO()
    with ZipFile(output, "w", ZIP_DEFLATED) as archive:
        archive.writestr(
            "bundle.json", json.dumps(bundle.model_dump(mode="json"), ensure_ascii=False)
        )
    return BundleArchive(output.getvalue(), summary(bundle))
