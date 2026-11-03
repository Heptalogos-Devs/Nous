"""Formation operations share the cognitive authority transaction."""

from __future__ import annotations

import asyncio
import json
import unicodedata
from dataclasses import dataclass
from hashlib import sha256
from typing import TYPE_CHECKING
from uuid import uuid4
from weakref import WeakValueDictionary

from sqlalchemy import select, tuple_, update

from nous.cognition.entities import project_entity_in
from nous.cognition.facts import FactService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.persistence import ReadView, WriteTx

from .lifecycle import selectable
from .schema import formation_scopes, revisions

if TYPE_CHECKING:
    from nous_sdk.contracts.events import FormationScope, PendingFormation

    from nous.platform.content import ContentStore
    from nous.platform.models import ModelService
    from nous.retrieval.service import RetrievalService

    from .memory import MemoryService
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
from nous_sdk.contracts.derivation import DerivationInput, DerivationStrategy, RepresentationRef
from nous_sdk.contracts.formation import (
    GraphDraft,
    GraphResolutionDraft,
    GraphResolutionInput,
    NotesDraft,
)
from nous_sdk.contracts.imports import ImportFact, ImportReference

from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
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
        self,
        records: CognitiveRecords,
        *,
        facts: FactService,
        configuration: ConfigurationService,
        memories: MemoryService,
        models: ModelService,
        retrieval: RetrievalService,
        content: ContentStore,
    ):
        self.configuration = configuration
        self.facts = facts
        self.records = records
        self.memories = memories
        self.models = models
        self.retrieval = retrieval
        self.content = content
        self._scope_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()

    async def register_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        scope: FormationScope,
        new_refs: tuple[MediaReference, ...],
        closed_through: int,
        background_refs: tuple[MediaReference, ...] = (),
        flush: bool = False,
    ) -> PendingFormation | None:
        """Register complete encounter groups without copying their source content."""
        from nous_sdk.contracts.events import PendingFormation
        from nous_sdk.contracts.namespace_policy import FormationPolicy

        from nous.platform.execution import JobSpec
        from nous.platform.persistence.authority import canonical_json

        policy = await self.configuration.policy(ctx, "formation", FormationPolicy)
        if not policy.enabled:
            return None
        if scope.namespace_id != ctx.namespace_id:
            raise DomainError(
                "formation_scope_invalid", "Formation scope belongs to another namespace"
            )
        if any(ref.observation_id is None for ref in (*new_refs, *background_refs)):
            raise DomainError(
                "formation_source_invalid", "Formation requires exact observation sources"
            )
        scope_data = scope.model_dump(mode="json")
        scope_id = sha256(canonical_json(scope_data).encode()).hexdigest()
        row = await tx.connection.scalar(
            select(formation_scopes.c.record).where(formation_scopes.c.scope_id == scope_id)
        )
        value = (
            dict(row)
            if row
            else {
                "scope": scope_data,
                "context": ctx.model_dump(mode="json"),
                "groups": [],
                "inflight": [],
                "job_id": None,
                "active_job": None,
            "active_context": None,
                "covered_through": 0,
            }
        )
        groups = list(value["groups"])
        known = {
            canonical_json(ref) for group in (*groups, *value["inflight"]) for ref in group["refs"]
        }
        incoming = []
        for ref in new_refs:
            payload = ref.model_dump(mode="json")
            key = canonical_json(payload)
            if key not in known:
                incoming.append(payload)
                known.add(key)
        if incoming:
            if not groups:
                value["context"] = ctx.model_dump(mode="json")
            groups.append(
                {
                    "refs": incoming,
                    "background_refs": [ref.model_dump(mode="json") for ref in background_refs],
                    "closed_through": closed_through,
                }
            )
        value["groups"] = groups
        job_id = value["job_id"]
        if groups and (job_id is None or flush):
            job_id = "formation:" + str(uuid4())
            await tx.enqueue(
                JobSpec(
                    workflow_name="nous.formation.contacts",
                    queue_name="formation",
                    workflow_id=job_id,
                    args=(scope_id, job_id),
                    scope=ctx.namespace_id,
                    delay_seconds=0 if flush else policy.coalesce_seconds,
                )
            )
            value["job_id"] = job_id
        if row:
            await tx.connection.execute(
                update(formation_scopes)
                .where(formation_scopes.c.scope_id == scope_id)
                .values(record=value, commit_seq=tx.commit_seq)
            )
        else:
            await tx.connection.execute(
                formation_scopes.insert().values(
                    scope_id=scope_id,
                    namespace_id=ctx.namespace_id,
                    record=value,
                    commit_seq=tx.commit_seq,
                )
            )
        selected_job = job_id or value["active_job"]
        if selected_job is None:
            return None
        return PendingFormation(
            scope_id=scope_id, job_id=selected_job, closed_through=closed_through
        )

    async def _contact_source(self, ctx: CallContext, ref: MediaReference) -> tuple[str, dict]:
        from nous_sdk.contracts.derivation import StructuredRepresentation, TextRepresentation

        from nous.material.text import source_region_text
        from nous.platform.content import ContentObject
        from nous.platform.persistence.authority import canonical_json

        assert ref.observation_id is not None
        async with self.records.authority.read(ctx) as view:
            selected = await self.records.materials.accepted_source_in(
                view, ctx, ref.observation_id, ref
            )
            if ref.kind in {"representation", "derived_region"}:
                payload = (await self.records.derivations.selected_in(view, ctx, ref)).payload
                text = (
                    payload.text
                    if isinstance(payload, TextRepresentation)
                    else canonical_json(payload.value)
                    if isinstance(payload, StructuredRepresentation)
                    else None
                )
            else:
                material, region, _ = await self.records.materials.media_source_in(view, ctx, ref)
                text = material.get("text")
                if text is None and (
                    material["media_type"].startswith("text/") or "json" in material["media_type"]
                ):
                    async with self.content.protect(
                        ContentObject(
                            material["content_hash"], material["byte_len"], material["media_type"]
                        )
                    ) as path:
                        text = (await asyncio.to_thread(path.read_bytes)).decode("utf-8-sig")
                if text is not None and region:
                    text = source_region_text(text, region)
        if text is None or not text.strip():
            raise DomainError("formation_source_empty", "The contact has no textual source")
        return text, dict(selected.event)

    @staticmethod
    def _contact_label(key: str, text: str, event: dict) -> str:
        context = event.get("context") or {}
        author = context.get("author") or {}
        name = author.get("name") or author.get("account_id")
        occurred = context.get("occurred_at") or event.get("event_time")
        observed = context.get("observed_at")
        conversation = context.get("conversation") or {}
        details = []
        if name:
            details.append("作者: " + str(name))
        observer = context.get("observer") or {}
        observer_name = observer.get("name") or observer.get("account_id")
        if observer_name:
            details.append("观察者: " + str(observer_name))
        for field, label in (("participants", "参与者"), ("audience", "听众")):
            names = [
                actor.get("name") or actor.get("account_id") for actor in context.get(field, ())
            ]
            if any(names):
                details.append(label + ": " + ", ".join(str(value) for value in names if value))
        if conversation.get("name"):
            details.append("会话: " + str(conversation["name"]))
        if occurred:
            details.append("发生时间: " + json.dumps(occurred, ensure_ascii=False))
        if observed:
            details.append("接触时间: " + json.dumps(observed, ensure_ascii=False))
        header = "[" + key + "]" + (" (" + "; ".join(details) + ")" if details else "")
        return header + "\n" + text

    async def _prepare_contact_batch(self, ctx, scope, groups):
        from nous_sdk.contracts import QueryRequest
        from nous_sdk.contracts.namespace_policy import FormationPolicy

        from nous.context.lifecycle import work_record_in
        from nous.platform.configuration.models import ModelConfiguration

        policy = await self.configuration.policy(ctx, "formation", FormationPolicy)
        resources = ModelConfiguration.model_validate(
            (await self.configuration.effective(ctx, "models"))["value"]
        )
        route, _ = resources.resolve("formation", require_configured=False)
        output_budget = min(policy.text_output_tokens, max(1, route.context_window // 4))
        accepted = []
        accepted_groups = 0
        background: dict[str, tuple[MediaReference, str, dict]] = {}
        chosen = None
        for group in groups:
            batch = []
            for raw in group["refs"]:
                ref = MediaReference.model_validate(raw)
                text, event = await self._contact_source(ctx, ref)
                batch.append((ref, text, event))
            trial = [*accepted, *batch]
            trial_background = dict(background)
            for raw in group["background_refs"]:
                ref = MediaReference.model_validate(raw)
                if ref.model_dump_json() not in trial_background:
                    text, event = await self._contact_source(ctx, ref)
                    trial_background[ref.model_dump_json()] = (ref, text, event)
            source_text = "\n\n".join(
                self._contact_label(f"s{index}", text, event)
                for index, (_, text, event) in enumerate(trial, 1)
            )
            context_text = "\n\n".join(
                self._contact_label(f"b{index}", text, event)
                for index, (_, text, event) in enumerate(trial_background.values(), 1)
            )
            statements = tuple(
                dict.fromkeys(
                    event["background"] for _, _, event in trial if event.get("background")
                )
            )
            if statements:
                context_text += "\n" + "\n".join(statements)
            if scope.work_id:
                async with self.records.authority.read(ctx) as view:
                    work = await work_record_in(view, ctx, scope.work_id)
                context_text += "\n当前工作：" + "\n".join(
                    (
                        work.content.purpose,
                        work.content.context_text,
                        *work.content.unresolved_questions,
                    )
                )
            query_text = " ".join(text for _, text, _ in trial)[:4096]
            found = await self.retrieval.query(
                ctx,
                QueryRequest(
                    text=query_text,
                    mode="lexical",
                    kinds=("memory",),
                    limit=8,
                    reranker="off",
                    capability_policy="best_effort",
                ),
            )
            candidates = [
                hit.object
                for hit in found.hits
                if isinstance(hit.object, CognitiveRecord)
                and isinstance(hit.object.body, MemoryBody)
            ]
            candidate_text = "\n".join(
                f"[m{index}] {record.body.text}" for index, record in enumerate(candidates, 1)
            )
            refs = tuple(ref for ref, _, _ in (*trial, *trial_background.values()))
            try:
                prepared = await self.models.prepare_generation(
                    ctx,
                    "formation",
                    {
                        "source_text": source_text,
                        "background": context_text,
                        "previous_memories": candidate_text,
                    },
                    NotesDraft,
                    source_ids=tuple(ref.exact() for ref in refs),
                    max_output_tokens=output_budget,
                )
            except DomainError as error:
                if error.code != "model_context_budget" or chosen is None:
                    raise
                break
            estimate = prepared.prompt.token_estimate
            if estimate.text_tokens + (estimate.media_tokens or 0) > policy.text_input_tokens:
                if chosen is None:
                    raise DomainError(
                        "formation_group_too_large",
                        "A complete interaction exceeds the formation text budget",
                    )
                break
            accepted = trial
            accepted_groups += 1
            background = trial_background
            chosen = (trial, trial_background, candidates, prepared)
        if chosen is None:
            raise DomainError("formation_source_empty", "No closed interaction was supplied")
        return accepted_groups, *chosen

    async def run_contacts(self, scope_id: str, job_id: str):
        """Freeze a window, form against current candidates, and advance only its sources."""
        from nous_sdk.contracts.common import utc_now
        from nous_sdk.contracts.events import FormationScope

        lock = self._scope_locks.get(scope_id)
        if lock is None:
            lock = asyncio.Lock()
            self._scope_locks[scope_id] = lock
        async with lock:
            async with self.records.authority.read() as view:
                raw = await view.connection.scalar(
                    select(formation_scopes.c.record).where(formation_scopes.c.scope_id == scope_id)
                )
            if raw is None:
                return {"scope_id": scope_id, "covered": 0}
            ctx = CallContext.model_validate(
                raw.get("active_context") if raw.get("active_job") == job_id else raw["context"]
            )
            scope = FormationScope.model_validate(raw["scope"])
            async with self.records.authority.write(ctx) as tx:
                value = dict(
                    await tx.connection.scalar(
                        select(formation_scopes.c.record).where(
                            formation_scopes.c.scope_id == scope_id
                        )
                    )
                )
                if value["active_job"] not in {None, job_id}:
                    raise DomainError(
                        "formation_previous_pending", "Earlier formation must finish first"
                    )
                if value["active_job"] is None:
                    if value["job_id"] != job_id:
                        return {"scope_id": scope_id, "covered": 0}
                    value.update(
                        active_job=job_id, active_context=value["context"],
                        inflight=value["groups"], groups=[], job_id=None
                    )
                    await tx.connection.execute(
                        update(formation_scopes)
                        .where(formation_scopes.c.scope_id == scope_id)
                        .values(record=value, commit_seq=tx.commit_seq)
                    )
            formed = []
            while value["inflight"]:
                (
                    count,
                    sources,
                    background,
                    candidates,
                    prepared,
                ) = await self._prepare_contact_batch(ctx, scope, value["inflight"])
                result = await self.models.execute_generation(ctx, prepared)
                refs = {
                    f"s{index}": (ref, event) for index, (ref, _, event) in enumerate(sources, 1)
                }
                previous = {
                    f"m{index}": record.reference for index, record in enumerate(candidates, 1)
                }
                used_previous = set()
                async with self.records.authority.write(ctx) as tx:
                    current = dict(
                        await tx.connection.scalar(
                            select(formation_scopes.c.record).where(
                                formation_scopes.c.scope_id == scope_id
                            )
                        )
                    )
                    if (
                        current["active_job"] != job_id
                        or current["inflight"][:count] != value["inflight"][:count]
                    ):
                        raise DomainError(
                            "formation_scope_changed", "Frozen formation sources changed"
                        )
                    all_sources = [ref for ref, _, _ in sources]
                    all_sources.extend(ref for ref, _, _ in background.values())
                    for ref in all_sources:
                        assert ref.observation_id is not None
                        await self.records.materials.accepted_source_in(
                            tx, ctx, ref.observation_id, ref
                        )
                    await self.records.derivations.adopt_model_result_in(
                        tx,
                        ctx,
                        result,
                        inputs=tuple(
                            DerivationInput(ordinal=i, source=ref)
                            for i, ref in enumerate(all_sources)
                        ),
                        schema_id="nous.memory-notes/1",
                        strategy="direct_structured",
                    )
                    for note in result.output.notes:
                        if any(key not in refs for key in note.source_keys):
                            raise DomainError(
                                "formation_source_invalid", "The note names an unavailable source"
                            )
                        if note.relation == "new":
                            if note.previous_key is not None:
                                raise DomainError(
                                    "formation_previous_invalid",
                                    "A new note cannot name a previous memory",
                                )
                            prior = None
                        else:
                            if (
                                note.previous_key not in previous
                                or note.previous_key in used_previous
                            ):
                                raise DomainError(
                                    "formation_previous_invalid",
                                    "The note names an unavailable or repeated memory",
                                )
                            prior = previous[note.previous_key]
                            used_previous.add(note.previous_key)
                        cited = [refs[key] for key in dict.fromkeys(note.source_keys)]
                        now = utc_now()
                        event_time = (
                            parse_timepoint(note.time_expression) if note.time_expression else None
                        )
                        if event_time is None and len(cited) == 1 and cited[0][1].get("event_time"):
                            from nous_sdk.contracts import TimePoint

                            event_time = TimePoint.model_validate(cited[0][1]["event_time"])
                        observed = None
                        observed_raw = (
                            (cited[0][1].get("context") or {}).get("observed_at")
                            if len(cited) == 1
                            else None
                        )
                        if observed_raw:
                            from nous_sdk.contracts import TimePoint

                            observed = TimePoint.model_validate(observed_raw)
                        times = CognitiveTimes(
                            event=event_time, observation=observed, formation=now, recorded=now
                        )
                        evidence = tuple(
                            EvidenceLink(
                                observation_id=ref.observation_id,
                                region_id=ref.id if ref.kind == "source_region" else None,
                            )
                            for ref, _ in cited
                        )
                        body = MemoryBody(
                            text=note.text, tags=note.topics, media=tuple(ref for ref, _ in cited)
                        )
                        adopted = await self.memories.adopt_formation_in(
                            tx,
                            ctx,
                            body=body,
                            evidence=evidence,
                            times=times,
                            producer=result.producer,
                            relation=note.relation,
                            previous=prior,
                        )
                        formed.extend(
                            record.reference.exact()
                            for record in adopted
                            if record.reference.kind == "memory"
                        )
                    covered = current["inflight"][:count]
                    current["inflight"] = current["inflight"][count:]
                    current["covered_through"] = max(
                        current["covered_through"], *(group["closed_through"] for group in covered)
                    )
                    if not current["inflight"]:
                        current["active_job"] = None
                        current["active_context"] = None
                    await tx.connection.execute(
                        update(formation_scopes)
                        .where(formation_scopes.c.scope_id == scope_id)
                        .values(record=current, commit_seq=tx.commit_seq)
                    )
                    value = current
            return {
                "scope_id": scope_id,
                "covered_through": value["covered_through"],
                "cognition": formed,
            }

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
            if set(note.source_keys) != {"s1"} or note.relation != "new" or note.previous_key:
                raise DomainError(
                    "formation_source_invalid", "The note refers to an unavailable source or memory"
                )
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
            records.extend(
                await self.memories.adopt_formation_in(
                    tx,
                    ctx,
                    body=body,
                    evidence=evidence,
                    times=note_times,
                    producer=result.producer,
                    relation="new",
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
        references: dict[str, CognitiveRef] = {entry.key: entry.reference for entry in catalog}
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
                    raise DomainError(
                        "formation_entity_target", "Manual entity target is not an Entity"
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
                                (
                                    *previous.body.mentions,
                                    *(EntityMention(reference=ref) for ref in media),
                                )
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
                    evidence=(
                        *evidence,
                        *((EvidenceLink(reference=previous.reference),) if previous else ()),
                    ),
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
            raise DomainError(
                "formation_key_unknown", "Manual proposal refers to an unavailable key"
            )
        try:
            ordered = tuple(
                TopologicalSorter(
                    {key: values.intersection(nodes) for key, values in dependencies.items()}
                ).static_order()
            )
        except CycleError as error:
            raise DomainError(
                "formation_cycle", "Manual concept and schema references are cyclic"
            ) from error
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
                members = tuple(
                    dict.fromkeys(
                        (
                            *(prior.members if isinstance(prior, ConceptBody) else ()),
                            *(references[value] for value in item.member_keys),
                        )
                    )
                )
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
                    evidence=(
                        *evidence,
                        *((EvidenceLink(reference=previous.reference),) if previous else ()),
                    ),
                    times=times,
                ),
            )
            references[key] = record.reference
            output.append(record)

        for item in draft.associations:
            if item.from_key not in references or item.to_key not in references:
                raise DomainError(
                    "formation_key_unknown", "Manual relation has an unavailable endpoint"
                )
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
