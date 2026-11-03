"""Readable random addresses shared by independently owned records."""

import secrets


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
