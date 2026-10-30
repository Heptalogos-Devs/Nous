"""Project actual Native claims into A-Memorix's published fact tables."""

from __future__ import annotations

from hashlib import sha256

from nous_wave.contracts import CognitiveRecord, EntityBody, SubjectBundle
from nous_wave.contracts.fact_body import FactClaimBody, PersonFactScope
from pydantic import JsonValue, TypeAdapter

from nous.cognition.fact_identity import normalized
from nous.cognition.projection import evidence_in


def external_scope(body: FactClaimBody, bundle: SubjectBundle) -> str:
    if not isinstance(body.scope, PersonFactScope):
        return body.scope.conversation_ref
    person = next((row for row in bundle.cognition if row.reference == body.scope.person), None)
    if person is not None and isinstance(person.body, EntityBody):
        return (
            person.body.identifiers.get("maibot:person")
            or person.body.identifiers.get("amemorix:person")
            or body.scope.person.id
        )
    return body.scope.person.id


def claim_key(body: FactClaimBody, bundle: SubjectBundle) -> str:
    from .portable import canonical

    return sha256(
        canonical(
            {
                "scope_type": body.scope.kind,
                "scope_id": external_scope(body, bundle),
                "fact_key": normalized(body.fact_key),
                "value": normalized(body.value_text),
                "polarity": body.polarity,
            }
        )
    ).hexdigest()


def project_facts(
    bundle: SubjectBundle,
    tables: dict[str, list[dict[str, JsonValue]]],
    selected: dict[str, CognitiveRecord],
) -> None:
    from .portable import canonical
    from .portable_identity import semantic_ids

    identities = semantic_ids(bundle)
    exacts = {record.reference.exact(): record for record in bundle.cognition}
    observation_rows = {row.id: row for row in bundle.observations}
    material_rows = {row.id: row for row in bundle.materials}
    claims = {
        key: record for key, record in selected.items() if isinstance(record.body, FactClaimBody)
    }
    claim_ids = {
        record.reference.id: claim_key(record.body, bundle)
        for record in claims.values()
        if isinstance(record.body, FactClaimBody)
    }

    def point(value: object) -> JsonValue:
        from nous_wave.contracts import TimePoint

        return (
            value.coordinate / 1000
            if isinstance(value, TimePoint) and value.domain == "unix_ms"
            else None
        )

    for record in claims.values():
        body = record.body
        assert isinstance(body, FactClaimBody)
        scope_id = external_scope(body, bundle)
        group: dict[str, JsonValue] = {
            "scope_type": body.scope.kind,
            "scope_id": scope_id,
            "fact_key": normalized(body.fact_key),
        }
        if body.cardinality == "set":
            group["value"] = normalized(body.value_text)
        validity = record.times.validity
        tables["fact_claims"].append(
            {
                "claim_id": claim_ids[record.reference.id],
                "scope_type": body.scope.kind,
                "scope_id": scope_id,
                "fact_key": normalized(body.fact_key),
                "value_text": body.value_text,
                "value_normalized": normalized(body.value_text),
                "polarity": body.polarity,
                "cardinality": body.cardinality,
                "conflict_group": sha256(canonical(group)).hexdigest(),
                "stability": body.stability,
                "profile_section": body.profile_section,
                "authority": body.authority,
                "status": body.status,
                "confidence": body.confidence if body.confidence is not None else 0.5,
                "valid_from": validity.start / 1000
                if validity and validity.domain == "unix_ms" and validity.start is not None
                else None,
                "valid_to": validity.end / 1000
                if validity and validity.domain == "unix_ms" and validity.end is not None
                else None,
                "first_observed_at": point(body.first_observed),
                "last_confirmed_at": point(body.last_confirmed),
                "created_at": point(record.times.formation),
                "updated_at": point(record.times.recorded),
            }
        )
        weights = {item.link.model_dump_json(): item for item in body.evidence}
        seen: set[tuple[str, str, str]] = set()
        for link in evidence_in(record):
            if link.polarity == "boundary":
                continue
            metadata: dict[str, JsonValue] = {
                "reference": TypeAdapter(JsonValue).validate_python(link.model_dump(mode="json"))
            }
            if link.reference:
                evidence_id = link.reference.exact()
                kind = link.reference.kind
                source = exacts.get(evidence_id)
                if source is not None:
                    metadata["source_snapshot"] = TypeAdapter(JsonValue).validate_python(
                        source.model_dump(mode="json")
                    )
                if (
                    kind == "memory"
                    and source is not None
                    and source.reference.id in selected
                    and selected[source.reference.id].reference == source.reference
                ):
                    kind, evidence_id = "paragraph", identities[source.reference.id]
                elif kind == "association" and link.reference.id in identities:
                    kind, evidence_id = "relation", identities[link.reference.id]
            else:
                kind, evidence_id = "observation", link.observation_id or ""
                event = observation_rows.get(evidence_id)
                if event is not None:
                    metadata["observation"] = TypeAdapter(JsonValue).validate_python(
                        event.model_dump(mode="json")
                    )
                    material = material_rows.get(event.material_id)
                    if material is not None:
                        metadata["material"] = TypeAdapter(JsonValue).validate_python(
                            material.model_dump(mode="json")
                        )
            stance = "refute" if link.polarity == "counterexample" else "support"
            signature = kind, evidence_id, stance
            if signature in seen:
                continue
            seen.add(signature)
            item = weights.get(link.model_dump_json())
            if item is not None:
                metadata.update(item.metadata)
            metadata["recorded_weight"] = item.weight if item else None
            metadata["weight_basis"] = (
                "recorded" if item and item.weight is not None else "neutral_prior"
            )
            metadata["recorded_confidence"] = body.confidence
            metadata["confidence_basis"] = (
                "recorded" if body.confidence is not None else "neutral_prior"
            )
            tables["fact_evidence"].append(
                {
                    "claim_id": claim_ids[record.reference.id],
                    "evidence_type": kind,
                    "evidence_id": evidence_id,
                    "stance": stance,
                    "weight": item.weight if item and item.weight is not None else 1.0,
                    "observed_at": point(record.times.recorded),
                    "metadata_json": canonical(metadata).decode(),
                }
            )
    for record in bundle.cognition:
        if (
            record.reference.id not in claims
            or record.cause is None
            or not record.cause.action.startswith("fact.")
        ):
            continue
        body = record.body
        if not isinstance(body, FactClaimBody):
            continue
        tables["fact_transitions"].append(
            {
                "transition_id": record.reference.exact(),
                "old_claim_id": claim_ids.get(record.reference.id) if record.cause.parent else None,
                "new_claim_id": next(
                    (
                        claim_ids.get(link.reference.id)
                        for link in record.evidence
                        if link.role == "fact.superseded_by" and link.reference
                    ),
                    None,
                )
                if body.status == "superseded"
                else None
                if body.status == "retracted"
                else claim_ids[record.reference.id],
                "transition_type": record.cause.action.removeprefix("fact."),
                "reason": record.cause.reason,
                "evidence_type": "",
                "evidence_id": "",
                "created_at": point(record.times.recorded),
            }
        )
