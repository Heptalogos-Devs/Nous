"""Reusable, reversible random identifier suffixes from fixed research codebooks.

All profiles use CSPRNG output. A database uniqueness constraint and collision
retry remain necessary. These identifiers are not authentication credentials.
See REPORT.md for tradeoffs and data attribution.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import secrets
from pathlib import Path

CODEBOOK_PATH = Path(__file__).with_name('codebooks.json')

# A book's exact contents and order are part of the encoding contract.
BOOK_DIGESTS = {
    'readable': '3f94e76bc1cb63be41a80428fcd894dae82ab77bedf67569d867e42be8c0f7c4',
    'plain3': '3dd949c32fff5e0f0751f9ed17cdb72fb7046d61b6eff0a4d5a8a5d46c58f3be',
    'fixed_first': 'bef1c9bf1cece3bee30efce9ca696e1cbfff4f81f81d1d0b59c71a3d15c2e5f9',
    'fixed_rest': 'd6bff77dcee76bbd778cdb1a62e1fbdf4e2a40a22b28a273f833c08b877c871b',
    'variable_first': '0a6e6264cddf136f7874447902b1ded4f9b528d981ca3a7649d9ad93f9828e4d',
    'variable_rest': '28a8aa84673d306dca21b0cf59191e39f0809c01fe219f4acce1941c44afc99c',
    'unicode_first': 'd61f5c54804da129d4d34d085db3cbc59495676d1a6ae2dc6cb915817d0f5f82',
    'unicode_rest': 'daf5937f5b7098fc452e74725b1da23a8d24c8509246be9178e2473cf6fb9775',
}

# Each pair selects a book and the number of bits consumed in that position.
PROFILES = {
    'readable60': (('readable', 10),) * 6,
    'no-space60': (('plain3', 10),) * 6,
    'fixed60': (('fixed_first', 5),) + (('fixed_rest', 11),) * 5,
    'fixed65': (('fixed_first', 10),) + (('fixed_rest', 11),) * 5,
    'variable60': (('variable_first', 8),) + (('variable_rest', 13),) * 4,
    'variable63': (('variable_first', 11),) + (('variable_rest', 13),) * 4,
    'unicode60': (('unicode_first', 15),) + (('unicode_rest', 15),) * 3,
}


class IdentifierCodec:
    """Encode and validate one profile using its fixed, ordered word tables."""

    def __init__(self, profile: str, codebook_path: Path = CODEBOOK_PATH) -> None:
        try:
            self.profile = PROFILES[profile]
        except KeyError as exc:
            raise ValueError(f'unknown profile: {profile}') from exc
        self.name = profile
        self.bit_width = sum(bits for _, bits in self.profile)
        raw = json.loads(codebook_path.read_text(encoding='utf-8'))
        self.books: dict[str, tuple[str, ...]] = {}
        self.indices: dict[str, dict[str, int]] = {}
        for name, bits in self.profile:
            if name in self.books:
                continue
            words = tuple(raw[name])
            digest = hashlib.sha256(('\n'.join(words) + '\n').encode('utf-8')).hexdigest()
            indices = {word: index for index, word in enumerate(words)}
            if digest != BOOK_DIGESTS[name] or len(indices) != len(words) or len(words) < 1 << bits:
                raise ValueError(f'codebook {name} has unexpected contents or order')
            self.books[name] = words
            self.indices[name] = indices

    def encode(self, value: int) -> str:
        """Encode an integer into the selected suffix format."""
        if type(value) is not int or not 0 <= value < 1 << self.bit_width:
            raise ValueError(f'value must be an unsigned {self.bit_width}-bit integer')
        parts: list[str] = []
        remaining = self.bit_width
        for book, bits in self.profile:
            remaining -= bits
            parts.append(self.books[book][(value >> remaining) & ((1 << bits) - 1)])
        separator = '' if self.name in {'readable60', 'no-space60'} else ' '
        return separator.join(parts)

    def decode(self, suffix: str) -> int:
        """Reject malformed suffixes and recover the encoded integer."""
        if self.name in {'readable60', 'no-space60'}:
            width = 4 if self.name == 'readable60' else 3
            if len(suffix) != width * len(self.profile):
                raise ValueError('invalid suffix length')
            parts = [suffix[i:i + width] for i in range(0, len(suffix), width)]
        else:
            parts = suffix.split(' ')
        if len(parts) != len(self.profile):
            raise ValueError('invalid number of units')
        value = 0
        for part, (book, bits) in zip(parts, self.profile, strict=True):
            try:
                index = self.indices[book][part]
            except KeyError as exc:
                raise ValueError('unknown suffix unit') from exc
            if index >= 1 << bits:
                raise ValueError('unit is outside this profile')
            value = (value << bits) | index
        return value

    def generate(self) -> str:
        """Generate a uniformly random suffix with the profile's bit width."""
        return self.encode(secrets.randbits(self.bit_width))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description='Generate a random identifier suffix')
    parser.add_argument('profile', choices=PROFILES, nargs='?', default='readable60')
    args = parser.parse_args()
    print(IdentifierCodec(args.profile).generate())
