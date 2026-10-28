from __future__ import annotations

import base64
import json
from datetime import datetime
from hashlib import sha256
from io import BytesIO
from typing import TYPE_CHECKING
from uuid import uuid4
from zipfile import ZipFile

from nous_wave.contracts import (
    CallContext,
    CatalogView,
    CognitiveRecord,
    DomainError,
    FileSuccess,
    InstallBundle,
    MediaReference,
    SourceRegion,
    SubjectBundle,
    TimePoint,
    UnitOutput,
)
from nous_wave.contracts.context import CognitiveTarget
from nous_wave.contracts.references import ObjectTarget, ReferenceBinding
from pydantic import JsonValue, TypeAdapter

from nous.cognition.projection import references_in
from nous.cognition.schema import revisions
from nous.identity.service import require_subject_in
from nous.imports.schema import catalogs, inputs, outputs, successes, units
from nous.material.service import MaterialRecord, materials, observations, regions
from nous.platform.content import ContentLease
from nous.references import allocate_in, import_bindings_in

from .schema import bundle_installs

if TYPE_CHECKING:
    from .bundles import BundleService


async def install_zip(self: BundleService, ctx: CallContext, data: bytes, operation_id: str):
    with ZipFile(BytesIO(data)) as archive:
        info = archive.getinfo("bundle.json")
        if info.file_size > 64 * 1024 * 1024:
            raise DomainError("bundle_too_large", "Bundle JSON exceeds 64 MiB")
        bundle = json.loads(archive.read(info))
    return await self.install(ctx, InstallBundle(operation_id=operation_id, bundle=bundle))


async def install(self: BundleService, ctx: CallContext, command: InstallBundle):
    receipt = await self.authority.receipt(
        ctx,
        command.operation_id,
        operation_kind="bundle.install",
        scope=ctx.require_subject(),
        payload=command,
    )
    if receipt is not None:
        return receipt.result
    bundle = SubjectBundle.model_validate(command.bundle)
    leases: dict[str, ContentLease] = {}
    try:
        if self.content_store:
            for row in bundle.materials:
                material = TypeAdapter(MaterialRecord).validate_python(row.record)
                if material.get("purged"):
                    continue
                encoded = bundle.contents.get(row.content_hash)
                data = (
                    base64.b64decode(encoded, validate=True)
                    if encoded is not None
                    else material["text"].encode()
                    if material["text"] is not None
                    else None
                )
                if (
                    data is None
                    or sha256(data).hexdigest() != row.content_hash
                    or len(data) != material["byte_len"]
                ):
                    raise DomainError(
                        "bundle_content_missing",
                        "Original content is absent or does not match its hash",
                    )
                leases[row.content_hash] = await self.content_store.publish_bytes(
                    data, material["media_type"]
                )
        return await self.install_prepared(ctx, command, bundle, leases)
    finally:
        if self.content_store:
            for lease in leases.values():
                await self.content_store.release(lease)


