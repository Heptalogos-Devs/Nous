"""Stable semantic keys follow A-Memorix's actual full-state validator."""

import re
from hashlib import sha256

from nous_wave.contracts import AssociationBody, EntityBody, MemoryBody, SubjectBundle


def paragraph_key(text: str) -> str:
    text = text.strip().replace("\r\n", "\n").replace("\r", "\n")
    return sha256(re.sub(r"[^\S\n]+", " ", text).encode()).hexdigest()


def entity_key(name: str) -> str:
    return sha256(name.strip().lower().encode()).hexdigest()


def relation_key(subject: str, predicate: str, target: str) -> str:
    return sha256(
        "|".join(value.strip().lower() for value in (subject, predicate, target)).encode()
    ).hexdigest()


def semantic_ids(bundle: SubjectBundle) -> dict[str, str]:
    exacts = {value.reference.exact(): value for value in bundle.cognition}
    result: dict[str, str] = {}
    for value in sorted(bundle.cognition, key=lambda value: value.reference.revision):
        body = value.body
        if isinstance(body, MemoryBody):
            result[value.reference.id] = paragraph_key(body.text)
        elif isinstance(body, EntityBody):
            result[value.reference.id] = entity_key(body.label)
        elif isinstance(body, AssociationBody):
            left, right = exacts.get(body.from_ref.exact()), exacts.get(body.to_ref.exact())
            if (
                left
                and right
                and isinstance(left.body, EntityBody)
                and isinstance(right.body, EntityBody)
            ):
                result[value.reference.id] = relation_key(
                    left.body.label, body.relation, right.body.label
                )
        else:
            result[value.reference.id] = value.reference.id
    return result
