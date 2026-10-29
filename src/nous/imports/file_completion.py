"""Publish whole-file success from actual completed unit attempts, across requests."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import uuid4

from nous_wave.contracts import (
    CallContext,
    CatalogView,
    FileSuccess,
    ImportDocument,
    ImportUnit,
    ProcessCatalog,
)
from pydantic import TypeAdapter
from sqlalchemy import delete, select

from .continuation import request_profiles_in, unit_candidates_in
from .schema import inputs, manifest, successes, units

if TYPE_CHECKING:
    from .service import ImportService


async def complete_files(
    self: ImportService,
    ctx: CallContext,
    command: ProcessCatalog,
    catalog: CatalogView,
) -> list[FileSuccess]:
    selected = (
        set(command.unit_ids)
        if command.unit_ids is not None
        else {unit.id for unit in catalog.units}
    )
    touched = {unit.input_source_id for unit in catalog.units if unit.id in selected}
    result: list[FileSuccess] = []
    async with self.authority.write(
        ctx,
        command.operation_id + ":complete",
        operation_kind="imports.complete",
        scope=ctx.require_subject(),
        payload=command,
    ) as tx:
        if tx.replayed:
            return TypeAdapter(list[FileSuccess]).validate_python(tx.replay_result)
        for source_id in sorted(touched):
            if any(error.input_source_id == source_id for error in catalog.errors):
                continue
            file_units = tuple(
                ImportUnit.model_validate(value)
                for value in (
                    await tx.connection.execute(
                        select(units.c.record).where(
                            units.c.catalog_id == catalog.id,
                            units.c.record["input_source_id"].as_string() == source_id,
                        )
                    )
                ).scalars()
            )
            source = (
                await tx.connection.execute(select(inputs.c.record).where(inputs.c.id == source_id))
            ).scalar_one()
            expected = source.get("original_unit_count")
            if expected is None and len(catalog.units) == catalog.original_unit_count:
                expected = sum(unit.input_source_id == source_id for unit in catalog.units)
            if (
                not file_units
                or expected != len(file_units)
                or any(unit.purged for unit in file_units)
            ):
                continue
            file_catalog = catalog.model_copy(update={"units": file_units})
            history = await unit_candidates_in(
                self, tx, ctx, file_catalog, command, {unit.id for unit in file_units}
            )
            completed = {
                unit.id: next(
                    (item.output for item in history.get(unit.id, ()) if item.complete), None
                )
                for unit in file_units
            }
            if any(output is None for output in completed.values()):
                continue
            chosen = {
                unit_id: output.id for unit_id, output in completed.items() if output is not None
            }
            document = ImportDocument.model_validate(source["document"])
            origin = command.origin if command.origin is not None else document.origin
            existing = (
                await tx.connection.execute(
                    select(successes.c.record).where(
                        successes.c.namespace_id == ctx.namespace_id,
                        successes.c.subject_id == ctx.require_subject(),
                        successes.c.input_source_id == source_id,
                    )
                )
            ).scalars()
            fact = next(
                (
                    FileSuccess.model_validate(value)
                    for value in existing
                    if value["unit_outputs"] == chosen and value["origin"] == origin
                ),
                None,
            )
            if fact is None:
                fact = FileSuccess(
                    id=str(uuid4()),
                    catalog_id=catalog.id,
                    input_source_id=source_id,
                    unit_outputs=chosen,
                    content_hash=source["content_hash"],
                    file_identity=document.file_identity,
                    origin=origin,
                    commit_seq=tx.commit_seq,
                )
                await tx.connection.execute(
                    successes.insert().values(
                        id=fact.id,
                        namespace_id=ctx.namespace_id,
                        subject_id=ctx.require_subject(),
                        input_source_id=source_id,
                        record=fact.model_dump(mode="json"),
                    )
                )
            # An admission's original qualification can complete in a later process
            # request. Recheck its real policy before moving its captured manifest key.
            for row, admission_profile in await request_profiles_in(tx, ctx, catalog.id):
                if row.get("kind") != "files":
                    continue
                captured = row.get("manifest_keys")
                if not isinstance(captured, dict) or not isinstance(captured.get(source_id), str):
                    continue
                eligible = await unit_candidates_in(
                    self, tx, ctx, file_catalog, admission_profile, set(chosen)
                )
                if not all(
                    any(
                        item.complete and item.output.id == chosen[unit.id]
                        for item in eligible.get(unit.id, ())
                    )
                    for unit in file_units
                ):
                    continue
                key = captured[source_id]
                await tx.connection.execute(delete(manifest).where(manifest.c.key == key))
                await tx.connection.execute(manifest.insert().values(key=key, success_id=fact.id))
            result.append(fact)
        tx.result(result)
    return result
