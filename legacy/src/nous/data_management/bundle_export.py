from __future__ import annotations

import asyncio
import base64
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    BundleRequest,
    CognitiveRecord,
    CognitiveRef,
    MediaReference,
    NamespaceBundle,
    UnitOutput,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.identity import IdentityPurpose
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.cognition.entities import project_entity_in
from nous.cognition.projection import evidence_in, media_references_in, references_in
from nous.cognition.schema import revisions, vectors
from nous.identity.access import require_namespace_in
from nous.imports.schema import catalogs, inputs, outputs, successes, units
from nous.material.service import MaterialRecord, materials, observations, regions, source_permitted
from nous.platform.content import ContentObject
from nous.platform.persistence import ReadView
from nous.references import export_bindings_in

from .selection import select_roots_in

if TYPE_CHECKING:
    from .bundles import BundleService


async def export(self: BundleService, ctx: CallContext, command: BundleRequest | None = None):
    async with self.authority.read(ctx) as view:
        return await self.export_in(view, ctx, command or BundleRequest())


async def export_in(
    self: BundleService,
    view: ReadView,
    ctx: CallContext,
    command: BundleRequest,
    *,
    purpose: IdentityPurpose = "active",
):
    await require_namespace_in(view, ctx, purpose=purpose)
    all_records = {
        row.reference.exact(): row
        for row in [
            await project_entity_in(self.cognition, view, ctx, CognitiveRecord.model_validate(value))
            for value in (
                await view.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        
                    )
                )
            ).scalars()
        ]
    }
    source_rows = {
        row["id"]: dict(row)
        for row in (
            await view.connection.execute(
                select(observations).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    
                )
            )
        ).mappings()
    }
    selected_observations: set[str]
    if command.references is None:
        chosen, selected_observations = await select_roots_in(
            view,
            ctx,
            command,
            all_records,
            TypeAdapter(dict[str, dict[str, JsonValue]]).validate_python(source_rows),
        )
    else:
        chosen = {reference.exact() for reference in command.references}
        selected_observations = set()
    for observation_id in selected_observations:
        if observation_id not in source_rows or not source_permitted(
            ctx, str(source_rows[observation_id]["material_id"]), observation_id
        ):
            raise DomainError(
                "source_unavailable",
                "Bundle selection includes a source outside the current read scope",
            )
    pending = list(chosen)
    while pending:
        key = pending.pop()
        if key not in all_records:
            raise DomainError(
                "bundle_reference_missing",
                "A selected bundle revision does not exist",
                reference=key,
            )
        record = all_records[key]
        if not all(
            oid in source_rows and source_permitted(ctx, source_rows[oid]["material_id"], oid)
            for oid in record.source_roots
        ):
            raise DomainError(
                "source_unavailable", "Bundle includes sources outside the current scope"
            )
        dependencies = list(references_in(record.body)) + [
            link.reference for link in evidence_in(record) if link.reference
        ]
        for reference in dependencies:
            if reference.exact() not in chosen:
                chosen.add(reference.exact())
                pending.append(reference.exact())
    records = [all_records[key] for key in sorted(chosen)]
    observation_ids = selected_observations | {
        oid for record in records for oid in record.source_roots
    }
    observation_ids.update(
        link.observation_id
        for record in records
        for link in evidence_in(record)
        if link.observation_id
    )
    from nous_sdk.contracts import MemoryBody

    observation_ids.update(
        record.body.contact_observation_id
        for record in records
        if isinstance(record.body, MemoryBody) and record.body.contact_observation_id
    )
    selected_outputs: list[UnitOutput] = []
    output_rows = await view.connection.execute(
        select(outputs.c.record)
        .select_from(
            outputs.join(units, outputs.c.unit_id == units.c.id).join(
                catalogs, catalogs.c.id == units.c.catalog_id
            )
        )
        .where(catalogs.c.namespace_id == ctx.namespace_id)
    )
    for value in output_rows.scalars():
        output = UnitOutput.model_validate(value)
        if (
            output.cognition
            and all(ref.exact() in chosen for ref in output.cognition)
            and output.observation_id in source_rows
        ):
            selected_outputs.append(output)
            observation_ids.add(output.observation_id)
    from nous_sdk.contracts.derivation import DerivedBundle

    derivation_refs = tuple(
        ref for record in records for ref in media_references_in(record.body)
    )
    derived = await self.cognition.derivations.export_in(
        view,
        ctx,
        derivation_refs,
        observation_ids=tuple(sorted(observation_ids)),
    )
    exported_references = {reference.exact() for reference in derivation_refs}
    exported_references.update(
        value.reference.media().exact() for value in derived.representations
    )
    representations = {
        value.reference.media().exact(): value for value in derived.representations
    }
    derived_regions = {value.id: value for value in derived.regions}
    observation_ids.update(derived.observation_ids)
    material_ids = {row["material_id"] for row in source_rows.values() if row["id"] in observation_ids}
    material_ids.update(derived.material_ids)
    source_region_ids = set(derived.source_region_ids) | {
        region_id
        for record in records
        for region_ids in record.source_regions.values()
        for region_id in region_ids
    }
    required_material_ids: set[str] = set()
    processed_material_ids: set[str] = set()
    processed_region_ids: set[str] = set()
    pending_derivations: dict[str, MediaReference] = {}
    while True:
        changed = False
        new_material_ids = material_ids - processed_material_ids
        if new_material_ids:
            material_rows = (
                await view.connection.execute(
                    select(materials.c.id, materials.c.record).where(
                        materials.c.id.in_(new_material_ids),
                        materials.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).all()
            found_material_ids = {str(row.id) for row in material_rows}
            if found_material_ids != new_material_ids:
                raise DomainError(
                    "bundle_source_missing", "Bundle material dependency is unavailable"
                )
            for row in material_rows:
                record = TypeAdapter(MaterialRecord).validate_python(row.record)
                for observation_id in record.get("required_observation_ids", ()):
                    observation_ids.add(observation_id)
                for source in record.get("required_source_ids", ()):
                    kind, separator, identity = source.partition(":")
                    if not separator or not identity:
                        raise DomainError(
                            "bundle_source_missing", "Material dependency must be a typed source reference"
                        )
                    if kind == "material":
                        required_material_ids.add(identity)
                        material_ids.add(identity)
                    elif kind in {"source-region", "source_region"}:
                        source_region_ids.add(identity)
                    elif kind == "representation":
                        pending_derivations[source] = MediaReference(kind="representation", id=identity)
                    elif kind in {"derived-region", "derived_region"}:
                        pending_derivations[source] = MediaReference(kind="derived_region", id=identity)
                    else:
                        raise DomainError(
                            "bundle_source_missing", "Material dependency has an unsupported source kind"
                        )
            processed_material_ids.update(new_material_ids)
            changed = True

        new_region_ids = source_region_ids - processed_region_ids
        if new_region_ids:
            region_rows = (
                await view.connection.execute(
                    select(regions.c.id, regions.c.material_id).where(
                        regions.c.id.in_(new_region_ids),
                        regions.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).all()
            if {str(row.id) for row in region_rows} != new_region_ids:
                raise DomainError(
                    "bundle_source_missing", "Bundle source region is unavailable"
                )
            for row in region_rows:
                material_ids.add(str(row.material_id))
                required_material_ids.add(str(row.material_id))
            processed_region_ids.update(new_region_ids)
            changed = True

        new_derivations = {
            key: value
            for key, value in pending_derivations.items()
            if key not in exported_references
        }
        if new_derivations:
            additional = await self.cognition.derivations.export_in(
                view, ctx, tuple(new_derivations.values())
            )
            representations.update(
                {value.reference.media().exact(): value for value in additional.representations}
            )
            derived_regions.update({value.id: value for value in additional.regions})
            observation_ids.update(additional.observation_ids)
            material_ids.update(additional.material_ids)
            source_region_ids.update(additional.source_region_ids)
            exported_references.update(new_derivations)
            changed = True

        for material_id in required_material_ids:
            if any(
                source_rows[observation_id]["material_id"] == material_id
                for observation_id in observation_ids
                if observation_id in source_rows
            ):
                continue
            candidates = [
                source
                for source in source_rows.values()
                if source["material_id"] == material_id
                and source["active"]
                and not source["record"].get("purged")
                and source_permitted(ctx, material_id, source["id"])
            ]
            if not candidates:
                raise DomainError(
                    "source_unavailable", "Required material source is outside the current scope"
                )
            observation_ids.add(sorted(candidates, key=lambda source: source["id"])[0]["id"])
            changed = True

        missing_observations = observation_ids - source_rows.keys()
        if missing_observations:
            raise DomainError(
                "bundle_source_missing", "Bundle observation dependency is unavailable"
            )
        if any(
            not source_permitted(
                ctx,
                str(source_rows[observation_id]["material_id"]),
                observation_id,
            )
            for observation_id in observation_ids
        ):
            raise DomainError(
                "source_unavailable", "Bundle source dependencies are outside the current scope"
            )
        for observation_id in observation_ids:
            material_ids.add(str(source_rows[observation_id]["material_id"]))

        if not changed:
            break

    derived = DerivedBundle(
        representations=tuple(representations.values()),
        regions=tuple(derived_regions.values()),
    )
    obs = [source_rows[oid] for oid in sorted(observation_ids)]
    material_ids.update(derived.material_ids)
    mat = [
        dict(row)
        for row in (
            await view.connection.execute(
                select(materials).where(
                    materials.c.id.in_(material_ids),
                    materials.c.namespace_id == ctx.namespace_id,
                )
            )
        ).mappings()
    ]
    unit_ids = {output.unit_id for output in selected_outputs}
    selected_units = [
        dict(row)
        for row in (
            await view.connection.execute(
                select(units)
                .select_from(units.join(catalogs, catalogs.c.id == units.c.catalog_id))
                .where(units.c.id.in_(unit_ids), catalogs.c.namespace_id == ctx.namespace_id)
            )
        ).mappings()
    ]
    input_ids = {row["record"]["input_source_id"] for row in selected_units}
    selected_inputs = [
        dict(row)
        for row in (
            await view.connection.execute(select(inputs).where(inputs.c.id.in_(input_ids)))
        ).mappings()
    ]
    catalog_ids = {row["catalog_id"] for row in selected_units}
    selected_catalogs: list[dict[str, JsonValue]] = []
    for row in (
        await view.connection.execute(
            select(catalogs).where(
                catalogs.c.id.in_(catalog_ids),
                catalogs.c.namespace_id == ctx.namespace_id,
                
            )
        )
    ).mappings():
        value = dict(row)
        header = dict(row["record"])
        header["unit_ids"] = [identifier for identifier in header["unit_ids"] if identifier in unit_ids]
        header["formation_references"] = [
            reference
            for reference in header.get("formation_references", [])
            if CognitiveRef.model_validate(reference).exact() in chosen
        ]
        header["input_source_ids"] = [
            identifier
            for identifier in header["input_source_ids"]
            if identifier in input_ids
        ]
        header["errors"] = [
            error
            for error in header.get("errors", [])
            if error.get("input_source_id") in input_ids
        ]
        value["record"] = {
            **header,
        }
        selected_catalogs.append(value)
    output_ids = {output.id for output in selected_outputs}
    facts = [
        dict(row)
        for row in (
            await view.connection.execute(
                select(successes).where(
                    successes.c.namespace_id == ctx.namespace_id,
                    
                )
            )
        ).mappings()
        if row["input_source_id"] in input_ids
        and set(row["record"]["unit_outputs"]).issubset(unit_ids)
        and set(row["record"]["unit_outputs"].values()).issubset(output_ids)
    ]
    result = TypeAdapter(dict[str, JsonValue]).validate_python(
        {
            "format": "nous.namespace.bundle",
            "package": command.package.model_dump(mode="json") if command.package else None,
            "content_level": command.content_level,
            "version": 2,
            "id": str(uuid4()),
            "namespace_id": ctx.namespace_id,
            
            "authority_commit_seq": view.commit_seq,
            "cognition": [record.model_dump(mode="json") for record in records],
            "materials": mat,
            "observations": obs,
            "catalogs": selected_catalogs,
            "inputs": selected_inputs,
            "units": selected_units,
            "outputs": [output.model_dump(mode="json") for output in selected_outputs],
            "file_successes": facts,
        }
    )

    source_region_ids.update(
        str(row["record"]["source_region_id"])
        for row in selected_units
        if row["record"].get("source_region_id")
    )
    source_region_ids.update(
        str(row["record"]["source"]["id"])
        for row in selected_units
        if isinstance(row["record"].get("source"), dict)
        and row["record"]["source"].get("kind") == "source_region"
    )
    region_ids = source_region_ids
    result["regions"] = [
        dict(row)
        for row in (
            await view.connection.execute(
                select(regions).where(
                    regions.c.id.in_(region_ids), regions.c.namespace_id == ctx.namespace_id
                )
            )
        ).mappings()
    ]
    contents: dict[str, JsonValue] = {}
    if self.content_store:
        for row in mat:
            material = TypeAdapter(MaterialRecord).validate_python(row["record"])
            if material.get("purged"):
                continue
            if material["text"] is not None:
                contents[material["content_hash"]] = base64.b64encode(
                    material["text"].encode()
                ).decode("ascii")
                continue
            content = ContentObject(
                material["content_hash"], material["byte_len"], material["media_type"]
            )
            async with self.content_store.protect(content) as path:
                contents[content.content_hash] = base64.b64encode(
                    await asyncio.to_thread(path.read_bytes)
                ).decode("ascii")
    result["derivations"] = derived.model_dump(mode="json")
    include_vectors = command.assets is None or (
        command.assets is not None and command.assets.include_vectors
    )
    if include_vectors:
        vector_references = {
            *chosen,
            *(f"material:{row['id']}" for row in mat),
            *(f"source-region:{region_id}" for region_id in region_ids),
            *(value.reference.media().exact() for value in derived.representations),
            *(f"derived-region:{region.id}" for region in derived.regions),
        }
        statement = select(vectors).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference.in_(vector_references))
        if command.assets is not None and command.assets.vector_spaces:
            statement = statement.where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.space.in_(command.assets.vector_spaces))
        from .schema import bundle_installs

        installations = (
            (
                await view.connection.execute(
                    select(bundle_installs.c.record).where(
                        bundle_installs.c.namespace_id == ctx.namespace_id,
                        
                    )
                )
            )
            .scalars()
            .all()
        )
        provenance: dict[tuple[str, str], dict[str, JsonValue]] = {}
        for installation in sorted(installations, key=lambda value: value.get("commit_seq", 0)):
            for value in installation.get("vector_provenance", []):
                provenance[(value["reference"], value["space"])] = value
        result["vectors"] = TypeAdapter(list[JsonValue]).validate_python(
            [
                {
                    **{key: value for key, value in row.items() if key != "namespace_id"},
                    "provenance": provenance.get((row["reference"], row["space"]), {}).get(
                        "provenance", {}
                    ),
                }
                for row in (await view.connection.execute(statement)).mappings()
            ]
        )
    result["bindings"] = [
        binding.model_dump(mode="json")
        for binding in await export_bindings_in(
            view,
            ctx,
            (
                *tuple(record.reference for record in records),
                *tuple(record.reference.media() for record in derived.representations),
                *tuple(
                    MediaReference(kind="derived_region", id=region.id)
                    for region in derived.regions
                ),
                *tuple(MediaReference(kind="material", id=row["id"]) for row in mat),
                *tuple(
                    MediaReference(kind="source_region", id=region_id) for region_id in region_ids
                ),
            ),
        )
    ]
    result["contents"] = contents
    return NamespaceBundle.model_validate(result).model_dump(mode="json")


async def export_zip(
    self: BundleService, ctx: CallContext, command: BundleRequest | None = None
) -> bytes:
    return (await self.export_archive(ctx, command)).data


async def inspect_zip(self: BundleService, ctx: CallContext, data: bytes):
    from .archive_read import decode

    bundle = await asyncio.to_thread(decode, data)
    for row in bundle.materials:
        material = TypeAdapter(MaterialRecord).validate_python(row.record)
        encoded = bundle.contents.get(row.content_hash)
        content = (
            base64.b64decode(encoded, validate=True)
            if encoded is not None
            else material["text"].encode()
            if material["text"] is not None
            else None
        )
        if content is not None and (
            sha256(content).hexdigest() != row.content_hash or len(content) != material["byte_len"]
        ):
            raise DomainError(
                "bundle_content_hash",
                "Bundle material content does not match its immutable identity",
            )
    return {
        "format": bundle.format,
        "version": bundle.version,
        "id": bundle.id,
        "cognition": len(bundle.cognition),
        "materials": len(bundle.materials),
        "observations": len(bundle.observations),
        "units": len(bundle.units),
        "outputs": len(bundle.outputs),
        "file_successes": len(bundle.file_successes),
        "regions": len(bundle.regions),
        "bytes": sum(
            TypeAdapter(MaterialRecord).validate_python(row.record)["byte_len"]
            for row in bundle.materials
        ),
    }