async def install_prepared(
    self: BundleService,
    ctx: CallContext,
    command: InstallBundle,
    original: SubjectBundle,
    leases: dict[str, ContentLease],
):
    bundle = original.model_dump(mode="json")
    if bundle.get("format") != "nous.subject" or bundle.get("version") != 1:
        raise DomainError("bundle_format", "Unsupported Nous subject bundle format")
    records = [CognitiveRecord.model_validate(value) for value in bundle["cognition"]]
    obs = {row["id"]: row for row in bundle["observations"]}
    mats = {row["id"]: row for row in bundle["materials"]}
    keys = {record.reference.exact() for record in records}
    for record in records:
        if not record.source_roots or any(oid not in obs for oid in record.source_roots):
            raise DomainError(
                "bundle_source_missing", "Cognition has an unresolved supporting source"
            )
        for ref in list(references_in(record.body)) + [
            link.reference for link in record.evidence if link.reference
        ]:
            if ref.exact() not in keys:
                raise DomainError(
                    "bundle_reference_missing",
                    "Cognition has an unresolved exact revision",
                    reference=ref.exact(),
                )
        if any(link.observation_id and link.observation_id not in obs for link in record.evidence):
            raise DomainError(
                "bundle_source_missing", "Cognition evidence has an unresolved observation"
            )
    for row in obs.values():
        if row["material_id"] not in mats or row["kind"] not in {"import", "contact"}:
            raise DomainError(
                "bundle_observation", "Observation has invalid material or event kind"
            )
    for row in mats.values():
        value = row["record"]
        if (
            value.get("text") is not None
            and sha256(value["text"].encode()).hexdigest() != row["content_hash"]
        ):
            raise DomainError(
                "bundle_content_hash", "Material bytes do not match their content hash"
            )
    async with self.authority.write(
        ctx,
        command.operation_id,
        operation_kind="bundle.install",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return tx.replay_result
        await require_subject_in(tx, ctx, "memory")
        mapping = {
            value: str(uuid4())
            for value in {
                *(record.reference.id for record in records),
                *obs,
                *mats,
                *(
                    row["id"]
                    for name in ("catalogs", "inputs", "units", "file_successes", "regions")
                    for row in bundle.get(name, [])
                ),
                *(row["id"] for row in bundle.get("outputs", [])),
            }
        }
        # These fields own identities; prose and arbitrary external origin strings remain literal.
        identity_fields = {
            "id",
            "namespace_id",
            "subject_id",
            "catalog_id",
            "input_source_id",
            "material_id",
            "observation_id",
            "unit_id",
            "output_id",
            "parent_catalog_id",
        }

        def remap(value: JsonValue, field: str = "") -> JsonValue:
            if isinstance(value, dict):
                if field in {"origin", "identifiers"}:
                    return value
                if field == "source_regions":
                    return {
                        mapping[key]: [
                            mapping.get(region, region)
                            for region in items
                            if isinstance(region, str)
                        ]
                        for key, items in value.items()
                        if isinstance(items, list)
                    }
                if field == "unit_outputs":
                    return {
                        mapping[key]: mapping[item]
                        for key, item in value.items()
                        if isinstance(item, str)
                    }
                return {key: remap(item, key) for key, item in value.items()}
            if isinstance(value, list):
                return [remap(item, field) for item in value]
            if field == "namespace_id":
                return ctx.namespace_id
            if field == "subject_id":
                return ctx.subject_id
            if field == "required_source_ids" and isinstance(value, str):
                if ":" not in value:
                    return mapping.get(value, value)
                kind, identity = value.split(":", 1)
                object_id, at, revision = identity.partition("@")
                return f"{kind}:{mapping.get(object_id, object_id)}" + (
                    f"@{revision}" if at else ""
                )
            if field in identity_fields or field in {
                "source_roots",
                "support_roots",
                "input_source_ids",
            }:
                return mapping.get(value, value) if isinstance(value, str) else value
            return value

        remapped = SubjectBundle.model_validate(remap(bundle)).model_dump(mode="json")
        for binding in original.bindings:
            target = binding.target
            identity = (
                target.identity.id if isinstance(target, ObjectTarget) else target.reference.id
            )
            if identity not in mapping or (
                isinstance(target, CognitiveTarget) and target.reference.exact() not in keys
            ):
                raise DomainError(
                    "bundle_reference_missing", "Readable binding points outside the bundle"
                )
        for new in remapped["materials"]:
            new["record"]["commit_seq"] = tx.commit_seq
            new["record"]["addresses"] = (
                await allocate_in(tx, ctx, MediaReference(kind="material", id=new["id"]))
            ).model_dump(mode="json")
            await tx.connection.execute(materials.insert().values(**new))
            if self.content_store and new["content_hash"] in leases:
                await self.content_store.attach_in(tx, leases[new["content_hash"]], new["id"])
        for new in remapped["regions"]:
            new["record"]["addresses"] = (
                await allocate_in(tx, ctx, MediaReference(kind="source_region", id=new["id"]))
            ).model_dump(mode="json")
            SourceRegion.model_validate(new["record"])
            await tx.connection.execute(regions.insert().values(**new))
        for new in remapped["observations"]:
            new["commit_seq"] = tx.commit_seq
            new["record"]["commit_seq"] = tx.commit_seq
            await tx.connection.execute(observations.insert().values(**new))
        for name, table in (("catalogs", catalogs), ("inputs", inputs), ("units", units)):
            for new in remapped[name]:
                if name == "catalogs":
                    CatalogView.model_validate(new["record"])
                await tx.connection.execute(table.insert().values(**new))
        heads: dict[str, int] = {}
        for record in records:
            heads[record.reference.id] = max(
                heads.get(record.reference.id, 0), record.reference.revision
            )
        for record, value in zip(records, remapped["cognition"], strict=True):
            value["commit_seq"] = tx.commit_seq
            adopted = CognitiveRecord.model_validate(value)
            adopted = adopted.model_copy(
                update={
                    "addresses": await allocate_in(tx, ctx, adopted.reference),
                    "times": adopted.times.model_copy(
                        update={
                            "recorded": TimePoint(
                                coordinate=int(
                                    datetime.fromisoformat(tx.recorded_at).timestamp() * 1000
                                )
                            )
                        }
                    ),
                }
            )
            await tx.connection.execute(
                revisions.insert().values(
                    id=adopted.reference.id,
                    revision=adopted.reference.revision,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.require_subject(),
                    kind=adopted.reference.kind,
                    head=heads[record.reference.id] == record.reference.revision,
                    record=adopted.model_dump(mode="json"),
                    commit_seq=tx.commit_seq,
                )
            )
        for value, new in zip(bundle.get("outputs", []), remapped["outputs"], strict=True):
            new["attempt_job_id"] = None
            new["commit_seq"] = tx.commit_seq
            output = UnitOutput.model_validate(new)
            if output.observation_id not in mapping.values() or not any(
                row["id"] == value["observation_id"] and row["kind"] == "import"
                for row in obs.values()
            ):
                raise DomainError(
                    "bundle_output_observation",
                    "UnitOutput must reference its real Import observation",
                )
            await tx.connection.execute(
                outputs.insert().values(
                    id=output.id,
                    unit_id=output.unit_id,
                    operation_id=command.operation_id,
                    record=output.model_dump(mode="json"),
                )
            )
        for new in remapped["file_successes"]:
            new["record"]["commit_seq"] = tx.commit_seq
            FileSuccess.model_validate(new["record"])
            await tx.connection.execute(successes.insert().values(**new))
        await import_bindings_in(
            tx,
            ctx,
            original.id,
            tuple(ReferenceBinding.model_validate(value) for value in remapped["bindings"]),
        )
        installation_id = str(uuid4())
        result = {
            "installation_id": installation_id,
            "state": "active",
            "owned_observation_ids": [mapping[oid] for oid in obs],
            "bundle_id": bundle["id"],
            "mapping": mapping,
            "cognition_count": len(records),
            "output_count": len(bundle.get("outputs", [])),
            "commit_seq": tx.commit_seq,
        }
        await tx.connection.execute(
            bundle_installs.insert().values(
                id=installation_id,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                record=result,
            )
        )
        return tx.result(result)
