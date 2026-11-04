"""Readable random addresses shared by independently owned records."""

import secrets
from collections.abc import Mapping

import sqlalchemy as sa

from nous.storage.database import WriteTx


def new_address(prefix: str) -> str:
    """Encode a random 48-bit value as three readable CVCVC groups."""
    consonants, vowels = "bdfghjklmnprstvz", "aiou"
    value = secrets.randbits(48)
    groups: list[str] = []
    for shift in (32, 16, 0):
        group = value >> shift
        groups.append(
            consonants[(group >> 12) & 15]
            + vowels[(group >> 10) & 3]
            + consonants[(group >> 6) & 15]
            + vowels[(group >> 4) & 3]
            + consonants[group & 15]
        )
    return prefix + ":" + "-".join(groups)


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
