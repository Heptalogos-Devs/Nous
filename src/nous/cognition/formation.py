"""Formation operations share the cognitive authority transaction."""

from __future__ import annotations

from typing import TYPE_CHECKING

from sqlalchemy import select

from nous.cognition.facts import FactService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.persistence import ReadView, WriteTx

from .formation_validation import validate_formation_draft
from .lifecycle import selectable
from .schema import revisions

if TYPE_CHECKING:
    from .records import CognitiveRecords
from nous_sdk.contracts import (
    AdoptCognition,
    AssociationBody,
    CallContext,
    CognitiveBody,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    ConceptBody,
    DomainError,
    EntityAttribute,
    EntityBody,
    EntityMention,
    EpistemicStance,
    EvidenceLink,
    FormationCatalogEntry,
    FormationProposal,
    ImportRecord,
    MediaReference,
    MemoryBody,
    MemoryInterpretation,
    MemorySemanticsBody,
    ProposedConcept,
    ProposedSchema,
    SchemaBody,
)
from nous_sdk.contracts.cognition import PersonProfileControls
from nous_sdk.contracts.imports import ImportFact, ImportReference


class FormationService:
    def __init__(
        self, records: CognitiveRecords, *, facts: FactService, configuration: ConfigurationService
    ):
        self.configuration = configuration
        self.facts = facts
        self.records = records

    async def adopt_tags_in(
        self, tx: WriteTx, ctx: CallContext, memory: CognitiveRecord, labels: tuple[str, ...]
    ) -> tuple[CognitiveRecord, ...]:
        import unicodedata

        keys = dict.fromkeys(
            unicodedata.normalize("NFC", label).strip() for label in labels if label.strip()
        )
        existing = [
            CognitiveRecord.model_validate(value)
            for value in (
                await tx.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.kind == "concept",
                        revisions.c.head.is_(True),
                    )
                )
            ).scalars()
        ]
        result: list[CognitiveRecord] = []
        for label in keys:
            matches = [
                record
                for record in existing
                if isinstance(record.body, ConceptBody)
                and record.body.state == "active"
                and (
                    label.casefold()
                    in [
                        record.body.label.casefold(),
                        *(alias.casefold() for alias in record.body.aliases),
                    ]
                )
                and await self.records.visible_in(tx, ctx, record)
            ]
            if len(matches) > 1:
                raise DomainError(
                    "ambiguous_tag",
                    "Tag label resolves to multiple Concepts",
                    candidates=[record.reference.model_dump() for record in matches],
                )
            if matches:
                previous = matches[0]
                assert isinstance(previous.body, ConceptBody)
                if any(ref.id == memory.reference.id for ref in previous.body.members):
                    result.append(previous)
                    continue
                body = previous.body.model_copy(
                    update={"members": (*previous.body.members, memory.reference)}
                )
                record = await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        id=previous.reference.id,
                        expected_revision=previous.reference.revision,
                        body=body,
                        evidence=(
                            EvidenceLink(reference=previous.reference),
                            EvidenceLink(reference=memory.reference),
                        ),
                        times=memory.times,
                    ),
                )
                existing = [
                    record if item.reference.id == previous.reference.id else item
                    for item in existing
                ]
            else:
                record = await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=ConceptBody(label=label, members=(memory.reference,)),
                        evidence=(EvidenceLink(reference=memory.reference),),
                        times=memory.times,
                    ),
                )
                existing.append(record)
            result.append(record)
        return tuple(result)

    async def formation_catalog_in(
        self, view: ReadView | WriteTx, ctx: CallContext, text: str = "", limit: int = 128
    ) -> tuple[FormationCatalogEntry, ...]:
        result: list[FormationCatalogEntry] = []
        active_roots = await self.records.source_roots_in(view, ctx)
        rows = (
            await view.connection.execute(
                select(revisions.c.record)
                .where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.head.is_(True),
                    revisions.c.kind.in_(("entity", "concept", "schema")),
                )
                .order_by(revisions.c.commit_seq.desc())
            )
        ).scalars()
        for value in rows:
            record = await self.records.project_record_in(
                view, ctx, CognitiveRecord.model_validate(value)
            )
            if not selectable(record) or not await self.records.visible_in(view, ctx, record):
                continue
            body = record.body
            label = (
                body.label
                if isinstance(body, (ConceptBody, EntityBody))
                else body.title
                if isinstance(body, SchemaBody)
                else ""
            )
            aliases = (
                body.effective_aliases
                if isinstance(body, EntityBody)
                else body.aliases
                if isinstance(body, ConceptBody)
                else ()
            )
            if text and (
                not any(term.casefold() in text.casefold() for term in (label, *aliases) if term)
            ):
                continue
            result.append(
                FormationCatalogEntry(
                    material_ids=tuple(
                        sorted(
                            {
                                active_roots[root]
                                for root in record.source_roots
                                if root in active_roots
                            }
                        )
                    ),
                    key=f"existing:{record.reference.id}",
                    reference=record.reference,
                    label=label,
                    aliases=aliases,
                    description=body.description
                    if isinstance(body, EntityBody)
                    else body.description
                    if isinstance(body, ConceptBody)
                    else body.claim
                    if isinstance(body, SchemaBody)
                    else "",
                )
            )
            if len(result) >= limit:
                break
        return tuple(result)

    async def formation_catalog(
        self, ctx: CallContext, text: str = "", limit: int = 128
    ) -> tuple[FormationCatalogEntry, ...]:
        async with self.records.authority.read(ctx) as view:
            return await self.formation_catalog_in(view, ctx, text, limit)

    async def adopt_text_formation_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        unit_text: str,
        source_record: ImportRecord,
        proposal: FormationProposal,
        observation_id: str,
        region_id: str | None,
        times: CognitiveTimes,
        catalog: tuple[FormationCatalogEntry, ...] = (),
        *,
        media: tuple[MediaReference, ...] = (),
    ) -> tuple[CognitiveRecord, ...]:
        """One faithful Memory and a separately identified interpretation of its revision."""
        original = await self.records.adopt_in(
            tx,
            ctx,
            AdoptCognition(
                body=MemoryBody(
                    text=unit_text,
                    media=media,
                    tags=source_record.tags,
                    external_ids={**source_record.identifiers, "import": source_record.external_id},
                    metadata=source_record.metadata,
                    structured=source_record.structured,
                ),
                evidence=(EvidenceLink(observation_id=observation_id, region_id=region_id),),
                times=times,
            ),
        )
        return (
            original,
            *await self.adopt_semantic_proposal_in(
                tx,
                ctx,
                original,
                proposal,
                times,
                catalog,
                facts=source_record.assertions.facts if source_record.assertions else (),
                source_references=source_record.assertions.references
                if source_record.assertions
                else (),
            ),
        )

    async def adopt_semantic_proposal_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        anchor: CognitiveRecord,
        proposal: FormationProposal,
        times: CognitiveTimes,
        catalog: tuple[FormationCatalogEntry, ...] = (),
        *,
        facts: tuple[ImportFact, ...] = (),
        source_references: tuple[ImportReference, ...] = (),
    ) -> tuple[CognitiveRecord, ...]:
        """Resolve model-local keys to real identities before adopting semantic objects."""
        from nous_sdk.contracts.namespace_policy import FormationPolicy

        from .formation_validation import validate_formation_policy

        if proposal.producer_call_id is not None:
            policy = await self.configuration.policy_in(tx, ctx, "formation", FormationPolicy)
            if not policy.enabled:
                raise DomainError("formation_disabled", "Namespace model formation is disabled")
            validate_formation_policy(proposal, policy)
        from graphlib import CycleError, TopologicalSorter

        if not isinstance(anchor.body, MemoryBody):
            raise DomainError(
                "formation_anchor", "Semantic formation requires an actual adopted Memory"
            )
        if source_references:
            from .graph_state import heads_in

            entries: list[FormationCatalogEntry] = []
            for binding in source_references:
                matches: list[CognitiveRecord] = []
                for record in await heads_in(tx, ctx, (binding.kind,)):
                    identifiers = (
                        record.body.identifiers
                        if isinstance(record.body, EntityBody)
                        else record.body.external_ids
                        if isinstance(record.body, (MemoryBody, AssociationBody))
                        else {}
                    )
                    if (
                        identifiers.get(binding.identifier_key)
                        in {binding.identifier, *binding.aliases}
                        and selectable(record)
                        and await self.records.visible_in(tx, ctx, record)
                    ):
                        matches.append(record)
                if len(matches) != 1:
                    raise DomainError(
                        "source_reference_missing" if not matches else "source_reference_ambiguous",
                        "Imported source reference must resolve to one current authorized object",
                        key=binding.key,
                    )
                selected = matches[0]
                entries.append(
                    FormationCatalogEntry(
                        key=binding.key, reference=selected.reference, label=binding.identifier
                    )
                )
            catalog = (*catalog, *entries)
        validate_formation_draft(proposal, catalog)
        result: list[CognitiveRecord] = []
        references = {entry.key: entry.reference for entry in catalog}
        references["memory"] = anchor.reference
        proposed_keys = [
            item.key for item in (*proposal.entities, *proposal.concepts, *proposal.schemas)
        ]
        if len(set(proposed_keys)) != len(proposed_keys) or set(proposed_keys).intersection(
            references
        ):
            raise DomainError(
                "formation_key_conflict",
                "Model-local keys must be unique and distinct from the input catalog",
            )

        async def resolve(key: str) -> CognitiveRef:
            reference = references.get(key)
            if reference is None:
                raise DomainError(
                    "formation_key_unknown",
                    "Model reference is absent from the supplied catalog",
                    key=key,
                )
            await self.records.get_in(tx, ctx, reference)
            return reference

        evidence = (EvidenceLink(reference=anchor.reference),)
        for item in proposal.entities:
            previous = (
                await self.records.get_in(tx, ctx, await resolve(item.target_key))
                if item.target_key
                else None
            )
            if previous is None and proposal.producer_call_id is None:
                from .graph_state import heads_in, resolve_entity

                existing = await heads_in(tx, ctx, ("entity",))
                if item.identifiers:
                    explicit_identifiers = {
                        key: value
                        for key, value in item.identifiers.items()
                        if not key.endswith(":name")
                    }
                    identity = (
                        {
                            key: value
                            for key, value in explicit_identifiers.items()
                            if key.startswith("dataset:")
                        }
                        or explicit_identifiers
                        or item.identifiers
                    )
                    matches = [
                        record
                        for record in existing
                        if isinstance(record.body, EntityBody)
                        and any(
                            (
                                record.body.identifiers.get(key) == value
                                for key, value in identity.items()
                            )
                        )
                        and selectable(record)
                    ]
                    if len(matches) > 1:
                        raise DomainError(
                            "source_entity_ambiguous",
                            "Source identifiers resolve to multiple active Entities",
                        )
                    previous = matches[0] if matches else None
                else:
                    previous = resolve_entity(existing, item.label)
                if previous is not None:
                    await self.records.get_in(tx, ctx, previous.reference)
            if previous is not None and (not isinstance(previous.body, EntityBody)):
                raise DomainError(
                    "formation_entity_target",
                    "An entity update must target an Entity catalog entry",
                )
            prior = (
                previous.body
                if previous is not None and isinstance(previous.body, EntityBody)
                else None
            )
            attributes = tuple(
                EntityAttribute(
                    predicate=attribute.predicate,
                    value=attribute.value,
                    epistemic=attribute.epistemic,
                    validity=attribute.validity,
                    evidence=evidence,
                )
                for attribute in item.attributes
            )
            body = EntityBody(
                label=item.label,
                entity_type=prior.entity_type
                if prior is not None
                and proposal.producer_call_id is None
                and (item.entity_type == "other")
                else item.entity_type,
                aliases=tuple(dict.fromkeys([*(prior.aliases if prior else ()), *item.aliases])),
                description=item.description or (prior.description if prior is not None else ""),
                generated_profile=prior.generated_profile
                if prior is not None and (not item.description) and (not attributes)
                else None,
                profile_controls=prior.profile_controls if prior else PersonProfileControls(),
                platform_identities=prior.platform_identities if prior else (),
                identifiers={**(prior.identifiers if prior else {}), **item.identifiers},
                attributes=(*(prior.attributes if prior else ()), *attributes),
                mentions=(
                    *(prior.mentions if prior else ()),
                    EntityMention(reference=anchor.reference),
                ),
            )
            record = await self.records.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=previous.reference.id if previous else None,
                    expected_revision=previous.reference.revision if previous else 0,
                    body=body,
                    evidence=(
                        *evidence,
                        *((EvidenceLink(reference=previous.reference),) if previous else ()),
                    ),
                    times=times,
                    producer_call_id=proposal.producer_call_id,
                ),
            )
            references[item.key] = record.reference
            result.append(record)
        if facts:
            from nous_sdk.contracts.facts import FactClaimInput

            for fact in facts:
                scope = (
                    {"kind": "person", "person": (await resolve(fact.person_key)).model_dump()}
                    if fact.person_key is not None
                    else {"kind": "chat", "conversation_ref": fact.conversation_ref}
                )
                claim = FactClaimInput.model_validate(
                    {
                        **fact.model_dump(exclude={"person_key", "conversation_ref", "validity"}),
                        "scope": scope,
                    }
                )
                result.extend(
                    await self.facts.assert_source_in(
                        tx,
                        ctx,
                        claim,
                        anchor,
                        fact.validity,
                        validity_explicit="validity" in fact.model_fields_set,
                    )
                )
        nodes: dict[str, ProposedConcept | ProposedSchema] = {
            item.key: item for item in (*proposal.concepts, *proposal.schemas)
        }
        dependencies = {
            key: set(
                item.member_keys
                if isinstance(item, ProposedConcept)
                else (*item.support_keys, *item.counterexample_keys)
            )
            for key, item in nodes.items()
        }
        for values in dependencies.values():
            if values.difference(references).difference(nodes):
                raise DomainError(
                    "formation_key_unknown",
                    "Semantic proposal has a reference outside the supplied catalog",
                )
        try:
            ordered = tuple(
                TopologicalSorter(
                    {key: values.intersection(nodes) for key, values in dependencies.items()}
                ).static_order()
            )
        except CycleError as error:
            raise DomainError(
                "formation_cycle",
                "Concept membership and Schema support must resolve without circular revisions",
            ) from error
        tag_labels = tuple(
            dict.fromkeys(
                [*anchor.body.tags, *(tag for item in proposal.memories for tag in item.tags)]
            )
        )
        for key in ordered:
            item = nodes[key]
            previous = (
                await self.records.get_in(tx, ctx, await resolve(item.target_key))
                if item.target_key
                else None
            )
            if isinstance(item, ProposedConcept):
                if previous is not None and (not isinstance(previous.body, ConceptBody)):
                    raise DomainError(
                        "formation_concept_target", "Concept updates must target a Concept"
                    )
                members = [await resolve(member) for member in item.member_keys]
                if item.label in tag_labels and anchor.reference not in members:
                    members.append(anchor.reference)
                prior_members = (
                    previous.body.members
                    if previous is not None and isinstance(previous.body, ConceptBody)
                    else ()
                )
                unique_members = {
                    reference.exact(): reference for reference in (*prior_members, *members)
                }
                body: CognitiveBody = ConceptBody(
                    label=item.label,
                    description=item.description,
                    aliases=item.aliases,
                    members=tuple(unique_members.values()),
                    lineage=(previous.reference,) if previous else (),
                )
            else:
                if previous is not None and (not isinstance(previous.body, SchemaBody)):
                    raise DomainError(
                        "formation_schema_target", "Schema updates must target a Schema"
                    )
                body = SchemaBody(
                    title=item.title,
                    claim=item.claim,
                    scope=item.scope,
                    boundary=item.boundary,
                    support=tuple([await resolve(member) for member in item.support_keys]),
                    counterexamples=tuple(
                        [await resolve(member) for member in item.counterexample_keys]
                    ),
                    lineage=(previous.reference,) if previous else (),
                    epistemic=item.epistemic,
                )
            record = await self.records.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=previous.reference.id if previous else None,
                    expected_revision=previous.reference.revision if previous else 0,
                    body=body,
                    evidence=(
                        *evidence,
                        *((EvidenceLink(reference=previous.reference),) if previous else ()),
                    ),
                    times=times,
                    producer_call_id=proposal.producer_call_id,
                ),
            )
            references[key] = record.reference
            result.append(record)
        for item in proposal.associations:
            body = AssociationBody(
                external_ids=item.external_ids,
                from_ref=await resolve(item.from_key),
                to_ref=await resolve(item.to_key),
                relation=item.relation,
                polarity=item.polarity,
                basis_class="source_evidence",
                description=item.description,
                epistemic=item.epistemic,
                weight=item.weight,
            )
            result.append(
                await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=body,
                        evidence=evidence,
                        times=times,
                        producer_call_id=proposal.producer_call_id,
                    ),
                )
            )
        concepts = await self.adopt_tags_in(tx, ctx, anchor, tag_labels)
        result.extend(
            record
            for record in concepts
            if record.reference.exact() not in {item.reference.exact() for item in result}
        )
        if proposal.memories or proposal.event_expression:
            all_concepts = tuple(
                {
                    record.reference.exact(): record.reference
                    for record in (*result, *concepts)
                    if isinstance(record.body, ConceptBody)
                }.values()
            )
            semantics = MemorySemanticsBody(
                memory=anchor.reference,
                event_expression=proposal.event_expression,
                event_parse_status="resolved"
                if proposal.event is not None
                else "unresolved"
                if proposal.event_expression
                else "not_provided",
                cognitive_role="declarative",
                formation_mode="grounded",
                epistemic=EpistemicStance(
                    classification="derived", attribution=proposal.producer_call_id or ctx.caller_id
                ),
                concepts=all_concepts,
                grounding_observation=anchor.source_roots[0]
                if len(anchor.source_roots) == 1
                else None,
                interpretations=tuple(
                    MemoryInterpretation(
                        text=item.text,
                        role=item.role,
                        formation_mode=item.formation_mode,
                        epistemic=item.epistemic,
                        tags=item.tags,
                    )
                    for item in proposal.memories
                ),
            )
            result.append(
                await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=semantics,
                        evidence=evidence,
                        times=times,
                        producer_call_id=proposal.producer_call_id,
                    ),
                )
            )
        return tuple(result)
