"""Time-domain identity and its persistent coordinate binding."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from fractions import Fraction
from typing import Literal, cast

import sqlalchemy as sa
from pydantic import BaseModel, ConfigDict, field_serializer, model_validator
from sqlalchemy.engine import RowMapping

from nous.common.errors import NousError
from nous.common.values import ResourceId
from nous.storage.database import Database, WriteTx, time_domain_definitions

from .coordinates import validate_coordinates
from .values import Clock, CognitiveTimes, Instant, Interval, Known, TemporalValue, TimePoint


class TimeDomainDefinition(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    domain_id: ResourceId
    kind: Literal["utc", "linear", "opaque"]
    name: str
    tick_seconds: Fraction | None = None
    epoch: str | None = None

    @model_validator(mode="after")
    def validate_coordinates(self) -> TimeDomainDefinition:
        validate_coordinates(self.domain_id, self.kind, self.tick_seconds, self.epoch)
        return self

    @field_serializer("tick_seconds")
    def serialize_tick_seconds(self, value: Fraction | None) -> str | None:
        return str(value) if value is not None else None


UTC = TimeDomainDefinition(
    domain_id="utc", kind="utc", name="UTC", tick_seconds=Fraction(1, 1_000_000), epoch="Unix epoch"
)


def _definitions(configured: Mapping[str, object]) -> dict[str, TimeDomainDefinition]:
    definitions = {"utc": UTC}
    for domain_id, value in configured.items():
        if isinstance(value, BaseModel):
            data = value.model_dump()
        elif isinstance(value, Mapping):
            data = dict(cast(Mapping[str, object], value))
        else:
            raise NousError(
                "configuration.invalid_value",
                "Invalid time domain",
                details={"domain_id": domain_id},
            )
        try:
            definitions[domain_id] = TimeDomainDefinition.model_validate(
                {**data, "domain_id": domain_id}
            )
        except ValueError as exc:
            raise NousError(
                "configuration.invalid_value",
                "Invalid time domain",
                details={"domain_id": domain_id},
            ) from exc
    return definitions


def _from_row(values: RowMapping) -> TimeDomainDefinition:
    numerator = cast(str | None, values["tick_numerator"])
    denominator = cast(str | None, values["tick_denominator"])
    tick = (
        Fraction(int(numerator), int(denominator))
        if numerator is not None and denominator is not None
        else None
    )
    return TimeDomainDefinition(
        domain_id=cast(str, values["domain_id"]),
        kind=cast(Literal["utc", "linear", "opaque"], values["kind"]),
        name=cast(str, values["name"]),
        tick_seconds=tick,
        epoch=cast(str | None, values["epoch"]),
    )


def _assert_compatible(
    old: Mapping[str, TimeDomainDefinition], desired: Mapping[str, TimeDomainDefinition]
) -> None:
    for domain_id, new in desired.items():
        previous = old.get(domain_id)
        if previous is not None and (previous.kind, previous.tick_seconds, previous.epoch) != (
            new.kind,
            new.tick_seconds,
            new.epoch,
        ):
            raise NousError(
                "configuration.binding_conflict",
                "A registered time domain cannot change its coordinate definition; use a new domain ID",
                details={"domain_id": domain_id},
            )


async def check_domains(db: Database, configured: Mapping[str, object]) -> None:
    """Check a candidate against persisted definitions without writing it."""
    desired = _definitions(configured)
    async with db.read() as tx:
        rows = (await tx.connection.execute(sa.select(time_domain_definitions))).mappings().all()
    previous = {item.domain_id: item for item in map(_from_row, rows)}
    _assert_compatible(previous, desired)


class TimeService:
    """Runtime clock and definitions available to operations in this instance."""

    def __init__(self, db: Database, clock: Clock | None = None) -> None:
        self._db = db
        self.clock = clock or Clock()

    async def domains(self) -> dict[str, TimeDomainDefinition]:
        async with self._db.read() as tx:
            rows = (
                (await tx.connection.execute(sa.select(time_domain_definitions))).mappings().all()
            )
        return {item.domain_id: item for item in map(_from_row, rows)}

    async def validate(
        self, values: Iterable[CognitiveTimes | TemporalValue | TimePoint | None]
    ) -> None:
        """Validate every explicit coordinate against this instance's registered domains."""
        referenced: set[str] = set()

        def collect(value: CognitiveTimes | TemporalValue | TimePoint | None) -> None:
            if isinstance(value, TimePoint):
                referenced.add(value.domain_id)
            elif isinstance(value, CognitiveTimes):
                for temporal in (
                    value.occurred,
                    value.observed,
                    value.formed,
                    value.valid,
                    value.recorded,
                ):
                    collect(temporal)
            elif isinstance(value, Instant):
                collect(value.at)
            elif isinstance(value, Interval):
                for boundary in (value.start, value.end):
                    if isinstance(boundary, Known):
                        collect(boundary.at)

        for value in values:
            collect(value)
        if referenced:
            unknown = referenced - (await self.domains()).keys()
            if unknown:
                raise NousError(
                    "time.domain_unknown",
                    "Time coordinates refer to unregistered domains",
                    details={"domains": sorted(unknown)},
                )


async def register_domain(definition: TimeDomainDefinition, tx: WriteTx) -> TimeDomainDefinition:
    """Register a domain within the caller's transaction without committing it."""
    result = await tx.connection.execute(
        sa.select(time_domain_definitions).where(
            time_domain_definitions.c.domain_id == definition.domain_id
        )
    )
    row = result.mappings().one_or_none()
    previous = _from_row(row) if row is not None else None
    if previous is not None:
        _assert_compatible({previous.domain_id: previous}, {definition.domain_id: definition})
        if previous.name != definition.name:
            await tx.connection.execute(
                time_domain_definitions.update()
                .where(time_domain_definitions.c.domain_id == definition.domain_id)
                .values(name=definition.name)
            )
    else:
        tick = definition.tick_seconds
        await tx.connection.execute(
            time_domain_definitions.insert().values(
                domain_id=definition.domain_id,
                kind=definition.kind,
                name=definition.name,
                tick_numerator=str(tick.numerator) if tick is not None else None,
                tick_denominator=str(tick.denominator) if tick is not None else None,
                epoch=definition.epoch,
            )
        )
    return definition


async def bind_domains(
    db: Database, configured: Mapping[str, object]
) -> dict[str, TimeDomainDefinition]:
    """Register new definitions and update names in a single write transaction."""
    desired = _definitions(configured)
    async with db.write() as tx:
        rows = (await tx.connection.execute(sa.select(time_domain_definitions))).mappings().all()
        previous = {item.domain_id: item for item in map(_from_row, rows)}
        _assert_compatible(previous, desired)
        for value in desired.values():
            await register_domain(value, tx)
        return {**previous, **desired}
