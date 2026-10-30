"""Foreign claim/evidence rows are real imported reports, never local task or caller history."""

from __future__ import annotations

from collections.abc import Callable
from typing import Protocol

from nous_sdk.contracts import (
    CognitiveBody,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EntityBody,
    EvidenceLink,
    TimeRange,
)
from nous_sdk.contracts.fact_body import (
    ChatFactScope,
    FactClaimBody,
    FactEvidence,
    PersonFactScope,
)
from pydantic import JsonValue, TypeAdapter

from .portable import canonical
from .portable_read import rows


class AdoptRecord(Protocol):
    def __call__(
        self,
        key: str,
        body: CognitiveBody,
        *,
        links: tuple[EvidenceLink, ...] = ...,
        roots: tuple[str, ...] = ...,
        raw: dict[str, JsonValue] | None = ...,
    ) -> CognitiveRef: ...


def adopt_facts(
    tables: dict[str, JsonValue],
    records: list[CognitiveRecord],
    entities: dict[str, CognitiveRef],
    memories: dict[str, CognitiveRef],
    source: Callable[[str, bytes, str, str], tuple[str, str]],
    adopt: AdoptRecord,
) -> None:
    from .portable_adoption import metadata, timestamp

    people: dict[str, CognitiveRef] = {}
    for entity in rows(tables.get("entities")):
        key = str(entity.get("hash") or "")
        if key not in entities:
            continue
        meta = metadata(entity.get("metadata"))
        tokens = [key]
        original = meta.get("reference")
        if original is not None:
            reference = CognitiveRef.model_validate(original)
            if reference.kind != "entity":
                raise DomainError(
                    "bundle_fact_scope", "Person identity metadata must identify an Entity"
                )
            tokens.append(reference.id)
        identifiers = meta.get("identifiers")
        if isinstance(identifiers, dict):
            tokens.extend(
                value
                for name in ("maibot:person", "amemorix:person")
                if isinstance(value := identifiers.get(name), str)
            )
        for token in tokens:
            if token in people and people[token] != entities[key]:
                raise DomainError(
                    "bundle_fact_scope", "A person scope has ambiguous explicit Entity identities"
                )
            people[token] = entities[key]
    claim_rows = rows(tables.get("fact_claims"))
    if len({TypeAdapter(str).validate_python(row.get("claim_id")) for row in claim_rows}) != len(
        claim_rows
    ):
        raise DomainError("bundle_fact_identity", "Fact claim identities must be unique")
    evidence_rows = rows(tables.get("fact_evidence"))
    imported: dict[str, CognitiveRef] = {}
    for row in claim_rows:
        key, scope_id = str(row.get("claim_id") or ""), str(row.get("scope_id") or "")
        if not key or not scope_id:
            raise DomainError(
                "bundle_fact_identity", "Claims require their declared identity and scope"
            )
        _, event = source("fact:" + key, canonical(row), key + ".json", "application/json")
        if row.get("scope_type") == "person":
            person = people.get(scope_id)
            if person is None:
                person = adopt(
                    "person:" + scope_id,
                    EntityBody(
                        label=scope_id,
                        entity_type="person",
                        identifiers={"amemorix:person": scope_id},
                    ),
                    links=(EvidenceLink(observation_id=event, role="portable_person_scope"),),
                )
                people[scope_id] = person
            else:
                index = next(
                    index for index, record in enumerate(records) if record.reference == person
                )
                record = records[index]
                if isinstance(record.body, EntityBody) and record.body.entity_type == "other":
                    records[index] = record.model_copy(
                        update={
                            "body": record.body.model_copy(update={"entity_type": "person"}),
                            "evidence": (
                                *record.evidence,
                                EvidenceLink(observation_id=event, role="portable_person_scope"),
                            ),
                            "source_roots": tuple(sorted({*record.source_roots, event})),
                            "support_roots": tuple(sorted({*record.support_roots, event})),
                        }
                    )
                elif not isinstance(record.body, EntityBody) or record.body.entity_type != "person":
                    raise DomainError(
                        "bundle_fact_scope",
                        "An explicit non-person Entity cannot become a person scope",
                    )
            scope = PersonFactScope(person=person)
        elif row.get("scope_type") == "chat":
            scope = ChatFactScope(conversation_ref=scope_id)
        else:
            raise DomainError("bundle_fact_scope", "Only declared person/chat scopes are supported")
        evidence: list[FactEvidence] = []
        links = [EvidenceLink(observation_id=event, role="portable_fact_claim")]
        confidence = row.get("confidence")
        for entry in evidence_rows:
            if entry.get("claim_id") != key:
                continue
            stance = entry.get("stance")
            if stance not in {"support", "refute"}:
                raise DomainError(
                    "bundle_fact_evidence", "Fact evidence must declare support or refute"
                )
            polarity = "counterexample" if stance == "refute" else "support"
            known = (
                memories.get(str(entry.get("evidence_id")))
                if entry.get("evidence_type") == "paragraph"
                else None
            )
            if known is not None:
                link = EvidenceLink(
                    reference=known, polarity=polarity, role="portable_fact_evidence"
                )
            else:
                _, evidence_event = source(
                    "fact-evidence:" + key + ":" + str(len(evidence)),
                    canonical(entry),
                    "fact-evidence.json",
                    "application/json",
                )
                link = EvidenceLink(
                    observation_id=evidence_event, polarity=polarity, role="portable_fact_evidence"
                )
            links.append(link)
            extra = metadata(entry.get("metadata_json"))
            if (
                extra.get("confidence_basis") in {"recorded", "neutral_prior"}
                and "recorded_confidence" in extra
            ):
                confidence = extra["recorded_confidence"]
            weight = (
                extra.get("recorded_weight")
                if extra.get("weight_basis") in {"recorded", "neutral_prior"}
                and "recorded_weight" in extra
                else entry.get("weight")
            )
            evidence.append(
                FactEvidence(
                    link=link,
                    weight=TypeAdapter(float | None).validate_python(weight),
                    metadata=extra,
                )
            )
        start, end = timestamp(row.get("valid_from")), timestamp(row.get("valid_to"))
        body = FactClaimBody.model_validate(
            {
                "scope": scope,
                "fact_key": row.get("fact_key"),
                "value_text": row.get("value_text"),
                "polarity": row.get("polarity"),
                "cardinality": row.get("cardinality"),
                "stability": row.get("stability"),
                "profile_section": row.get("profile_section"),
                "authority": row.get("authority"),
                "status": row.get("status"),
                "confidence": confidence,
                "evidence": tuple(evidence),
                "first_observed": timestamp(row.get("first_observed_at")),
                "last_confirmed": timestamp(row.get("last_confirmed_at")),
            }
        )
        if isinstance(scope, PersonFactScope):
            links.append(
                EvidenceLink(reference=scope.person, polarity="boundary", role="person_scope")
            )
        reference = adopt(key, body, links=tuple(links), raw=row)
        imported[key] = reference
        index = next(index for index, record in enumerate(records) if record.reference == reference)
        record = records[index]
        validity = TimeRange(
            start=start.coordinate if start else None, end=end.coordinate if end else None
        )
        records[index] = record.model_copy(
            update={"times": record.times.model_copy(update={"validity": validity})}
        )

    transitions = rows(tables.get("fact_transitions"))
    latest: dict[str, dict[str, JsonValue]] = {}
    for row in transitions:
        old_id = str(row.get("old_claim_id") or "")
        if row.get("transition_type") != "supersede" or not old_id:
            continue
        when = timestamp(row.get("created_at"))
        previous = latest.get(old_id)
        previous_when = timestamp(previous.get("created_at")) if previous else None
        if previous is None or (
            when is not None
            and previous_when is not None
            and when.coordinate >= previous_when.coordinate
        ):
            latest[old_id] = row
    states = {str(row["claim_id"]): row.get("status") for row in claim_rows}
    for row in transitions:
        old, new = (
            imported.get(str(row.get("old_claim_id") or "")),
            imported.get(str(row.get("new_claim_id") or "")),
        )
        if old is None and new is None:
            continue
        _, event = source(
            "fact-transition:" + str(row.get("transition_id")),
            canonical(row),
            "fact-transition.json",
            "application/json",
        )
        for reference in {ref.exact(): ref for ref in (old, new) if ref is not None}.values():
            index = next(
                index for index, record in enumerate(records) if record.reference == reference
            )
            record = records[index]
            body = record.body
            assert isinstance(body, FactClaimBody)
            if (
                row.get("transition_type") == "supersede"
                and reference == new
                and old is not None
                and old != new
                and states.get(str(row.get("old_claim_id"))) == "superseded"
                and latest.get(str(row.get("old_claim_id"))) is row
            ):
                body = body.model_copy(
                    update={"supersedes": tuple(dict.fromkeys((*body.supersedes, old)))}
                )
            records[index] = record.model_copy(
                update={
                    "body": body,
                    "evidence": (
                        *record.evidence,
                        EvidenceLink(observation_id=event, role="portable_fact_transition"),
                    ),
                    "source_roots": tuple(sorted({*record.source_roots, event})),
                    "support_roots": tuple(sorted({*record.support_roots, event})),
                }
            )
