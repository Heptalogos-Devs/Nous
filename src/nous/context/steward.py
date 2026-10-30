"""An optional configured worker selects authorized context, retaining attribution."""

from hashlib import sha256
from typing import Protocol

from nous_sdk.contracts import CallContext, DomainError, EpistemicStance
from nous_sdk.contracts.context import (
    ContextConsumerPolicy,
    ContextPrepare,
    ContextSegment,
    StewardProposal,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.material.service import observations
from nous.platform.models import ModelService
from nous.platform.persistence import Authority
from nous.platform.persistence.authority import canonical_json


class StewardOwner(Protocol):
    authority: Authority
    models: ModelService | None


async def steward_select(
    service: StewardOwner,
    ctx: CallContext,
    command: ContextPrepare,
    policy: ContextConsumerPolicy,
    offered: list[ContextSegment],
) -> tuple[list[ContextSegment], str | None]:
    if policy.steward_role is None:
        return offered, None
    if service.models is None:
        raise DomainError("steward_unavailable", "Configured Steward model owner is unavailable")
    required = [segment for segment in offered if segment.required]
    optional = {
        f"s{index}": segment for index, segment in enumerate(offered) if not segment.required
    }
    if not optional:
        return required, None
    roots: set[str] = set()
    for segment in offered:
        values = segment.provenance.get("source_roots")
        if isinstance(values, list):
            roots.update(value for value in values if isinstance(value, str))
    material_ids = {
        target.reference.id
        for segment in offered
        for target in segment.sources
        if target.kind == "media" and target.reference.kind == "material"
    }
    async with service.authority.read(ctx) as view:
        material_ids.update(
            str(value)
            for value in (
                await view.connection.execute(
                    select(observations.c.material_id).where(
                        observations.c.id.in_(roots),
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.subject_id,
                        observations.c.active.is_(True),
                    )
                )
            ).scalars()
        )
    generated = await service.models.generate(
        ctx,
        policy.steward_role,
        {
            "usage": command.usage.model_dump(mode="json"),
            "required": [segment.model_dump(mode="json") for segment in required],
            "optional": [
                {"id": key, "segment": segment.model_dump(mode="json")}
                for key, segment in optional.items()
            ],
            "budget": {
                "items": min(command.max_items, policy.max_items),
                "text_bytes": min(command.max_text_bytes, policy.max_text_bytes),
            },
        },
        StewardProposal,
        source_ids=tuple(sorted(material_ids)),
    )
    proposal = generated.output
    if any(key not in optional for key in proposal.selected_ids):
        raise DomainError("steward_invalid_selection", "Steward selected an unavailable source key")
    selected = [optional[key] for key in proposal.selected_ids]
    if proposal.summary:
        sources = tuple(
            {
                target.model_dump_json(): target for item in selected for target in item.sources
            }.values()
        )
        selected.append(
            ContextSegment(
                key="steward:"
                + sha256(
                    canonical_json({"sources": sources, "summary": proposal.summary}).encode()
                ).hexdigest(),
                text=proposal.summary,
                sources=sources,
                dependencies=sources,
                authority="steward_interpretation",
                epistemic=EpistemicStance(
                    classification="inferred", attribution="steward:" + policy.steward_role
                ),
                provenance={
                    "producer_call_id": generated.call_id,
                    "role_id": policy.steward_role,
                    "source_roots": TypeAdapter(JsonValue).validate_python(sorted(roots)),
                },
            )
        )
    return [*required, *selected], generated.call_id
