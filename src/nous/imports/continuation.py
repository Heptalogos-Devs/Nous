"""Reuse attested unit adoption and completion under the requested processing policy."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    AdmitFiles,
    CallContext,
    CatalogView,
    DomainError,
    ImportUnit,
    ProcessCatalog,
    UnitOutput,
)
from nous_wave.contracts.imports import UnitAttempt
from pydantic import JsonValue
from sqlalchemy import or_, select

from nous.cognition.graph_state import heads_in
from nous.cognition.schema import vectors
from nous.material.service import require_observation_in
from nous.platform.execution import require_job_in
from nous.platform.persistence import ReadView, WriteTx

from .attempts import attempts_in
from .schema import outputs, requests

if TYPE_CHECKING:
    from .service import ImportService


def formation_options(unit: ImportUnit, command: ProcessCatalog) -> dict[str, JsonValue]:
    return {
        **unit.formation_options,
        **(
            command.formation_options_override.model_dump(mode="json", exclude_unset=True)
            if command.formation_options_override
            else {}
        ),
    }


def same_content(unit: ImportUnit, previous: ProcessCatalog, current: ProcessCatalog) -> bool:
    if previous.deferred_formation != current.deferred_formation:
        return False
    if unit.requires_llm and previous.llm_enabled != current.llm_enabled:
        return False
    if (
        unit.requires_llm
        and current.llm_enabled
        and formation_options(unit, previous) != formation_options(unit, current)
    ):
        return False
    return not current.deferred_formation or previous.vectors == current.vectors


def process_request(row: dict[str, JsonValue]) -> ProcessCatalog | None:
    value = row.get("command")
    if not isinstance(value, dict):
        return None
    if row.get("kind") == "files":
        source = AdmitFiles.model_validate(value)
        catalog_id = row.get("catalog_id")
        if not isinstance(catalog_id, str):
            return None
        return ProcessCatalog(
            operation_id=source.operation_id + ":process",
            force=source.force,
            catalog_id=catalog_id,
            deferred_formation=source.deferred_formation,
            llm_enabled=source.llm_enabled,
            formation_retries=source.formation_retries,
            vectors=source.vectors,
        )
    return ProcessCatalog.model_validate(value) if "catalog_id" in value else None


async def request_profiles_in(
    view: ReadView | WriteTx, ctx: CallContext, catalog_id: str
) -> list[tuple[dict[str, JsonValue], ProcessCatalog]]:
    rows = (
        await view.connection.execute(
            select(requests.c.record).where(
                requests.c.record["context"]["namespace_id"].as_string() == ctx.namespace_id,
                requests.c.record["context"]["subject_id"].as_string() == ctx.require_subject(),
                or_(
                    requests.c.record["command"]["catalog_id"].as_string() == catalog_id,
                    requests.c.record["catalog_id"].as_string() == catalog_id,
                ),
            )
        )
    ).scalars()
    result: list[tuple[dict[str, JsonValue], ProcessCatalog]] = []
    for row in rows:
        if row.get("purged"):
            continue
        command = process_request(row)
        if command is not None:
            result.append((row, command))
    return result


async def validate_retry_in(
    view: ReadView | WriteTx, ctx: CallContext, command: ProcessCatalog
) -> None:
    if command.retry_parent_job_id is None:
        return
    await require_job_in(view, ctx, command.retry_parent_job_id)
    profiles = await request_profiles_in(view, ctx, command.catalog_id)
    if not any(row.get("job_id") == command.retry_parent_job_id for row, _ in profiles):
        raise DomainError("import_retry_mismatch", "Retry parent is not processing this catalog")


async def vectors_complete_in(view: ReadView | WriteTx, output: UnitOutput) -> bool:
    requested = {ref.exact() for ref in output.cognition}
    if not requested:
        return False
    rows = (
        await view.connection.execute(
            select(vectors.c.reference, vectors.c.space).where(vectors.c.reference.in_(requested))
        )
    ).all()
    spaces: dict[str, set[str]] = {}
    for reference, space in rows:
        spaces.setdefault(str(space), set()).add(str(reference))
    return any(requested.issubset(present) for present in spaces.values())


@dataclass(frozen=True)
class UnitCandidate:
    output: UnitOutput
    attempt: UnitAttempt
    complete: bool


async def unit_candidates_in(
    self: ImportService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    catalog: CatalogView,
    command: ProcessCatalog,
    unit_ids: set[str],
) -> dict[str, list[UnitCandidate]]:
    profiles = await request_profiles_in(view, ctx, catalog.id)
    attempts = await attempts_in(view, ctx, catalog.id, unit_ids)
    by_unit = {unit.id: unit for unit in catalog.units if unit.id in unit_ids}
    output_ids = {item.attempt.output_id for item in attempts if item.attempt.output_id}
    values = (
        await view.connection.execute(select(outputs.c.record).where(outputs.c.id.in_(output_ids)))
    ).scalars()
    retained = {value["id"]: UnitOutput.model_validate(value) for value in values}
    kinds = tuple({ref.kind for value in retained.values() for ref in value.cognition})
    reference_ids = tuple({ref.id for value in retained.values() for ref in value.cognition})
    heads = (
        {
            record.reference.id: record
            for record in await heads_in(view, ctx, kinds, reference_ids=reference_ids)
        }
        if kinds
        else {}
    )
    result: dict[str, list[UnitCandidate]] = {}
    for captured in reversed(attempts):
        attempt = captured.attempt
        output = retained.get(attempt.output_id or "")
        unit = by_unit[attempt.unit_id]
        previous = next(
            (
                profile
                for row, profile in profiles
                if row.get("job_id") == attempt.job_id
                and profile.operation_id == attempt.operation_id
                and CallContext.model_validate(row["context"]).caller_id == captured.caller_id
            ),
            None,
        )
        if (
            output is None
            or output.unit_id != unit.id
            or previous is None
            or not same_content(unit, previous, command)
        ):
            continue
        try:
            accepted = (
                await require_observation_in(
                    view, ctx, unit.observation_id or "", region_id=unit.source_region_id
                )
            )["record"]
            event = (
                await require_observation_in(
                    view, ctx, output.observation_id, region_id=unit.source_region_id
                )
            )["record"]
            if event["material_id"] != unit.material_id or event["origin"] != (
                command.origin if command.origin is not None else accepted["origin"]
            ):
                continue
            valid = True
            for reference in output.cognition:
                record = await self.cognition.get_in(view, ctx, reference)
                head = heads.get(reference.id)
                if record.purged or record.deleted or head is None or head.deleted or head.purged:
                    valid = False
                    break
            if not valid or not output.cognition:
                continue
        except DomainError as error:
            if error.code not in {
                "source_unavailable",
                "source_purged",
                "cognition_not_found",
                "region_not_found",
            }:
                raise
            continue
        complete = attempt.status == "succeeded" and attempt.stage == "complete"
        if not command.deferred_formation:
            rank = {"disabled": 0, "metadata_allowed": 1, "required": 2}
            complete = complete and rank[previous.vectors] >= rank[command.vectors]
            if command.vectors == "required":
                complete = complete and await vectors_complete_in(view, output)
        result.setdefault(unit.id, []).append(UnitCandidate(output, attempt, complete))
    return result
