"""Normalize typed semantic proposals and expose a model catalog without authority IDs."""

from __future__ import annotations

from collections.abc import Sequence
from typing import cast

from nous_sdk.contracts import CognitiveRef, EvidenceLink, MediaReference
from nous_sdk.contracts.ego_model import SeedAdoption, SeedProposal
from nous_sdk.contracts.growth import (
    GrowthAction,
    GrowthAdoption,
    GrowthBinding,
    GrowthDraft,
    GrowthProposal,
    GrowthProposalAdoption,
    GrowthSnapshot,
)
from nous_sdk.contracts.growth_bodies import (
    GrowthAttribute,
    GrowthEvidenceRef,
    GrowthMediaRef,
    GrowthRef,
)
from pydantic import BaseModel, JsonValue, TypeAdapter

_OBJECT = TypeAdapter(dict[str, JsonValue])


def normalize_growth(proposal: GrowthProposal | SeedProposal) -> GrowthDraft:
    """Only the program turns local references into an authority action plan."""
    actions: list[GrowthAction] = []
    for action in proposal.actions:
        if not isinstance(action, (GrowthProposalAdoption, SeedAdoption)):
            actions.append(action)
            continue
        bindings: list[GrowthBinding] = []

        def transform(
            value: object, path: str = "", refs: list[GrowthBinding] = bindings
        ) -> JsonValue:
            if isinstance(value, GrowthEvidenceRef):
                refs.append(GrowthBinding(path=path, key=value.key, kind="evidence"))
                return None
            if isinstance(value, GrowthMediaRef):
                refs.append(GrowthBinding(path=path, key=value.key, kind="media"))
                return None
            if isinstance(value, GrowthRef):
                refs.append(GrowthBinding(path=path, key=value.key))
                return None
            if isinstance(value, GrowthAttribute):
                result = value.model_dump(mode="json", exclude={"evidence"})
                result["evidence"] = [None for _ in value.evidence]
                refs.extend(
                    (
                        GrowthBinding(
                            path=f"{path}/evidence/{index}", key=reference.key, kind="evidence"
                        )
                        for index, reference in enumerate(value.evidence)
                    )
                )
                return _OBJECT.validate_python(result)
            if isinstance(value, BaseModel):
                return {
                    key: transform(getattr(value, key), f"{path}/{key}")
                    for key in type(value).model_fields
                }
            if isinstance(value, (tuple, list)):
                return [
                    transform(child, f"{path}/{index}")
                    for index, child in enumerate(cast(Sequence[object], value))
                ]
            return TypeAdapter(JsonValue).validate_python(value)

        body = _OBJECT.validate_python(transform(action.body))
        if action.body.kind == "journal":
            body["knowledge_cutoff"] = 0
        elif action.body.kind == "association":
            body["basis_class"] = "cognitive_derivation"
        elif action.body.kind == "memory_semantics":
            body["event_parse_status"] = (
                "unresolved" if action.body.event_expression else "not_provided"
            )
        actions.append(
            GrowthAdoption(
                action="adopt",
                key=action.key,
                target_key=action.target_key,
                body_input=body,
                bindings=tuple(bindings),
                evidence_keys=action.evidence_keys,
                reason=action.reason,
            )
        )
    return GrowthDraft(
        summary=proposal.summary, rationale=proposal.rationale, actions=tuple(actions)
    )


def growth_model_catalog(snapshot: GrowthSnapshot) -> dict[str, JsonValue]:
    keys = {entry.record.reference.exact(): entry.key for entry in snapshot.entries}
    from .projection import media_references_in

    media = {
        reference.exact(): reference
        for entry in snapshot.entries
        for reference in media_references_in(entry.record.body)
    }
    for source in snapshot.sources:
        media.setdefault(source.source.exact(), source.source)
    media_keys = {identity: f"m{index}" for index, identity in enumerate(media)}
    omitted = {
        "media",
        "external_ids",
        "identifiers",
        "platform_identities",
        "grounding_observation",
        "purged_attribute_count",
        "purged_identity_count",
    }

    def project(value: object) -> JsonValue:
        if isinstance(value, CognitiveRef):
            key = keys.get(value.exact())
            return {"key": key} if key else {"unavailable": True, "kind": value.kind}
        if isinstance(value, MediaReference):
            return {"key": media_keys[value.exact()], "kind": value.kind}
        if isinstance(value, EvidenceLink):
            return (
                {"reference": project(value.reference)}
                if value.reference
                else {"source": "observed_material", "polarity": value.polarity, "role": value.role}
            )
        if isinstance(value, BaseModel):
            return {
                key: project(getattr(value, key))
                for key in type(value).model_fields
                if key not in omitted and key in value.model_fields_set
            }
        if isinstance(value, (tuple, list)):
            return [project(child) for child in cast(Sequence[object], value)]
        return TypeAdapter(JsonValue).validate_python(value)

    entries: list[JsonValue] = []
    for entry in snapshot.entries:
        body = _OBJECT.validate_python(project(entry.record.body))
        epistemic = body.get("epistemic")
        if (
            isinstance(epistemic, dict)
            and epistemic.get("attribution") == entry.record.producer_call_id
        ):
            epistemic["attribution"] = "model_derived"
        entries.append(
            {
                "key": entry.key,
                "body": body,
                "times": entry.record.times.model_dump(mode="json"),
                "lifecycle": entry.record.lifecycle.model_dump(mode="json")
                if entry.record.lifecycle
                else None,
                "commit_seq": entry.record.commit_seq,
            }
        )
    return {
        "entries": entries,
        **(
            {
                "sources": [
                    {
                        "key": source.key,
                        "text": source.text,
                        "media": {
                            "key": media_keys[source.source.exact()],
                            "kind": source.source.kind,
                        },
                        "times": source.times.model_dump(mode="json"),
                        "requires_media": source.requires_media,
                    }
                    for source in snapshot.sources
                ]
            }
            if snapshot.sources
            else {}
        ),
        "media": [
            {"key": media_keys[identity], "kind": reference.kind}
            for identity, reference in media.items()
        ],
    }
