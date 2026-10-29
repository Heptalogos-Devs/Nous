"""One person selection and evidence projection for A-Memorix profile readers."""

from __future__ import annotations

import unicodedata
from typing import TYPE_CHECKING

from nous_wave.amemorix.contracts import ProfileInput
from nous_wave.contracts import CallContext, CognitiveRecord, DomainError, EntityBody

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


def require_retained(command: ProfileInput) -> None:
    if command.force_refresh:
        raise DomainError(
            "profile_refresh_unavailable",
            "Forced profile refresh requires the profile formation operation; no refresh was submitted",
        )


async def select_person(
    self: AMemorixService, ctx: CallContext, command: ProfileInput
) -> CognitiveRecord:
    require_retained(command)
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
                name.casefold().strip() for name in (record.body.label, *record.body.aliases)
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
        "profile_text": record.body.description,
        "updated_at": point.coordinate / 1000 if point.domain == "unix_ms" else None,
        "profile_source": "nous_entity",
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
                "evidence_type": "entity_attribute",
                "type": "entity_attribute",
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
            "summary": record.body.description,
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
        "effective_aliases": record.body.aliases,
        "reference": record.reference,
    }
