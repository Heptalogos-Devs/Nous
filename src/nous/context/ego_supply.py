"""Supply current personality independently of keyword recall, with explicit social scope."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    ConcernBody,
    DesireBody,
    DomainKnowledgeBody,
    EgoFacetBody,
    EntityBody,
    LanguageConventionBody,
    NarrativeThreadBody,
    QueryRequest,
    SocialRelationBody,
)
from nous_sdk.contracts.cognition import CognitiveKind
from nous_sdk.contracts.context import ContextSegment, ContextUsage
from nous_sdk.contracts.records import RecordsQuery

from nous.identity.access import require_namespace_in
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .runtime import ContextRuntime


async def segments_in(
    self: ContextRuntime, view: ReadView | WriteTx, ctx: CallContext, usage: ContextUsage
) -> tuple[ContextSegment, ...]:
    subject = await require_namespace_in(view, ctx)
    # A worker/reviewer does not acquire the subject's personality merely by
    # reading its namespace. Explicit sources remain available in compilation.
    if usage.view != "subject":
        return ()
    kinds: list[CognitiveKind] = []
    capabilities = set(subject.enabled_features)
    if "ego" in capabilities:
        kinds.extend(("self_facet", "domain_knowledge", "narrative_thread"))
    if "social" in capabilities:
        kinds.extend(("social_relation", "language_convention"))
    if "motivation" in capabilities:
        kinds.extend(("desire", "concern"))
    if not kinds:
        return ()
    listeners: set[str] = set()
    groups: list[CognitiveRef] = []
    for reference in usage.audience_entities:
        record = await self.cognition.get_in(view, ctx, reference)
        if record.reference.kind == "entity":
            listeners.add(record.reference.id)
            if isinstance(record.body, EntityBody) and record.body.entity_type == "group":
                groups.append(record.reference)
    situations = {value.casefold() for value in usage.situations}
    related: set[str] = set()
    if usage.intent.strip():
        selected = await self.retrieval.query(
            ctx,
            QueryRequest(
                text=usage.intent, kinds=tuple(kinds), limit=64, capability_policy="best_effort"
            ),
        )
        related = {hit.object.reference.exact() for hit in selected.hits}

    records: list[CognitiveRecord] = []
    cursor = None
    while True:
        page = await self.cognition.records_in(
            view,
            ctx,
            RecordsQuery(
                kinds=tuple(kinds),
                cursor=cursor,
                limit=500,
            ),
        )
        records.extend(entry.record for entry in page.items)
        cursor = page.next_cursor
        if cursor is None:
            break
    relations = [record.body for record in records if isinstance(record.body, SocialRelationBody)]
    result: list[ContextSegment] = []
    for record in records:
        body = record.body
        required = False
        selected = False
        if isinstance(body, EgoFacetBody):
            selected = not body.contexts or bool(
                situations.intersection(value.casefold() for value in body.contexts)
            )
            if (
                body.category == "other"
                and not body.contexts
                and not body.audience
                and body.relationship is None
            ):
                selected = False
            if body.audience:
                selected = selected and bool(
                    listeners.intersection(ref.id for ref in body.audience)
                )
            if body.relationship is not None:
                requirement = body.relationship
                relevant = [
                    relation
                    for relation in relations
                    if relation.from_entity.id == requirement.from_entity.id
                    and relation.to_entity.id in listeners
                    and relation.dimension == requirement.dimension
                    and (
                        not relation.groups
                        or listeners.intersection(ref.id for ref in relation.groups)
                    )
                ]
                selected = (
                    selected
                    and bool(relevant)
                    and all(
                        (
                            relation.trust_domains.get(requirement.trust_domain, -1)
                            if requirement.trust_domain
                            else relation.degree
                        )
                        >= requirement.minimum_degree
                        and (
                            requirement.minimum_confidence is None
                            or relation.epistemic.confidence is not None
                            and relation.epistemic.confidence >= requirement.minimum_confidence
                        )
                        and relation.judgement != "insufficient"
                        for relation in relevant
                    )
                )
            required = (
                selected
                and body.category == "identity"
                and not body.contexts
                and not body.audience
                and body.relationship is None
            )
        elif isinstance(body, DomainKnowledgeBody):
            selected = record.reference.exact() in related
        elif isinstance(body, NarrativeThreadBody):
            selected = record.reference.exact() in related
        elif isinstance(body, SocialRelationBody):
            selected = bool(listeners.intersection((body.from_entity.id, body.to_entity.id)))
            if body.groups:
                selected = selected and bool(listeners.intersection(ref.id for ref in body.groups))
        elif isinstance(body, LanguageConventionBody):
            from nous_sdk.contracts.social import SymbolSelection

            selected = usage.symbols is None and self.symbols.scoped(
                body,
                SymbolSelection(
                    audience=usage.audience_entities,
                    groups=tuple(groups),
                    contexts=usage.situations,
                ),
                express=True,
            )
            selected = selected and (
                not body.contexts
                or bool(situations.intersection(value.casefold() for value in body.contexts))
            )
        elif isinstance(body, (DesireBody, ConcernBody)):
            selected = record.reference.exact() in related
        if selected:
            segment = await self.record_segment_in(view, ctx, record)
            result.append(
                segment.model_copy(
                    update={
                        "required": required,
                        "provenance": {
                            **segment.provenance,
                            "supply": "current_personality" if required else "task_related_cognition",
                            "audience": usage.audience,
                        },
                    }
                )
            )
    return tuple(result)
