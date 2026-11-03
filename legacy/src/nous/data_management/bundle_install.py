from __future__ import annotations

import asyncio
import base64
import json
from datetime import datetime
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4

from nous_sdk.contracts import (
    CognitiveRecord,
    FileSuccess,
    InstallBundle,
    MediaReference,
    NamespaceBundle,
    SourceRegion,
    TimePoint,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.context import CognitiveTarget
from nous_sdk.contracts.identity import IdentityPurpose
from nous_sdk.contracts.references import ObjectTarget
from pydantic import TypeAdapter
from sqlalchemy import select

from nous.cognition.projection import evidence_in, references_in
from nous.cognition.schema import revisions, vectors
from nous.identity.access import require_namespace_in
from nous.imports.schema import catalogs, inputs, outputs, successes, units
from nous.material.service import MaterialRecord, materials, observations, regions
from nous.platform.content import ContentLease
from nous.references import allocate_in, import_bindings_in

from .schema import bundle_installs

if TYPE_CHECKING:
    from .bundles import BundleService


async def install_zip(self: BundleService, ctx: CallContext, data: bytes):
    from .archive_read import decode

    bundle = await asyncio.to_thread(decode, data)
    return await self.install(
        ctx, InstallBundle(bundle=bundle.model_dump(mode="json"))
    )


async def install(
    self: BundleService,
    ctx: CallContext,
    command: InstallBundle,
    *,
    purpose: IdentityPurpose = "active",
):
    bundle = NamespaceBundle.model_validate(command.bundle)
    from .bundle_validation import validate_graph

    validate_graph(bundle)
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
        return await self.install_prepared(
            ctx,
            command,
            bundle,
            leases,
            purpose=purpose,
        )
    finally:
        if self.content_store:
            for lease in leases.values():
                await self.content_store.release(lease)


async def install_prepared(
    self: BundleService,
    ctx: CallContext,
    command: InstallBundle,
    original: NamespaceBundle,
    leases: dict[str, ContentLease],
    *,
    purpose: IdentityPurpose = "active",
):
    bundle = original.model_dump(mode="json")
    if bundle.get("format") != "nous.namespace.bundle" or bundle.get("version") != 2:
        raise DomainError("bundle_format", "Unsupported Nous namespace bundle format")
    records = [CognitiveRecord.model_validate(value) for value in bundle["cognition"]]
    obs = {row["id"]: row for row in bundle["observations"]}
    mats = {row["id"]: row for row in bundle["materials"]}
    keys = {record.reference.exact() for record in records}
    for record in records:
        # Empty source_roots are valid for system-formed cognition; only named roots must resolve.
        if any(oid not in obs for oid in record.source_roots):
            raise DomainError(
                "bundle_source_missing", "Cognition has an unresolved supporting source"
            )
        for ref in list(references_in(record.body)) + [
            link.reference for link in evidence_in(record) if link.reference
        ]:
            if ref.exact() not in keys:
                raise DomainError(
                    "bundle_reference_missing",
                    "Cognition has an unresolved exact revision",
                    reference=ref.exact(),
                )
        if any(
            link.observation_id and link.observation_id not in obs for link in evidence_in(record)
        ):
            raise DomainError(
                "bundle_source_missing", "Cognition evidence has an unresolved observation"
            )
    for row in obs.values():
        # NamespaceBundle has validated the shared ObservationKind, including
        # immutable snapshots read from registered external resources.
        if row["material_id"] not in mats:
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
    async with self.authority.write(ctx) as tx:
        await require_namespace_in(tx, ctx, "memory", purpose=purpose)
        if command.mode == "restore":
            if original.content_level != "full":
                raise DomainError(
                    "bundle_restore_full_required", "Restore requires a full namespace archive"
                )
            for table in (revisions, catalogs, observations):
                statement = select(table.c.id).where(table.c.namespace_id == ctx.namespace_id)
                if await tx.connection.scalar(statement.limit(1)) is not None:
                    raise DomainError(
                        "bundle_restore_nonempty", "Restore requires an empty receiving Namespace"
                    )
        digest = sha256(
            json.dumps(
                original.model_dump(mode="json", exclude={"id", "authority_commit_seq"}),
                sort_keys=True,
                ensure_ascii=False,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
        if original.package is not None and original.package.version is not None:
            registered = (
                await tx.connection.execute(
                    select(bundle_installs.c.record).where(
                        bundle_installs.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).scalars()
            for existing in registered:
                if (
                    existing.get("state") == "active"
                    and isinstance(existing.get("package"), dict)
                    and existing["package"].get("id") == original.package.id
                    and existing["package"].get("version") == original.package.version
                    and existing.get("placement") == command.placement.model_dump(mode="json")
                ):
                    if existing.get("content_digest") != digest:
                        raise DomainError(
                            "bundle_release_conflict",
                            "This package release already has different content in the receiving placement",
                        )
                    return existing
        formal_ids = {
            *(record.reference.id for record in records),
            *(record.reference.id for record in original.derivations.representations),
            *(region.id for region in original.derivations.regions),
            *obs,
            *mats,
            *(row["id"] for row in bundle.get("regions", [])),
        }
        runtime_rows = (
            *original.catalogs,
            *original.inputs,
            *original.units,
            *original.outputs,
            *original.file_successes,
        )
        mapping = {row.id: str(uuid4()) for row in runtime_rows}
        installation_id = str(uuid4())
        for binding in original.bindings:
            target = binding.target
            identity = (
                target.identity.id if isinstance(target, ObjectTarget) else target.reference.id
            )
            if "@" in identity:
                identity = identity.rsplit("@", 1)[0]
            if (identity not in formal_ids and identity not in mapping) or (
                isinstance(target, CognitiveTarget) and target.reference.exact() not in keys
            ):
                raise DomainError(
                    "bundle_reference_missing", "Readable binding points outside the bundle"
                )
        for archived in original.materials:
            new = {
                "id": archived.id,
                "namespace_id": ctx.namespace_id,
                "content_hash": archived.content_hash,
                "record": {**archived.record},
            }
            new["record"]["namespace_id"] = ctx.namespace_id
            new["record"]["commit_seq"] = tx.commit_seq
            new["record"]["addresses"] = (
                await allocate_in(tx, ctx, MediaReference(kind="material", id=archived.id))
            ).model_dump(mode="json")
            await tx.connection.execute(materials.insert().values(**new))
            if self.content_store and archived.content_hash in leases:
                await self.content_store.attach_in(
                    tx, leases[archived.content_hash], archived.id
                )
        for archived in original.regions:
            new = {**archived, "namespace_id": ctx.namespace_id, "record": {**archived["record"]}}
            new["record"]["addresses"] = (
                await allocate_in(tx, ctx, MediaReference(kind="source_region", id=new["id"]))
            ).model_dump(mode="json")
            SourceRegion.model_validate(new["record"])
            await tx.connection.execute(regions.insert().values(**new))
        for archived in original.observations:
            new = archived.model_dump(mode="json")
            new["namespace_id"] = ctx.namespace_id
            new["commit_seq"] = tx.commit_seq
            new["record"]["namespace_id"] = ctx.namespace_id
            new["record"]["commit_seq"] = tx.commit_seq
            new["record"]["placement"] = command.placement.model_dump(mode="json")
            await tx.connection.execute(observations.insert().values(**new))
        for archived in original.catalogs:
            record = {**archived.record}
            record["id"] = mapping[archived.id]
            record["namespace_id"] = ctx.namespace_id
            record["unit_ids"] = [mapping[identifier] for identifier in record["unit_ids"]]
            record["input_source_ids"] = [
                mapping[identifier] for identifier in record["input_source_ids"]
            ]
            await tx.connection.execute(
                catalogs.insert().values(
                    id=mapping[archived.id],
                    namespace_id=ctx.namespace_id,
                    record=record,
                )
            )
        for archived in original.inputs:
            record = {**archived.record}
            record["id"] = mapping[archived.id]
            record["catalog_id"] = mapping[archived.catalog_id]
            await tx.connection.execute(
                inputs.insert().values(
                    id=mapping[archived.id],
                    catalog_id=mapping[archived.catalog_id],
                    material_id=archived.material_id,
                    record=record,
                )
            )
        for archived in original.units:
            record = {**archived.record}
            record["id"] = mapping[archived.id]
            record["catalog_id"] = mapping[archived.catalog_id]
            record["input_source_id"] = mapping[record["input_source_id"]]
            await tx.connection.execute(
                units.insert().values(
                    id=mapping[archived.id],
                    catalog_id=mapping[archived.catalog_id],
                    record=record,
                )
            )
        if original.derivations.representations or original.derivations.regions:
            await self.cognition.derivations.install_in(
                tx, ctx, original.derivations, purpose=purpose
            )
        heads: dict[str, int] = {}
        for record in records:
            heads[record.reference.id] = max(
                heads.get(record.reference.id, 0), record.reference.revision
            )
        for record in records:
            adopted = record.model_copy(
                update={
                    "namespace_id": ctx.namespace_id,
                    "commit_seq": tx.commit_seq,
                    "addresses": await allocate_in(tx, ctx, record.reference),
                    "times": record.times.model_copy(
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
                    kind=adopted.reference.kind,
                    head=heads[adopted.reference.id] == adopted.reference.revision,
                    record=adopted.model_dump(mode="json"),
                    commit_seq=tx.commit_seq,
                )
            )
        vector_targets = {
            *keys,
            *(f"material:{identity}" for identity in mats),
            *(f"source-region:{row['id']}" for row in bundle.get("regions", [])),
            *(value.reference.media().exact() for value in original.derivations.representations),
            *(f"derived-region:{region.id}" for region in original.derivations.regions),
        }
        spaces: dict[str, tuple[str, int]] = {}
        for archived in original.vectors:
            if archived.reference not in vector_targets:
                raise DomainError(
                    "bundle_vector_reference", "A vector must identify an archived exact object"
                )
            signature = (archived.model, archived.dimensions)
            if spaces.setdefault(archived.space, signature) != signature:
                raise DomainError(
                    "embedding_space_conflict",
                    "An archived space mixes model identities or dimensions",
                )
            current = (
                await tx.connection.execute(
                    select(vectors.c.model, vectors.c.dimensions).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.space == archived.space)
                    .limit(1)
                )
            ).first()
            if current is not None and (current.model, current.dimensions) != signature:
                raise DomainError(
                    "embedding_space_conflict",
                    "Archived vectors conflict with the authority's existing space",
                )
            row = archived.model_dump(mode="json", exclude={"provenance"})
            row["commit_seq"] = tx.commit_seq
            await tx.connection.execute(vectors.insert().values(**{**row, "namespace_id": ctx.namespace_id}))
        for archived in original.outputs:
            output = archived.model_copy(
                update={
                    "id": mapping[archived.id],
                    "unit_id": mapping[archived.unit_id],
                    "attempt_job_id": None,
                    "commit_seq": tx.commit_seq,
                }
            )
            if output.observation_id not in obs or obs[output.observation_id]["kind"] != "import":
                raise DomainError(
                    "bundle_output_observation",
                    "UnitOutput must reference its real Import observation",
                )
            await tx.connection.execute(
                outputs.insert().values(
                    id=output.id,
                    unit_id=output.unit_id,
                    operation_id=installation_id,
                    record=output.model_dump(mode="json"),
                )
            )
        for archived in original.file_successes:
            record = {**archived.record}
            record.update(
                id=mapping[archived.id],
                catalog_id=mapping[record["catalog_id"]],
                input_source_id=mapping[archived.input_source_id],
                unit_outputs={
                    mapping[unit_id]: mapping[output_id]
                    for unit_id, output_id in record["unit_outputs"].items()
                },
                commit_seq=tx.commit_seq,
            )
            FileSuccess.model_validate(record)
            await tx.connection.execute(
                successes.insert().values(
                    id=mapping[archived.id],
                    namespace_id=ctx.namespace_id,
                    input_source_id=mapping[archived.input_source_id],
                    record=record,
                )
            )
        await import_bindings_in(
            tx,
            ctx,
            original.id,
            original.bindings,
        )
        result = {
            "installation_id": installation_id,
            "package": original.package.model_dump(mode="json") if original.package else None,
            "content_level": original.content_level,
            "mode": command.mode,
            "placement": command.placement.model_dump(mode="json"),
            "content_digest": digest,
            "source_namespace_id": original.namespace_id,
            "installed_at": tx.recorded_at,
            "state": "active",
            "owned_observation_ids": list(obs),
            "bundle_id": bundle["id"],
            "mapping": mapping,
            "vector_provenance": [
                {
                    "reference": archived.reference,
                    "space": archived.space,
                    "provenance": archived.provenance,
                    "source_namespace_id": original.namespace_id,
                }
                for archived in original.vectors
            ],
            "cognition_count": len(records),
            "output_count": len(original.outputs),
            "commit_seq": tx.commit_seq,
        }
        await tx.connection.execute(
            bundle_installs.insert().values(
                id=installation_id,
                namespace_id=ctx.namespace_id,
                record=result,
            )
        )
        return result
