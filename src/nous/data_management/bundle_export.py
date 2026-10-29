from __future__ import annotations

import asyncio
import base64
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import (
    BundleRequest,
    CallContext,
    CognitiveRecord,
    DomainError,
    MediaReference,
    SubjectBundle,
    UnitOutput,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.cognition.projection import media_references_in, references_in
from nous.cognition.schema import revisions, vectors
from nous.identity.service import require_subject_in
from nous.imports.schema import catalogs, inputs, outputs, successes, units
from nous.material.service import MaterialRecord, materials, observations, regions, source_permitted
from nous.platform.content import ContentObject
from nous.references import export_bindings_in

from .selection import select_roots_in

if TYPE_CHECKING:
    from .bundles import BundleService


async def export(self: BundleService, ctx: CallContext, command: BundleRequest | None = None):
    command = command or BundleRequest()
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx)
        all_records = {
            row.reference.exact(): row
            for row in [
                CognitiveRecord.model_validate(value)
                for value in (
                    await view.connection.execute(
                        select(revisions.c.record).where(
                            revisions.c.namespace_id == ctx.namespace_id,
                            revisions.c.subject_id == ctx.subject_id,
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
                        observations.c.subject_id == ctx.subject_id,
                    )
                )
            ).mappings()
        }
        selected, selected_observations = await select_roots_in(
            view,
            ctx,
            command,
            all_records,
            TypeAdapter(dict[str, dict[str, JsonValue]]).validate_python(source_rows),
        )
        chosen = (
            {reference.exact() for reference in command.references}
            if command.references is not None
            else selected
        )
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
                link.reference for link in record.evidence if link.reference
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
            for link in record.evidence
            if link.observation_id
        )
        selected_outputs: list[UnitOutput] = []
        for value in (await view.connection.execute(select(outputs.c.record))).scalars():
            output = UnitOutput.model_validate(value)
            if (
                output.cognition
                and all(ref.exact() in chosen for ref in output.cognition)
                and output.observation_id in source_rows
            ):
                selected_outputs.append(output)
                observation_ids.add(output.observation_id)
        from nous_wave.contracts.derivation import ArtifactRepresentation

        derived = await self.cognition.derivations.export_in(
            view,
            ctx,
            tuple(ref for record in records for ref in media_references_in(record.body)),
            observation_ids=tuple(sorted(observation_ids)),
        )
        observation_ids.update(derived.observation_ids)
        artifacts = {
            record.payload.material_id
            for record in derived.representations
            if isinstance(record.payload, ArtifactRepresentation)
        }
        observation_ids.update(
            oid for oid, source in source_rows.items() if source["material_id"] in artifacts
        )
        obs = [source_rows[oid] for oid in sorted(observation_ids)]
        material_ids = {row["material_id"] for row in obs} | set(derived.material_ids) | artifacts
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
                await view.connection.execute(select(units).where(units.c.id.in_(unit_ids)))
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
                    catalogs.c.subject_id == ctx.subject_id,
                )
            )
        ).mappings():
            value = dict(row)
            value["record"] = {
                **row["record"],
                "units": [unit for unit in row["record"]["units"] if unit["id"] in unit_ids],
                "formation_catalog": [
                    entry
                    for entry in row["record"].get("formation_catalog", [])
                    if entry["reference"]["kind"]
                    + ":"
                    + entry["reference"]["id"]
                    + "@"
                    + str(entry["reference"]["revision"])
                    in chosen
                ],
                "input_source_ids": [
                    sid for sid in row["record"]["input_source_ids"] if sid in input_ids
                ],
            }
            selected_catalogs.append(value)
        output_ids = {output.id for output in selected_outputs}
        facts = [
            dict(row)
            for row in (
                await view.connection.execute(
                    select(successes).where(
                        successes.c.namespace_id == ctx.namespace_id,
                        successes.c.subject_id == ctx.subject_id,
                    )
                )
            ).mappings()
            if set(row["record"]["unit_outputs"].values()).issubset(output_ids)
        ]
        result = TypeAdapter(dict[str, JsonValue]).validate_python(
            {
                "format": "nous.subject",
                "package": command.package.model_dump(mode="json") if command.package else None,
                "content_level": command.content_level,
                "version": 1,
                "id": str(uuid4()),
                "namespace_id": ctx.namespace_id,
                "subject_id": ctx.subject_id,
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

        region_ids = set(derived.source_region_ids) | {
            region_id
            for record in records
            for region_ids in record.source_regions.values()
            for region_id in region_ids
        }
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
        if command.assets is not None and command.assets.include_vectors:
            vector_references = {
                *chosen,
                *(f"material:{row['id']}" for row in mat),
                *(f"source-region:{region_id}" for region_id in region_ids),
                *(value.reference.media().exact() for value in derived.representations),
                *(f"derived-region:{region.id}" for region in derived.regions),
            }
            result["vectors"] = TypeAdapter(list[JsonValue]).validate_python(
                [
                    dict(row)
                    for row in (
                        await view.connection.execute(
                            select(vectors).where(vectors.c.reference.in_(vector_references))
                        )
                    ).mappings()
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
                        MediaReference(kind="source_region", id=region_id)
                        for region_id in region_ids
                    ),
                ),
            )
        ]
        result["contents"] = contents
        return SubjectBundle.model_validate(result).model_dump(mode="json")


async def export_zip(
    self: BundleService, ctx: CallContext, command: BundleRequest | None = None
) -> bytes:
    return (await self.export_archive(ctx, command)).data


async def inspect_zip(self: BundleService, ctx: CallContext, data: bytes):
    ctx.require_subject()
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
