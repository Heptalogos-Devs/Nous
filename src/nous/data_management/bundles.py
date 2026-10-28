"""Logical subject bundles preserve historical facts without replaying old jobs."""

from __future__ import annotations

import asyncio
import base64
import json
from hashlib import sha256
from io import BytesIO
from uuid import uuid4
from zipfile import ZIP_DEFLATED, ZipFile

from nous_wave.contracts import (
    BundleRequest,
    CallContext,
    CatalogView,
    CognitiveRecord,
    DomainError,
    FileSuccess,
    InstallBundle,
    SourceCommand,
    SourceRegion,
    SubjectBundle,
    UnitOutput,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import JSON, Column, String, Table, select, update

from nous.cognition.service import CognitionService, references_in, revisions
from nous.identity.service import require_subject_in
from nous.imports.service import catalogs, inputs, outputs, successes, units
from nous.material.service import MaterialRecord, materials, observations, regions, source_permitted
from nous.platform.content import ContentLease, ContentObject, ContentStore
from nous.platform.persistence import Authority, metadata

bundle_installs = Table(
    "bundle_installs",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False),
    Column("subject_id", String, nullable=False),
    Column("record", JSON, nullable=False),
)


class BundleService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        content_store: ContentStore | None = None,
    ) -> None:
        self.authority, self.cognition = authority, cognition
        self.content_store = content_store

    async def export(self, ctx: CallContext, command: BundleRequest | None = None):
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
            chosen = (
                {reference.exact() for reference in command.references}
                if command.references is not None
                else set(all_records)
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
                    oid in source_rows
                    and source_permitted(ctx, source_rows[oid]["material_id"], oid)
                    for oid in record.source_roots
                ):
                    raise DomainError(
                        "source_unavailable", "Bundle includes sources outside the current scope"
                    )
                dependencies = list(references_in(record.body.model_dump(mode="json") if record.body else None)) + [
                    link.reference for link in record.evidence if link.reference
                ]
                for reference in dependencies:
                    if reference.exact() not in chosen:
                        chosen.add(reference.exact())
                        pending.append(reference.exact())
            records = [all_records[key] for key in sorted(chosen)]
            observation_ids = {oid for record in records for oid in record.source_roots}
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
            obs = [source_rows[oid] for oid in sorted(observation_ids)]
            material_ids = {row["material_id"] for row in obs}
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
            result = TypeAdapter(dict[str, JsonValue]).validate_python({
                "format": "nous.subject",
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
            })

            region_ids = {
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
                    content = ContentObject(
                        material["content_hash"], material["byte_len"], material["media_type"]
                    )
                    async with self.content_store.protect(content) as path:
                        contents[content.content_hash] = base64.b64encode(
                            await asyncio.to_thread(path.read_bytes)
                        ).decode("ascii")
            result["contents"] = contents
            return SubjectBundle.model_validate(result).model_dump(mode="json")

    async def export_zip(self, ctx: CallContext, command: BundleRequest | None = None) -> bytes:
        bundle = await self.export(ctx, command)
        buffer = BytesIO()
        with ZipFile(buffer, "w", ZIP_DEFLATED) as archive:
            archive.writestr("bundle.json", json.dumps(bundle, ensure_ascii=False))
        return buffer.getvalue()

    async def install_zip(self, ctx: CallContext, data: bytes, operation_id: str):
        with ZipFile(BytesIO(data)) as archive:
            info = archive.getinfo("bundle.json")
            if info.file_size > 64 * 1024 * 1024:
                raise DomainError("bundle_too_large", "Bundle JSON exceeds 64 MiB")
            bundle = json.loads(archive.read(info))
        return await self.install(ctx, InstallBundle(operation_id=operation_id, bundle=bundle))

    async def install(self, ctx: CallContext, command: InstallBundle):
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
            return await self._install_prepared(ctx, command, bundle, leases)
        finally:
            if self.content_store:
                for lease in leases.values():
                    await self.content_store.release(lease)

    async def _install_prepared(
        self,
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
            for ref in list(references_in(record.body.model_dump(mode="json") if record.body else None)) + [
                link.reference for link in record.evidence if link.reference
            ]:
                if ref.exact() not in keys:
                    raise DomainError(
                        "bundle_reference_missing",
                        "Cognition has an unresolved exact revision",
                        reference=ref.exact(),
                    )
            if any(
                link.observation_id and link.observation_id not in obs for link in record.evidence
            ):
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
                if field in identity_fields or field in {
                    "source_roots",
                    "support_roots",
                    "input_source_ids",
                }:
                    return mapping.get(value, value) if isinstance(value, str) else value
                return value

            remapped = SubjectBundle.model_validate(remap(bundle)).model_dump(mode="json")
            for new in remapped["materials"]:
                new["record"]["commit_seq"] = tx.commit_seq
                await tx.connection.execute(materials.insert().values(**new))
                if self.content_store and new["content_hash"] in leases:
                    await self.content_store.attach_in(tx, leases[new["content_hash"]], new["id"])
            for new in remapped["regions"]:
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

    async def list_installs(self, ctx: CallContext):
        async with self.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
            return [row for row in (await view.connection.execute(select(bundle_installs.c.record).where(bundle_installs.c.namespace_id == ctx.namespace_id, bundle_installs.c.subject_id == ctx.subject_id))).scalars()]

    async def inspect_zip(self, ctx: CallContext, data: bytes):
        ctx.require_subject()
        with ZipFile(BytesIO(data)) as archive:
            info = archive.getinfo("bundle.json")
            if info.file_size > 64 * 1024 * 1024:
                raise DomainError("bundle_too_large", "Bundle JSON exceeds 64 MiB")
            bundle = SubjectBundle.model_validate_json(archive.read(info))
        for row in bundle.materials:
            material = TypeAdapter(MaterialRecord).validate_python(row.record)
            encoded = bundle.contents.get(row.content_hash)
            content = base64.b64decode(encoded, validate=True) if encoded is not None else material["text"].encode() if material["text"] is not None else None
            if content is not None and (sha256(content).hexdigest() != row.content_hash or len(content) != material["byte_len"]):
                raise DomainError("bundle_content_hash", "Bundle material content does not match its immutable identity")
        return {"format": bundle.format, "version": bundle.version, "id": bundle.id, "cognition": len(bundle.cognition), "materials": len(bundle.materials), "observations": len(bundle.observations), "units": len(bundle.units), "outputs": len(bundle.outputs), "file_successes": len(bundle.file_successes), "regions": len(bundle.regions), "bytes": sum(TypeAdapter(MaterialRecord).validate_python(row.record)["byte_len"] for row in bundle.materials)}

    async def uninstall(self, ctx: CallContext, installation_id: str, operation_id: str):
        from .sources import SourceService
        sources = SourceService(self.authority, self.cognition)
        async with self.authority.write(ctx, operation_id, operation_kind="bundle.uninstall", scope=ctx.require_subject(), payload={"installation_id": installation_id}) as tx:
            if tx.replayed:
                return tx.replay_result
            value = (await tx.connection.execute(select(bundle_installs.c.record).where(bundle_installs.c.id == installation_id, bundle_installs.c.namespace_id == ctx.namespace_id, bundle_installs.c.subject_id == ctx.subject_id))).scalar_one_or_none()
            if value is None:
                raise DomainError("bundle_install_not_found", "Bundle installation is unavailable")
            if value.get("state") == "uninstalled":
                return tx.result({"installation_id": installation_id, "state": "uninstalled", "already_uninstalled": True})
            owned = tuple(value.get("owned_observation_ids", ()))
            affected: list[str] = []
            if owned:
                result, _detached = await sources.change_in(tx, ctx, SourceCommand(operation_id=operation_id, observation_ids=owned, reason=f"uninstall bundle {value['bundle_id']}"), "trash")
                affected = [ref.exact() for ref in result.affected]
            value = {**value, "state": "uninstalled", "uninstall_operation_id": operation_id, "uninstalled_commit_seq": tx.commit_seq}
            await tx.connection.execute(update(bundle_installs).where(bundle_installs.c.id == installation_id).values(record=value))
            return tx.result({"installation_id": installation_id, "state": "uninstalled", "affected": affected, "commit_seq": tx.commit_seq})
