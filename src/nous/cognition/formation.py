"""Formation operations share the cognitive authority transaction."""

from __future__ import annotations

import unicodedata
from dataclasses import dataclass
from typing import TYPE_CHECKING

from sqlalchemy import select, tuple_

from nous.cognition.entities import project_entity_in
from nous.cognition.facts import FactService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.persistence import ReadView, WriteTx

from .lifecycle import selectable
from .schema import revisions

if TYPE_CHECKING:
    from .records import CognitiveRecords
from nous_sdk.contracts import (
    AdoptCognition,
    AssociationBody,
    CognitiveBody,
    CognitiveRecord,
    CognitiveRef,
    CognitiveTimes,
    ConceptBody,
    EntityAttribute,
    EntityBody,
    EntityMention,
    EvidenceLink,
    FormationCatalogEntry,
    FormationDraft,
    ImportRecord,
    MediaReference,
    MemoryBody,
    ProposedConcept,
    ProposedSchema,
    SchemaBody,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.derivation import DerivationInput, DerivationStrategy, RepresentationRef
from nous_sdk.contracts.formation import (
    GraphDraft,
    GraphResolutionDraft,
    GraphResolutionInput,
    NotesDraft,
)
from nous_sdk.contracts.imports import ImportFact, ImportReference

from nous.platform.time import parse_timepoint

if TYPE_CHECKING:
    from nous.platform.models.generation import ModelResult


@dataclass(frozen=True)
class GraphResolutionPlan:
    request: GraphResolutionInput | None
    entity_keys: tuple[str, ...]
    relation_keys: tuple[str, ...]
    candidates: dict[str, FormationCatalogEntry]
    entity_candidates: dict[str, tuple[str, ...]]
    relation_endpoints: dict[str, tuple[str, str]]


class FormationService:
    def __init__(
        self, records: CognitiveRecords, *, facts: FactService, configuration: ConfigurationService
    ):
        self.configuration = configuration
        self.facts = facts
        self.records = records

    async def formation_catalog_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        text: str = "",
        limit: int = 128,
        *,
        references: tuple[CognitiveRef, ...] | None = None,
    ) -> tuple[FormationCatalogEntry, ...]:
        result: list[FormationCatalogEntry] = []
        active_roots = await self.records.source_roots_in(view, ctx)
        rows = (
            await view.connection.execute(
                select(revisions.c.record)
                .where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.head.is_(True)
                    if references is None
                    else tuple_(revisions.c.id, revisions.c.revision).in_(
                        [(ref.id, ref.revision) for ref in references]
                    ),
                    revisions.c.kind.in_(("entity", "concept", "schema")),
                )
                .order_by(revisions.c.commit_seq.desc())
            )
        ).scalars()
        for value in rows:
            record = await project_entity_in(
                self.records, view, ctx, CognitiveRecord.model_validate(value)
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

    async def adopt_notes_result_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        result: ModelResult[NotesDraft],
        *,
        observation_id: str,
        region_id: str | None,
        times: CognitiveTimes,
        inputs: tuple[DerivationInput, ...],
        evidence: tuple[EvidenceLink, ...],
        source_record: ImportRecord | None = None,
        media: tuple[MediaReference, ...] = (),
        strategy: DerivationStrategy = "direct_structured",
        extract_time: bool = True,
    ) -> tuple[tuple[CognitiveRecord, ...], RepresentationRef]:
        from nous.material.service import require_observation_in

        event = await require_observation_in(tx, ctx, observation_id, region_id=region_id)
        contact_id = observation_id if event["kind"] == "contact" else None
        if contact_id is not None:
            selected = MediaReference.model_validate(event["record"]["contact_source"])
            media = tuple(dict.fromkeys((*media, selected)))
        representation = await self.records.derivations.adopt_model_result_in(
            tx,
            ctx,
            result,
            inputs=inputs,
            schema_id="nous.memory-notes/1",
            strategy=strategy,
        )
        records: list[CognitiveRecord] = []
        for note in result.output.notes:
            if not note.text.strip():
                continue
            event_time = (
                parse_timepoint(note.time_expression)
                if extract_time and note.time_expression
                else None
            )
            note_times = times.model_copy(update={"event": event_time or times.event})
            tags = tuple(
                dict.fromkeys((*(source_record.tags if source_record else ()), *note.topics))
            )
            body = MemoryBody(
                text=note.text,
                media=media,
                contact_observation_id=contact_id,
                tags=tags,
                external_ids=(
                    {**source_record.identifiers, "import": source_record.external_id}
                    if source_record
                    else {}
                ),
                metadata=source_record.metadata if source_record else {},
                structured=source_record.structured if source_record else None,
            )
            records.append(
                await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=body,
                        evidence=evidence,
                        times=note_times,
                        producer=result.producer,
                    ),
                )
            )
        return tuple(records), representation.reference

    @staticmethod
    def graph_resolution_plan(
        graph: GraphDraft,
        source_text: str,
        catalog: tuple[FormationCatalogEntry, ...],
    ) -> GraphResolutionPlan:
        def normalized(value: str) -> str:
            return unicodedata.normalize("NFC", value).strip().casefold()

        entity_keys = tuple(f"E{index + 1}" for index in range(len(graph.entities)))
        relation_keys = tuple(f"R{index + 1}" for index in range(len(graph.relations)))
        existing = tuple(item for item in catalog if item.reference.kind == "entity")
        candidate_rows: dict[str, FormationCatalogEntry] = {}
        entity_candidates: dict[str, tuple[str, ...]] = {}
        resolution_entities: list[dict[str, object]] = []
        needs_resolution = False
        next_candidate = 1
        for key, entity in zip(entity_keys, graph.entities, strict=True):
            names = {normalized(value) for value in (entity.name, *entity.aliases) if value.strip()}
            matches = [
                item
                for item in existing
                if names.intersection(
                    normalized(value) for value in (item.label, *item.aliases) if value.strip()
                )
            ]
            keys = []
            rows = []
            for item in matches:
                candidate_key = f"C{next_candidate}"
                next_candidate += 1
                candidate_rows[candidate_key] = item
                keys.append(candidate_key)
                rows.append(
                    {
                        "key": candidate_key,
                        "name": item.label,
                        "aliases": list(item.aliases),
                        "description": item.description,
                    }
                )
            entity_candidates[key] = tuple(keys)
            needs_resolution = needs_resolution or bool(keys)
            resolution_entities.append(
                {
                    "key": key,
                    "name": entity.name,
                    "entity_type": entity.entity_type,
                    "description": entity.description,
                    "candidates": rows,
                }
            )

        names_to_keys: dict[str, list[str]] = {}
        for key, entity in zip(entity_keys, graph.entities, strict=True):
            for label in (entity.name, *entity.aliases):
                normalized_label = normalized(label)
                if normalized_label:
                    names_to_keys.setdefault(normalized_label, []).append(key)

        relation_endpoints: dict[str, tuple[str, str]] = {}
        for key, relation in zip(relation_keys, graph.relations, strict=True):
            subjects = tuple(dict.fromkeys(names_to_keys.get(normalized(relation.subject), ())))
            objects = tuple(dict.fromkeys(names_to_keys.get(normalized(relation.object), ())))
            for endpoint, label, matches in (
                ("subject", relation.subject, subjects),
                ("object", relation.object, objects),
            ):
                if len(matches) != 1:
                    raise DomainError(
                        "formation_relation_entity_unknown"
                        if not matches
                        else "formation_relation_entity_ambiguous",
                        "A graph relation endpoint must identify exactly one extracted entity",
                        relation_key=key,
                        endpoint=endpoint,
                        name=label,
                    )
            relation_endpoints[key] = (subjects[0], objects[0])

        request = (
            GraphResolutionInput(
                source_text=source_text,
                resolution_data={"entities": resolution_entities},
            )
            if needs_resolution
            else None
        )
        return GraphResolutionPlan(
            request=request,
            entity_keys=entity_keys,
            relation_keys=relation_keys,
            candidates=candidate_rows,
            entity_candidates=entity_candidates,
            relation_endpoints=relation_endpoints,
        )

    async def adopt_graph_result_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        result: ModelResult[GraphDraft],
        *,
        resolution: ModelResult[GraphResolutionDraft] | None,
        plan: GraphResolutionPlan,
        observation_id: str,
        times: CognitiveTimes,
        inputs: tuple[DerivationInput, ...],
        evidence: tuple[EvidenceLink, ...],
        media: tuple[MediaReference, ...],
        strategy: DerivationStrategy = "direct_structured",
    ) -> tuple[tuple[CognitiveRecord, ...], tuple[RepresentationRef, ...]]:
        if (plan.request is None) != (resolution is None):
            raise DomainError(
                "formation_resolution_mismatch",
                "Semantic disambiguation is required exactly when extracted names are ambiguous",
            )
        assignments: dict[str, str | None] = {key: None for key in plan.entity_keys}
        if resolution is not None:
            entity_assignments = {
                item.entity_key: item.candidate_key for item in resolution.output.entities
            }
            if set(entity_assignments) != set(plan.entity_keys):
                raise DomainError(
                    "formation_resolution_invalid",
                    "Entity resolution omitted or repeated a local key",
                )
            for key, candidate in entity_assignments.items():
                if candidate is not None and candidate not in plan.entity_candidates.get(key, ()):
                    raise DomainError(
                        "formation_resolution_invalid",
                        "Entity resolution selected a candidate outside its supplied list",
                        entity_key=key,
                    )
                assignments[key] = candidate

        representation = await self.records.derivations.adopt_model_result_in(
            tx,
            ctx,
            result,
            inputs=inputs,
            schema_id="nous.graph-extraction/1",
            strategy=strategy,
        )
        representations = [representation.reference]
        if resolution is not None:
            resolution_inputs = (
                *inputs,
                DerivationInput(
                    ordinal=len(inputs),
                    source=representation.reference.media(),
                    role="context",
                ),
            )
            resolved = await self.records.derivations.adopt_model_result_in(
                tx,
                ctx,
                resolution,
                inputs=resolution_inputs,
                schema_id="nous.graph-resolution/1",
                strategy=strategy,
            )
            representations.append(resolved.reference)
        from .lifecycle import selectable

        def normalized(value: str) -> str:
            return unicodedata.normalize("NFC", value).strip().casefold()

        source_mentions = tuple(
            EntityMention(reference=reference, role="extracted_from_source") for reference in media
        )
        entities = dict(zip(plan.entity_keys, result.output.entities, strict=True))
        entity_records: dict[str, CognitiveRecord] = {}
        grouped_candidates: dict[str, list[str]] = {}
        for key, candidate in assignments.items():
            if candidate is not None:
                grouped_candidates.setdefault(candidate, []).append(key)
        for candidate_key, keys in grouped_candidates.items():
            previous = await self.records.get_in(tx, ctx, plan.candidates[candidate_key].reference)
            if not isinstance(previous.body, EntityBody) or not selectable(previous):
                raise DomainError(
                    "formation_entity_target", "Resolved graph candidate is not an active Entity"
                )
            proposed = [entities[key] for key in keys]
            aliases = tuple(
                dict.fromkeys(
                    alias
                    for item in proposed
                    for alias in (item.name, *item.aliases)
                    if normalized(alias) != normalized(previous.body.label)
                )
            )
            descriptions = tuple(
                dict.fromkeys(
                    item.description.strip() for item in proposed if item.description.strip()
                )
            )
            body = previous.body.model_copy(
                update={
                    "aliases": tuple(dict.fromkeys((*previous.body.aliases, *aliases))),
                    "description": previous.body.description or "\n".join(descriptions),
                    "mentions": tuple(dict.fromkeys((*previous.body.mentions, *source_mentions))),
                }
            )
            adopted = await self.records.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=previous.reference.id,
                    expected_revision=previous.reference.revision,
                    body=body,
                    evidence=(*evidence, EvidenceLink(reference=previous.reference)),
                    times=times,
                    producer=result.producer,
                ),
            )
            for key in keys:
                entity_records[key] = adopted

        output_records: list[CognitiveRecord] = []
        for key in plan.entity_keys:
            if key in entity_records:
                output_records.append(entity_records[key])
                continue
            item = entities[key]
            record = await self.records.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    body=EntityBody(
                        label=item.name,
                        entity_type=item.entity_type,
                        aliases=item.aliases,
                        description=item.description,
                        mentions=source_mentions,
                    ),
                    evidence=evidence,
                    times=times,
                    producer=result.producer,
                ),
            )
            entity_records[key] = record
            output_records.append(record)

        for key, item in zip(plan.relation_keys, result.output.relations, strict=True):
            subject_key, object_key = plan.relation_endpoints[key]
            relation = AssociationBody(
                from_ref=entity_records[subject_key].reference,
                to_ref=entity_records[object_key].reference,
                relation=item.relation,
                basis_class="source_evidence",
                description=item.explanation,
            )
            output_records.append(
                await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=relation,
                        evidence=evidence,
                        times=times,
                        producer=result.producer,
                    ),
                )
            )
        return tuple(output_records), tuple(representations)

    async def adopt_manual_draft_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        draft: FormationDraft,
        *,
        observation_id: str,
        region_id: str | None,
        times: CognitiveTimes,
        catalog: tuple[FormationCatalogEntry, ...] = (),
        media: tuple[MediaReference, ...] = (),
        facts: tuple[ImportFact, ...] = (),
        source_references: tuple[ImportReference, ...] = (),
    ) -> tuple[CognitiveRecord, ...]:
        from graphlib import CycleError, TopologicalSorter

        from nous.material.service import require_observation_in

        if source_references:
            raise DomainError(
                "manual_source_reference_unsupported",
                "Manual source references require an explicit identity owner",
            )
        event = await require_observation_in(tx, ctx, observation_id, region_id=region_id)
        event_time = parse_timepoint(draft.event_expression) if draft.event_expression else None
        times = times.model_copy(update={"event": event_time or times.event})
        evidence = (EvidenceLink(observation_id=observation_id, region_id=region_id),)
        contact_id = observation_id if event["kind"] == "contact" else None
        if contact_id is not None:
            media = tuple(
                dict.fromkeys(
                    (*media, MediaReference.model_validate(event["record"]["contact_source"]))
                )
            )
        references: dict[str, CognitiveRef] = {
            entry.key: entry.reference for entry in catalog
        }
        output: list[CognitiveRecord] = []
        manual_memories: list[CognitiveRecord] = []
        for proposal in draft.memories:
            record = await self.records.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    body=MemoryBody(
                        text=proposal.text,
                        summary=proposal.summary,
                        tags=proposal.tags,
                        media=media,
                        contact_observation_id=contact_id,
                        role=proposal.role,
                        formation_mode=proposal.formation_mode,
                        epistemic=proposal.epistemic,
                    ),
                    evidence=evidence,
                    times=times,
                ),
            )
            manual_memories.append(record)
            output.append(record)
        if len(manual_memories) == 1:
            references["memory"] = manual_memories[0].reference

        graph_items = (*draft.entities, *draft.concepts, *draft.schemas)
        keys = [item.key for item in graph_items]
        if len(set(keys)) != len(keys) or set(keys).intersection(references):
            raise DomainError("formation_key_conflict", "Manual proposal keys must be unique")
        for item in graph_items:
            if item.target_key is not None and item.target_key not in {
                entry.key for entry in catalog
            }:
                raise DomainError(
                    "formation_target_unknown",
                    "An update target must identify a supplied existing source",
                    key=item.target_key,
                )

        for item in draft.entities:
            previous = None
            if item.target_key is not None:
                previous = await self.records.get_in(tx, ctx, references[item.target_key])
                if not isinstance(previous.body, EntityBody):
                    raise DomainError("formation_entity_target", "Manual entity target is not an Entity")
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
            body = (
                previous.body.model_copy(
                    update={
                        "entity_type": item.entity_type
                        if item.entity_type != "other"
                        else previous.body.entity_type,
                        "aliases": tuple(dict.fromkeys((*previous.body.aliases, *item.aliases))),
                        "description": item.description or previous.body.description,
                        "identifiers": {**previous.body.identifiers, **item.identifiers},
                        "attributes": (*previous.body.attributes, *attributes),
                        "mentions": tuple(
                            dict.fromkeys(
                                (*previous.body.mentions,
                                *(EntityMention(reference=ref) for ref in media))
                            )
                        ),
                    }
                )
                if previous is not None
                else EntityBody(
                    label=item.label,
                    entity_type=item.entity_type,
                    aliases=item.aliases,
                    description=item.description,
                    identifiers=item.identifiers,
                    attributes=attributes,
                    mentions=tuple(EntityMention(reference=ref) for ref in media),
                )
            )
            record = await self.records.adopt_in(
                tx,
                ctx,
                AdoptCognition(
                    id=previous.reference.id if previous else None,
                    expected_revision=previous.reference.revision if previous else 0,
                    body=body,
                    evidence=(*evidence, *((EvidenceLink(reference=previous.reference),) if previous else ())),
                    times=times,
                ),
            )
            references[item.key] = record.reference
            output.append(record)

        nodes: dict[str, ProposedConcept | ProposedSchema] = {
            item.key: item for item in (*draft.concepts, *draft.schemas)
        }
        dependencies = {
            key: set(
                item.member_keys
                if isinstance(item, ProposedConcept)
                else (*item.support_keys, *item.counterexample_keys)
            )
            for key, item in nodes.items()
        }
        if any(values.difference(references).difference(nodes) for values in dependencies.values()):
            raise DomainError("formation_key_unknown", "Manual proposal refers to an unavailable key")
        try:
            ordered = tuple(
                TopologicalSorter(
                    {key: values.intersection(nodes) for key, values in dependencies.items()}
                ).static_order()
            )
        except CycleError as error:
            raise DomainError("formation_cycle", "Manual concept and schema references are cyclic") from error
        for key in ordered:
            item = nodes[key]
            previous = (
                await self.records.get_in(tx, ctx, references[item.target_key])
                if item.target_key
                else None
            )
            if isinstance(item, ProposedConcept):
                if previous is not None and not isinstance(previous.body, ConceptBody):
                    raise DomainError("formation_concept_target", "Manual target is not a Concept")
                prior = previous.body if previous is not None else None
                members = tuple(dict.fromkeys(
                    (*(prior.members if isinstance(prior, ConceptBody) else ()),
                    *(references[value] for value in item.member_keys))
                ))
                body: CognitiveBody = ConceptBody(
                    label=item.label,
                    description=item.description,
                    aliases=item.aliases,
                    members=members,
                    lineage=(previous.reference,) if previous else (),
                )
            else:
                if previous is not None and not isinstance(previous.body, SchemaBody):
                    raise DomainError("formation_schema_target", "Manual target is not a Schema")
                body = SchemaBody(
                    title=item.title,
                    claim=item.claim,
                    scope=item.scope,
                    boundary=item.boundary,
                    support=tuple(references[value] for value in item.support_keys),
                    counterexamples=tuple(references[value] for value in item.counterexample_keys),
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
                    evidence=(*evidence, *((EvidenceLink(reference=previous.reference),) if previous else ())),
                    times=times,
                ),
            )
            references[key] = record.reference
            output.append(record)

        for item in draft.associations:
            if item.from_key not in references or item.to_key not in references:
                raise DomainError("formation_key_unknown", "Manual relation has an unavailable endpoint")
            relation = AssociationBody(
                external_ids=item.external_ids,
                from_ref=references[item.from_key],
                to_ref=references[item.to_key],
                relation=item.relation,
                polarity=item.polarity,
                basis_class="source_evidence",
                description=item.description,
                epistemic=item.epistemic,
                weight=item.weight,
            )
            output.append(
                await self.records.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(body=relation, evidence=evidence, times=times),
                )
            )

        if facts:
            if len(manual_memories) != 1:
                raise DomainError(
                    "manual_fact_memory_required",
                    "Manual facts require exactly one explicitly supplied Memory",
                )
            from nous_sdk.contracts.facts import FactClaimInput

            anchor = manual_memories[0]
            for fact in facts:
                scope = (
                    {"kind": "person", "person": references[fact.person_key].model_dump()}
                    if fact.person_key is not None
                    else {"kind": "chat", "conversation_ref": fact.conversation_ref}
                )
                claim = FactClaimInput.model_validate(
                    fact.model_dump(exclude={"person_key", "conversation_ref", "validity"})
                    | {"scope": scope}
                )
                output.extend(
                    await self.facts.assert_source_in(
                        tx,
                        ctx,
                        claim,
                        anchor,
                        fact.validity,
                        validity_explicit="validity" in fact.model_fields_set,
                    )
                )
        return tuple(output)
