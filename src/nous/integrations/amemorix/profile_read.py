"""One person selection and evidence projection for A-Memorix profile readers."""

from __future__ import annotations

import unicodedata
from typing import TYPE_CHECKING

from nous_sdk.amemorix.contracts import ProfileInput
from nous_sdk.contracts import CallContext, CognitiveRecord, DomainError, EntityBody

from nous.material.service import require_observation_in

if TYPE_CHECKING:
    from .service import AMemorixService

_UNAVAILABLE = {"source_unavailable", "cognition_not_found", "region_not_found"}


def person_id(record: CognitiveRecord) -> str:
    assert isinstance(record.body, EntityBody)
    direct = record.body.identifiers.get("maibot:person")
    if direct:
        return direct
    return next(
        (
            identity.account_id
            for identity in record.body.platform_identities
            if identity.platform == "maibot"
        ),
        record.reference.id,
    )


async def people(self: AMemorixService, ctx: CallContext) -> tuple[CognitiveRecord, ...]:
    return tuple(
        sorted(
            (
                record
                for record in await self.records(ctx, ("entity",))
                if isinstance(record.body, EntityBody)
                and record.body.entity_type == "person"
                and not record.deleted
            ),
            key=lambda record: (person_id(record), record.reference.id),
        )
    )


async def select_person(
    self: AMemorixService, ctx: CallContext, command: ProfileInput
) -> CognitiveRecord:
    if not command.person_id and not command.person_name:
        raise DomainError("person_required", "Profile requires person_id or person_keyword")
    found: list[CognitiveRecord] = []
    for record in await people(self, ctx):
        assert isinstance(record.body, EntityBody)
        if command.person_id:
            matches = command.person_id in {
                person_id(record),
                record.reference.id,
                record.reference.exact(),
                *(
                    identity.account_id
                    for identity in record.body.platform_identities
                    if identity.platform == "maibot"
                ),
            }
        else:
            key = command.person_name.casefold().strip()
            matches = key in {
                name.casefold().strip()
                for name in (record.body.label, *record.body.effective_aliases)
            }
        if matches:
            found.append(record)
    if len(found) != 1:
        raise DomainError(
            "person_not_found" if not found else "person_ambiguous",
            "Person has no unique authorized profile",
        )
    return found[0]


def summary(record: CognitiveRecord) -> dict[str, object]:
    assert isinstance(record.body, EntityBody)
    point = record.times.recorded
    return {
        "person_id": person_id(record),
        "person_name": record.body.label,
        "profile_version": record.reference.revision,
        "profile_text": record.body.profile_text,
        "auto_profile_text": record.body.description,
        "manual_override_text": record.body.profile_controls.summary.text
        if record.body.profile_controls.summary
        else None,
        "updated_at": point.coordinate / 1000 if point.domain == "unix_ms" else None,
        "profile_source": "nous_entity",
        "has_manual_override": record.body.profile_controls.summary is not None,
        "manual_override": record.body.profile_controls.summary.text
        if record.body.profile_controls.summary
        else None,
        "reference": record.reference,
    }


def grams(value: str) -> set[str]:
    normalized = unicodedata.normalize("NFKC", value).casefold()
    token = "".join(char for char in normalized if char.isalnum())
    return {token[index : index + 2] for index in range(len(token) - 1)} - {
        "用户",
        "这个",
        "那个",
        "自己",
        "喜欢",
    }


