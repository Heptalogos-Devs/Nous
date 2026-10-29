"""A-Memorix profiles through the formal domain owners."""

from __future__ import annotations

from collections import Counter
from typing import TYPE_CHECKING

from nous_wave.amemorix.contracts import (
    ProfileInput,
)
from nous_wave.contracts import (
    CallContext,
    DomainError,
    EntityBody,
)
from pydantic import JsonValue

from .helpers import json_object

if TYPE_CHECKING:
    from .service import AMemorixService


async def profile(
    self: AMemorixService, ctx: CallContext, command: ProfileInput
) -> dict[str, JsonValue]:
    if not command.person_id and not command.person_name:
        raise DomainError("person_required", "Profile requires person_id or person_name")
    found = [
        record
        for record in await self.records(ctx, ("entity",))
        if isinstance(record.body, EntityBody)
        and record.body.entity_type == "person"
        and (
            (
                command.person_id
                and record.body.identifiers.get("maibot:person") == command.person_id
            )
            or (
                command.person_name
                and (
                    record.body.label == command.person_name
                    or command.person_name in record.body.aliases
                )
            )
        )
    ]
    if len(found) != 1:
        raise DomainError(
            "person_not_found" if not found else "person_ambiguous",
            "Person has no unique authorized profile",
        )
    record = found[0]
    assert isinstance(record.body, EntityBody)
    accepted = [
        attribute
        for attribute in record.body.attributes
        if attribute.epistemic.classification in {"observed", "reported"}
        and not any(link.polarity == "counterexample" for link in attribute.evidence)
        and (attribute.validity is None or attribute.validity.contains(ctx.clock))
    ][: command.limit]
    uncertain = [attribute for attribute in record.body.attributes if attribute not in accepted][
        : command.limit
    ]
    return json_object(
        {
            "success": True,
            "summary": record.body.description,
            "traits": [f"{attribute.predicate}: {attribute.value}" for attribute in accepted],
            "evidence": [
                {
                    "hash": record.reference.exact(),
                    "content": f"{attribute.predicate}: {attribute.value}",
                    "type": "entity_attribute",
                    "metadata": {
                        "epistemic": attribute.epistemic,
                        "validity": attribute.validity,
                    },
                    "evidence": attribute.evidence,
                }
                for attribute in accepted
            ],
            "uncertain": uncertain,
            "reference": record.reference,
        }
    )


async def stats(self: AMemorixService, ctx: CallContext) -> dict[str, JsonValue]:
    records = await self.records(ctx)
    counts = Counter(record.reference.kind for record in records if not record.deleted)
    jobs = await self.app.execution.list_jobs(ctx)
    from .diagnostics import diagnostics

    observed = await diagnostics(self, ctx)
    retrieval = observed["retrieval"]
    assert isinstance(retrieval, dict)
    spaces = retrieval["spaces"]
    assert isinstance(spaces, list)
    vector_count = 0
    for space in spaces:
        if isinstance(space, dict):
            count = space.get("indexed_references")
            if isinstance(count, int):
                vector_count += count
    return json_object(
        {
            **observed,
            "success": True,
            "paragraphs": counts["memory"],
            "entities": counts["entity"],
            "relations": counts["association"],
            "episodes": counts["episode"],
            "cognition": dict(counts),
            "tasks": dict(Counter(job.status for job in jobs)),
            "tasks_recent": True,
            "vectors": vector_count,
        }
    )
