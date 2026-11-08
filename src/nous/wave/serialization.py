"""Canonical serialization and signatures for Wave assets."""

from __future__ import annotations

import hashlib
import json


def encode_json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def content_signature(value: object) -> str:
    return hashlib.sha256(encode_json(value).encode("utf-8")).hexdigest()
