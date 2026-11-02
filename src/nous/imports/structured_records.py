"""Explicit source entities and binary assertions become the native formation contract."""

from __future__ import annotations

from typing import Literal

from nous_sdk.contracts import (
    EpistemicStance,
    ProposedAssociation,
    ProposedEntity,
)
from nous.platform.errors import DomainError
from nous_sdk.contracts.imports import ImportAssertions
from pydantic import JsonValue, TypeAdapter


def assertions_from_source(
    value: dict[str, JsonValue], *, openie: bool = False, identifier_scope: str | None = None
) -> ImportAssertions | None:
    raw_entities = value.get("extracted_entities" if openie else "entities") or []
    raw_relations = value.get("extracted_triples" if openie else "relations") or []
    if not raw_entities and not raw_relations:
        return None
    entities: dict[str, ProposedEntity] = {}

    def entity(raw: JsonValue) -> str:
        if isinstance(raw, str):
            name, fields = raw.strip(), {}
        elif isinstance(raw, dict):
            fields = dict(raw)
            name = str(fields.get("name") or fields.get("label") or "").strip()
        else:
            raise ValueError("Source entities require names or typed entity objects")
        if not name:
            raise ValueError("Source entity name is empty")
        if name not in entities:
            key = f"entity:{len(entities)}"
            entities[name] = ProposedEntity.model_validate(
                {
                    "key": key,
                    "label": name,
                    "entity_type": fields.get("entity_type", "other"),
                    "aliases": fields.get("aliases", []),
                    "description": fields.get("description", ""),
                    "identifiers": {
                        **TypeAdapter(dict[str, str]).validate_python(
                            fields.get("identifiers", {})
                        ),
                        **({identifier_scope + ":name": name} if identifier_scope else {}),
                    },
                }
            )
        return entities[name].key

    for item in TypeAdapter(list[JsonValue]).validate_python(raw_entities):
        entity(item)
    relations: list[ProposedAssociation] = []
    for item in TypeAdapter(list[JsonValue]).validate_python(raw_relations):
        fields = item if isinstance(item, dict) else {}
        if isinstance(item, list) and len(item) == 3:
            subject, predicate, obj = item
        elif fields:
            subject, predicate, obj = (
                fields.get("subject"),
                fields.get("predicate"),
                fields.get("object"),
            )
        else:
            raise ValueError("Source relations require complete triples")
        if not isinstance(predicate, str) or not predicate.strip():
            raise ValueError("Source relation predicate is empty")
        relations.append(
            ProposedAssociation(
                from_key=entity(subject),
                to_key=entity(obj),
                relation=predicate.strip(),
                epistemic=EpistemicStance(classification="reported"),
                description=str(fields.get("description") or ""),
                weight=TypeAdapter(float | None).validate_python(fields.get("weight")),
                external_ids=TypeAdapter(dict[str, str]).validate_python(
                    fields.get("external_ids", {})
                ),
                polarity=TypeAdapter(Literal["positive", "negative"]).validate_python(
                    fields.get("polarity", "positive")
                ),
            )
        )
    if len(entities) > 10000 or len(relations) > 10000:
        raise DomainError(
            "source_assertion_limit", "A source record contains too many explicit assertions"
        )
    return ImportAssertions(
        memories=(), entities=tuple(entities.values()), associations=tuple(relations)
    )
