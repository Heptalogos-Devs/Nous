"""Transactional request reservations shared across roles using one provider resource."""

import sqlalchemy as sa
from nous_sdk.contracts import DomainError
from sqlalchemy.dialects.sqlite import insert

from nous.platform.persistence import WriteTx, metadata

usage = sa.Table(
    "nous_model_resource_usage",
    metadata,
    sa.Column("resource", sa.String, primary_key=True),
    sa.Column("day", sa.String, primary_key=True),
    sa.Column("requests", sa.Integer, nullable=False),
)
allocations = sa.Table(
    "nous_model_budget_allocations",
    metadata,
    sa.Column("call_id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    sa.Column("role", sa.String, nullable=False),
    sa.Column("resource", sa.String, nullable=False),
    sa.Column("day", sa.String, nullable=False),
    sa.Column("requests", sa.Integer, nullable=False),
)


async def reserve_in(
    tx: WriteTx,
    call_id: str,
    role: str,
    resource: str,
    day: str,
    requests: int,
    role_maximum: int,
    resource_maximum: int,
) -> None:
    role_used = int(
        await tx.connection.scalar(
            sa.select(sa.func.coalesce(sa.func.sum(allocations.c.requests), 0)).where(
                allocations.c.namespace_id == tx.ctx.namespace_id,
                allocations.c.role == role,
                allocations.c.day == day,
            )
        )
        or 0
    )
    resource_used = int(
        await tx.connection.scalar(
            sa.select(usage.c.requests).where(usage.c.resource == resource, usage.c.day == day)
        )
        or 0
    )
    if role_used + requests > role_maximum or resource_used + requests > resource_maximum:
        raise DomainError(
            "model_budget_exhausted",
            "The task is waiting for its daily model request budget",
            role=role,
            requested=requests,
            role_remaining=max(0, role_maximum - role_used),
            resource_remaining=max(0, resource_maximum - resource_used),
        )
    await tx.connection.execute(
        insert(usage)
        .values(resource=resource, day=day, requests=requests)
        .on_conflict_do_update(
            index_elements=[usage.c.resource, usage.c.day],
            set_={"requests": usage.c.requests + requests},
        )
    )
    await tx.connection.execute(
        insert(allocations)
        .values(
            call_id=call_id,
            namespace_id=tx.ctx.namespace_id,
            role=role,
            resource=resource,
            day=day,
            requests=requests,
        )
        .on_conflict_do_update(
            index_elements=[allocations.c.call_id], set_={"day": day, "requests": requests}
        )
    )


async def settle_in(tx: WriteTx, call_id: str, actual_requests: int) -> None:
    row = (
        (
            await tx.connection.execute(
                sa.select(allocations).where(allocations.c.call_id == call_id)
            )
        )
        .mappings()
        .one()
    )
    difference = int(row["requests"]) - actual_requests
    await tx.connection.execute(
        usage.update()
        .where(usage.c.resource == row["resource"], usage.c.day == row["day"])
        .values(requests=usage.c.requests - difference)
    )
    await tx.connection.execute(
        allocations.update()
        .where(allocations.c.call_id == call_id)
        .values(requests=actual_requests)
    )
