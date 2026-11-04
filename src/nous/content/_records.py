"""Transactional identities shared by content record owners."""

from __future__ import annotations

from collections.abc import Mapping
from datetime import datetime

import sqlalchemy as sa
from sqlalchemy.dialects.sqlite import insert

from nous.common.address import new_address
from nous.common.errors import NousError
from nous.storage.database import WriteTx


async def insert_address(
    tx: WriteTx, table: sa.Table, prefix: str, values: Mapping[str, object]
) -> str:
    for _ in range(32):
        address = new_address(prefix)
        result = await tx.connection.execute(
            insert(table)
            .values(**values, address=address)
            .on_conflict_do_nothing(index_elements=[table.c.address])
        )
        if result.rowcount == 1:
            return address
    raise NousError("content.address_exhausted", "Cannot allocate a unique readable address")


def date_value(value: object) -> datetime | None:
    return datetime.fromisoformat(str(value)) if value else None
