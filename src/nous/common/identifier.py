"""Readable random identifiers shared by independently owned records."""

import re
import secrets
from collections.abc import Mapping

import sqlalchemy as sa

from nous.common._identifier_words import FIRST_WORDS, REMAINING_WORDS
from nous.common.errors import NousError
from nous.storage.database import WriteTx

_IDENTIFIER = re.compile(r"([a-z][a-z0-9]*)\((?:[A-Za-z]{4} ){5}[A-Za-z]{4}\)")


def new_identifier(prefix: str) -> str:
    """Encode 65 random bits as a type and six fixed-width units in parentheses."""
    value = secrets.randbits(65)
    groups = [FIRST_WORDS[value >> 55]]
    groups.extend(REMAINING_WORDS[(value >> shift) & 2047] for shift in (44, 33, 22, 11, 0))
    return f"{prefix}({' '.join(groups)})"


def identifier_type(value: str) -> str | None:
    """Return the type of a complete identifier, or None when its syntax does not match.

    This checks the identifier shape without looking up an object. Revision suffixes
    belong outside the identifier and must be separated before calling this function.
    """
    match = _IDENTIFIER.fullmatch(value)
    return match.group(1) if match else None


def split_revision(value: str) -> tuple[str, int | None]:
    """Separate an optional positive ``@N`` revision from an identifier or internal ID.

    Raises:
        NousError: The revision suffix is missing, malformed, or not positive.
    """
    target, marker, suffix = value.rpartition("@")
    if not marker:
        return value, None
    if not target or not suffix.isascii() or not suffix.isdecimal() or int(suffix) < 1:
        raise NousError("input.invalid_reference", "Revision ordinal must be a positive integer")
    return target, int(suffix)


async def insert_identifier(
    tx: WriteTx, table: sa.Table, prefix: str, values: Mapping[str, object]
) -> str:
    """Allocate and insert a readable identity in the caller's transaction."""
    from sqlalchemy.dialects.sqlite import insert

    for _ in range(32):
        identifier = new_identifier(prefix)
        result = await tx.connection.execute(
            insert(table)
            .values(**values, identifier=identifier)
            .on_conflict_do_nothing(index_elements=[table.c.identifier])
        )
        if result.rowcount == 1:
            return identifier
    raise NousError("storage.identifier_exhausted", "Cannot allocate a unique readable identifier")
