"""Adapted from crates/nous-domain/src/references/proquint.rs.

Copyright 2026 Aravine Zhu. Licensed under Apache-2.0; the license text is
distributed in nous/resources/licenses/Nous-LICENSE-APACHE.
"""

CONSONANTS = "bdfghjklmnprstvz"
VOWELS = "aiou"


def encode(value: int) -> str:
    if not 0 <= value < 1 << 48:
        raise ValueError("Proquint address exceeds 48 bits")
    groups: list[str] = []
    for shift in (32, 16, 0):
        group = value >> shift
        groups.append(
            CONSONANTS[(group >> 12) & 15]
            + VOWELS[(group >> 10) & 3]
            + CONSONANTS[(group >> 6) & 15]
            + VOWELS[(group >> 4) & 3]
            + CONSONANTS[group & 15]
        )
    return "-".join(groups)


def decode(value: str) -> int:
    if len(value) != 17 or value[5] != "-" or value[11] != "-":
        raise ValueError("Invalid Proquint reference")
    result = 0
    for word in value.split("-"):
        group = 0
        for character, alphabet, width in zip(
            word, (CONSONANTS, VOWELS, CONSONANTS, VOWELS, CONSONANTS), (4, 2, 4, 2, 4), strict=True
        ):
            group = (group << width) | alphabet.index(character)
        result = (result << 16) | group
    return result


if __name__ == "__main__":
    print(encode(1234567890))
    print(decode("badif-guhaj-kilom"))