async def read_profile(
    self: AMemorixService, ctx: CallContext, record: CognitiveRecord, command: ProfileInput
) -> dict[str, object]:
    accepted: list[dict[str, object]] = []
    uncertain: list[dict[str, object]] = []
    async with self.app.authority.read(ctx) as view:
        record = await self.app.cognition.get_in(view, ctx, record.reference)
        assert isinstance(record.body, EntityBody)
        for index, attribute in enumerate(record.body.attributes):
            readable = True
            for link in attribute.evidence:
                try:
                    if link.reference is not None:
                        await self.app.cognition.get_in(view, ctx, link.reference)
                    elif link.observation_id is not None:
                        await require_observation_in(
                            view, ctx, link.observation_id, region_id=link.region_id
                        )
                except DomainError as error:
                    if error.code not in _UNAVAILABLE:
                        raise
                    readable = False
                    break
            if not readable:
                continue
            item: dict[str, object] = {
                "evidence_key": f"{record.reference.exact()}#/body/attributes/{index}",
                "hash": f"{record.reference.exact()}#/body/attributes/{index}",
                "evidence_type": "entity_attribute",
                "type": "entity_attribute",
                "deletable": True,
                "correction_mode": "withdraw_attribute_support",
                "content": f"{attribute.predicate}: {attribute.value}",
                "reference": record.reference,
                "attribute_pointer": f"/body/attributes/{index}",
                "evidence": attribute.evidence,
                "metadata": {"epistemic": attribute.epistemic, "validity": attribute.validity},
            }
            confirmed = (
                attribute.epistemic.classification in {"observed", "reported"}
                and not any(link.polarity == "counterexample" for link in attribute.evidence)
                and (attribute.validity is None or attribute.validity.contains(ctx.clock))
            )
            (accepted if confirmed else uncertain).append(item)
        from nous_sdk.contracts.fact_body import FactClaimBody, PersonFactScope
        from nous_sdk.contracts.facts import FactQuery

        from nous.cognition.fact_reads import query_in as fact_query_in

        claims = await fact_query_in(
            self.app.cognition,
            view,
            ctx,
            FactQuery(
                scope=PersonFactScope(person=record.reference),
                statuses=("active", "conflicted"),
                limit=command.limit,
            ),
        )
        for claim in claims:
            body = claim.body
            assert isinstance(body, FactClaimBody)
            item = {
                "evidence_key": claim.reference.exact(),
                "hash": claim.reference.exact(),
                "evidence_type": "fact_claim",
                "type": "fact_claim",
                "deletable": True,
                "correction_mode": "retract_fact_claim",
                "reference": claim.reference,
                "content": f"{body.fact_key} ({body.polarity}): {body.value_text}",
                "evidence": (*claim.evidence, *(entry.link for entry in body.evidence)),
                "metadata": {
                    "status": body.status,
                    "stability": body.stability,
                    "authority": body.authority,
                    "confidence": body.confidence,
                    "validity": claim.times.validity,
                },
            }
            confirmed = (
                body.status == "active"
                and body.stability == "stable"
                and not any(entry.link.polarity == "counterexample" for entry in body.evidence)
                and (claim.times.validity is None or claim.times.validity.contains(ctx.clock))
            )
            (accepted if confirmed else uncertain).append(item)
        if command.context_text:
            context = grams(command.context_text)
            uncertain.sort(
                key=lambda item: (
                    -len(context & grams(str(item["content"]))),
                    str(item["evidence_key"]),
                )
            )
        return {
            **summary(record),
            "success": True,
            "summary": record.body.profile_text,
            "traits": [item["content"] for item in accepted[: command.limit]],
            "evidence": accepted[: command.limit],
            "uncertain": uncertain[: command.limit],
            "uncertain_fact_count": len(uncertain),
            "evidence_count": len(accepted),
            "watermark": view.commit_seq,
        }


async def aliases(
    self: AMemorixService, ctx: CallContext, record: CognitiveRecord
) -> dict[str, object]:
    record = await self.app.cognition.get(ctx, record.reference)
    assert isinstance(record.body, EntityBody)
    return {
        "success": True,
        "person_id": person_id(record),
        "primary_name": record.body.label,
        "effective_aliases": record.body.effective_aliases,
        "derived_aliases": record.body.aliases,
        "manual_aliases": record.body.profile_controls.aliases.aliases
        if record.body.profile_controls.aliases
        else (),
        "has_override": record.body.profile_controls.aliases is not None,
        "has_manual_override": record.body.profile_controls.aliases is not None,
        "reference": record.reference,
    }
