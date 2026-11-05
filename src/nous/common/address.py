"""Readable random addresses shared by independently owned records."""

import secrets
from collections.abc import Mapping

import sqlalchemy as sa

from nous.common._address_words import FIRST_WORDS, REMAINING_WORDS
from nous.storage.database import WriteTx


def new_address(prefix: str) -> str:
    """Encode 65 random bits as six fixed-width ordinary-token units."""
    value = secrets.randbits(65)
    groups = [FIRST_WORDS[value >> 55]]
    groups.extend(REMAINING_WORDS[(value >> shift) & 2047] for shift in (44, 33, 22, 11, 0))
    return prefix + ":" + " ".join(groups)


async def insert_address(
    tx: WriteTx, table: sa.Table, prefix: str, values: Mapping[str, object]
) -> str:
    """Allocate and insert a readable identity in the caller's transaction."""
    from sqlalchemy.dialects.sqlite import insert

    from nous.common.errors import NousError

    for _ in range(32):
        address = new_address(prefix)
        result = await tx.connection.execute(
            insert(table)
            .values(**values, address=address)
            .on_conflict_do_nothing(index_elements=[table.c.address])
        )
        if result.rowcount == 1:
            return address
    raise NousError("storage.address_exhausted", "Cannot allocate a unique readable address")
