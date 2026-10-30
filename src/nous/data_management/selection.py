"""Resolve bundle selectors to actual scoped observation and cognition identities."""

from nous_sdk.contracts import BundleRequest, CallContext, CognitiveRecord, DomainError
from nous_sdk.contracts.bundles import (
    AllSelection,
    ChatSelection,
    ImageSelection,
    MaterialSelection,
    PackageSelection,
    PackageTokenSelection,
    SourceSelection,
)
from pydantic import JsonValue
from sqlalchemy import select

from nous.imports.schema import outputs
from nous.material.service import materials, source_permitted
from nous.platform.persistence import ReadView

from .schema import bundle_installs


async def select_roots_in(
    view: ReadView,
    ctx: CallContext,
    command: BundleRequest,
    records: dict[str, CognitiveRecord],
    sources: dict[str, dict[str, JsonValue]],
) -> tuple[set[str], set[str]]:
    selector = command.selector
    if isinstance(selector, AllSelection):
        allowed_sources = {
            key
            for key, source in sources.items()
            if source_permitted(ctx, str(source["material_id"]), key)
            and isinstance(source["record"], dict)
            and source["record"].get("source_type") != "bundle_export"
        }
        return {
            key
            for key, record in records.items()
            if set(record.source_roots).issubset(allowed_sources)
        }, allowed_sources
    selected_sources: set[str] = set()
    if isinstance(selector, ChatSelection):
        for key, row in sources.items():
            source = row["record"]
            if not isinstance(source, dict):
                continue
            placement = source.get("placement")
            origin = source.get("origin")
            if isinstance(placement, dict):
                matches = (
                    placement.get("kind") == "chat"
                    and placement.get("conversation_ref") == selector.conversation_ref
                )
            else:
                matches = isinstance(origin, dict) and selector.conversation_ref in origin.values()
            if matches:
                selected_sources.add(key)
    elif isinstance(selector, SourceSelection):
        from nous.material.named_sources import named_source_observations_in

        selected_sources = set(await named_source_observations_in(view, ctx, selector.sources))
    elif isinstance(selector, (MaterialSelection, ImageSelection)):
        rows = (
            await view.connection.execute(
                select(materials.c.id, materials.c.record).where(
                    materials.c.namespace_id == ctx.namespace_id
                )
            )
        ).all()
        selected_materials: set[str] = set()
        for material_id, value in rows:
            if isinstance(selector, MaterialSelection):
                matches = material_id in selector.material_ids
            else:
                matches = str(value["media_type"]).startswith("image/") and (
                    value["content_hash"] in selector.content_hashes
                    or not selector.content_hashes
                    and not selector.occurrence_ids
                )
            if matches:
                selected_materials.add(str(material_id))
        selected_sources = {
            key for key, row in sources.items() if row["material_id"] in selected_materials
        }
        if isinstance(selector, ImageSelection):
            image_materials = {
                identity
                for identity, value in rows
                if str(value["media_type"]).startswith("image/")
            }
            selected_sources.update(
                key
                for key, event in sources.items()
                if key in selector.occurrence_ids and event["material_id"] in image_materials
            )
    elif isinstance(selector, (PackageSelection, PackageTokenSelection)):
        rows = (
            await view.connection.execute(
                select(bundle_installs.c.record).where(
                    bundle_installs.c.namespace_id == ctx.namespace_id,
                    bundle_installs.c.subject_id == ctx.subject_id,
                )
            )
        ).scalars()
        found = False
        for value in rows:
            package_id = (
                value.get("package", {}).get("id")
                if isinstance(value.get("package"), dict)
                else None
            )
            matches = (
                (
                    value.get("installation_id") == selector.installation_id
                    if selector.installation_id
                    else package_id == selector.package_id
                )
                if isinstance(selector, PackageSelection)
                else selector.token in {package_id, value.get("installation_id")}
            )
            if matches:
                found = True
                selected_sources.update(str(key) for key in value.get("owned_observation_ids", []))
        if not found:
            raise DomainError(
                "bundle_install_not_found", "No installation matches this package selector"
            )
    else:
        values = (await view.connection.execute(select(outputs.c.record))).scalars()
        for value in values:
            if (
                value.get("attempt_job_id") == selector.job_id
                and value.get("observation_id") in sources
            ):
                selected_sources.add(str(value["observation_id"]))
        if not selected_sources:
            raise DomainError(
                "import_task_not_found", "No adopted outputs match this actual import job"
            )
    if isinstance(selector, ImageSelection) and not (
        command.assets and command.assets.include_image_related
    ):
        return set(), selected_sources
    chosen = {
        key
        for key, record in records.items()
        if set(record.source_roots).intersection(selected_sources)
    }
    return chosen, selected_sources
