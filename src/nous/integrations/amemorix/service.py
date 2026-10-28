"""A-Memorix semantics delegate state to the native Python domain owners."""

from collections import Counter
from typing import TYPE_CHECKING
from uuid import NAMESPACE_URL, uuid5

from nous_wave.amemorix.contracts import (
    AMemorixOperation,
    AMemorixPolicy,
    IngestInput,
    MaintainInput,
    ProfileInput,
    SearchInput,
    SummaryInput,
)
from nous_wave.contracts import (
    AdmitFiles,
    AdoptCognition,
    AssociationBody,
    CallContext,
    CognitiveRecord,
    CognitiveRef,
    DomainError,
    EntityBody,
    EntityAttribute,
    EpistemicStance,
    EvidenceLink,
    ImportDocument,
    ImportResult,
    QueryRequest,
    TimeFilter,
    TimeRange,
)
from nous_wave.contracts.context import CognitiveTarget
from nous_wave.contracts.lifecycle import MemoryChange
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select

from nous.cognition.service import body_text, revisions, vectors
from nous.identity.service import require_subject_in

from .helpers import json_object, json_value, point

if TYPE_CHECKING:
    from nous.application import Application


class AMemorixService:
    def __init__(self, app: Application) -> None:
        self.app = app

    async def execute(self, ctx: CallContext, operation: AMemorixOperation) -> dict[str, JsonValue]:
        async with self.app.authority.read(ctx) as view:
            await require_subject_in(view, ctx)
        tool, arguments = operation.tool, operation.arguments
        if tool == "search_memory":
            return await self.search(ctx, SearchInput.model_validate(arguments))
        if tool in {"ingest_text", "ingest_summary"}:
            command = (
                SummaryInput.model_validate(arguments)
                if tool == "ingest_summary"
                else IngestInput.model_validate(arguments)
            )
            return await self.ingest(ctx, command)
        if tool == "maintain_memory":
            return await self.maintain(ctx, MaintainInput.model_validate(arguments))
        if tool == "get_person_profile":
            return await self.profile(ctx, ProfileInput.model_validate(arguments))
        if tool == "memory_source":
            from nous_wave.amemorix.references import target_reference

            target = target_reference(
                TypeAdapter(dict[str, JsonValue] | str).validate_python(arguments.get("target"))
            )
            if not isinstance(target, CognitiveTarget):
                raise DomainError(
                    "cognition_required", "Memory source requires an exact cognitive revision"
                )
            record = await self.app.cognition.get(ctx, target.reference)
            return json_object(
                {
                    "success": True,
                    "memory": record,
                    "reference": target.reference,
                    "sources": record.source_roots,
                    "evidence": record.evidence,
                }
            )
        if tool == "memory_stats":
            return await self.stats(ctx)
        if tool in {"memory_source_admin", "memory_delete_admin"}:
            from .sources import source_admin

            return await source_admin(self, ctx, tool, arguments)
        if tool == "memory_import_admin":
            from .imports import import_admin

            return await import_admin(self, ctx, arguments)
        if tool == "memory_bundle_admin":
            from .bundles import bundle_admin

            return await bundle_admin(self, ctx, arguments)
        if tool == "memory_runtime_admin":
            from .runtime import runtime_admin

            return await runtime_admin(self, ctx, arguments)
        raise DomainError("capability_unavailable", f"A-Memorix operation {tool!r} is unavailable")

    async def policy(self, ctx: CallContext) -> AMemorixPolicy:
        value = await self.app.configuration.effective(ctx, "integrations.amemorix")
        return AMemorixPolicy.model_validate(value["value"])

    async def records(
        self, ctx: CallContext, kinds: tuple[str, ...] = ()
    ) -> tuple[CognitiveRecord, ...]:
        query = select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.subject_id == ctx.subject_id,
            revisions.c.head.is_(True),
        )
        if kinds:
            query = query.where(revisions.c.kind.in_(kinds))
        async with self.app.authority.read(ctx) as view:
            records = [
                CognitiveRecord.model_validate(value)
                for value in (await view.connection.execute(query)).scalars()
            ]
            visible: list[CognitiveRecord] = []
            roots = await self.app.cognition.active_roots_in(view, ctx)
            for record in records:
                if not record.purged and await self.app.cognition.visible_in(
                    view, ctx, record, roots
                ):
                    visible.append(record)
            return tuple(visible)

    async def memory_ref(self, ctx: CallContext, target: str) -> CognitiveRef:
        if target.startswith("memory:"):
            target = target.removeprefix("memory:")
        identity, separator, version = target.partition("@")
        candidates = [
            record
            for record in await self.records(ctx, ("memory",))
            if record.reference.id == identity
            and (not separator or record.reference.revision == int(version))
        ]
        if len(candidates) != 1:
            raise DomainError("memory_not_found", "Memory target is unavailable or ambiguous")
        return candidates[0].reference

    async def search(self, ctx: CallContext, command: SearchInput) -> dict[str, JsonValue]:
        policy = await self.policy(ctx)
        if not policy.permits(
            command.respect_filter, command.chat_id, command.group_id, command.user_id
        ):
            return {"success": True, "summary": "", "hits": [], "filtered": True}
        sources = await self.app.sources.list(ctx)
        chats = {value for value in (command.chat_id, *command.shared_chat_ids) if value}
        selected = tuple(
            source["id"]
            for source in sources
            if (
                not chats
                or source["origin"].get("conversation_ref") in chats
                or source["source_type"] == "web_import"
            )
            and (
                not command.respect_filter
                or source["source_type"] not in policy.excluded_source_types
            )
        )
        text = command.query
        if command.person_id:
            names = [
                record.body.label
                for record in await self.records(ctx, ("entity",))
                if isinstance(record.body, EntityBody)
                and record.body.identifiers.get("maibot:person") == command.person_id
            ]
            if not names:
                return {"success": True, "summary": "", "hits": [], "filtered": False}
            import json

            text += " @entity(" + json.dumps(names[0], ensure_ascii=False) + ")"
        start, end = point(command.time_start), point(command.time_end)
        times = (
            (
                TimeFilter(
                    axis="event",
                    range=TimeRange(
                        start=start.coordinate if start else None,
                        end=end.coordinate if end else None,
                    ),
                ),
            )
            if start or end
            else ()
        )
        if command.mode == "time":
            text += " $sort(occurred,desc)"
        kinds = ("episode",) if command.mode == "episode" else ()
        mode = (
            "hybrid"
            if command.mode == "hybrid"
            else "lexical"
            if command.mode == "time"
            else policy.search_mode
        )
        query = QueryRequest.model_validate(
            {
                "text": text,
                "mode": mode,
                "kinds": kinds,
                "limit": command.limit,
                "source_ids": selected,
                "times": [value.model_dump(mode="json") for value in times],
            }
        )
        result = await self.app.retrieval.query(ctx, query)
        hits: list[dict[str, JsonValue]] = []
        for hit in result.hits:
            if not isinstance(hit.object, CognitiveRecord) or hit.object.body is None:
                continue
            record = hit.object
            content = body_text(record.body)
            hits.append(
                json_object(
                    {
                        "hash": record.reference.id
                        if record.reference.kind == "memory"
                        else record.reference.exact(),
                        "type": "paragraph"
                        if record.reference.kind == "memory"
                        else record.reference.kind,
                        "title": content[:80],
                        "content": content,
                        "score": hit.score,
                        "source": next(
                            (
                                source["source_type"]
                                for source in sources
                                if source["id"] in record.source_roots
                            ),
                            "",
                        ),
                        "metadata": {
                            "reference": record.reference,
                            "source_roots": record.source_roots,
                        },
                        "episode_id": record.reference.exact()
                        if record.reference.kind == "episode"
                        else "",
                    }
                )
            )
        return json_object(
            {
                "success": True,
                "summary": "\n".join(
                    f"{index + 1}. {str(hit['content'])[:120]}"
                    for index, hit in enumerate(hits[:5])
                ),
                "hits": hits,
                "filtered": False,
                "coverage": result.coverage,
                "retrieval_mode": command.mode,
                "time_start": command.time_start,
                "time_end": command.time_end,
            }
        )

    async def ingest(self, ctx: CallContext, command: IngestInput) -> dict[str, JsonValue]:
        policy = await self.policy(ctx)
        if not policy.permits(
            command.respect_filter, command.chat_id, command.group_id, command.user_id
        ):
            return {
                "success": True,
                "stored_ids": [],
                "skipped_ids": [command.external_id],
                "detail": "chat_filtered",
                "filtered": True,
            }
        operation = command.operation_id or str(
            uuid5(
                NAMESPACE_URL,
                f"nous:amemorix:{ctx.namespace_id}:{ctx.subject_id}:{command.source_type}:{command.external_id}",
            )
        )
        replay = await self.app.authority.receipt(
            ctx,
            operation + ":facade",
            operation_kind="amemorix.ingest",
            scope=ctx.require_subject(),
            payload=command,
        )
        if replay is not None:
            response = json_object(replay.result)
            for reference in TypeAdapter(tuple[CognitiveRef, ...]).validate_python(
                response.get("references", [])
            ):
                await self.app.cognition.get(ctx, reference)
            return response
        origin = {"external_object_ref": command.external_id}
        if command.chat_id:
            origin["conversation_ref"] = command.chat_id
        if command.person_ids:
            origin["person_ids"] = ",".join(command.person_ids)
        from nous_wave.contracts.imports import ImportRecord
        import json
        event_time=point(command.timestamp if command.timestamp is not None else command.time_start)
        record=ImportRecord(external_id=command.external_id,text=command.text,source=command.source_type+(":"+command.chat_id if command.chat_id else ""),source_type=command.source_type,event_time=event_time,origin=origin,tags=command.tags,metadata={**command.metadata,"person_ids":list(command.person_ids),"participants":list(command.participants),"chat_id":command.chat_id},structured=json_value({"entities":command.entities,"relations":command.relations,"time_start":command.time_start,"time_end":command.time_end}))
        document=ImportDocument(text=json.dumps([json_value(record)],ensure_ascii=False),format="json",filename=command.external_id+".json",media_type="application/json",source_type=command.source_type,origin=origin,event_time=event_time,file_identity=f"amemorix:{command.source_type}:{command.external_id}")
        result = await self.app.imports.admit_files(
            ctx,
            AdmitFiles(
                operation_id=operation,
                inputs=(document,),
                llm_enabled=policy.form_memory,
                vectors="metadata_allowed" if policy.form_memory else "disabled",
                dedupe_policy="manifest",
            ),
        )
        if not isinstance(result, ImportResult):
            return json_object(
                {
                    "success": True,
                    "stored_ids": [],
                    "skipped_ids": result["skipped_success_ids"],
                    "detail": "exists",
                }
            )
        memories = tuple(
            reference
            for output in result.outputs
            for reference in output.cognition
            if reference.kind == "memory"
        )
        targets: list[CognitiveRef] = []
        if memories:
            targets = await self._graph(ctx, command, memories[0], operation)
        response = json_object(
            {
                "success": True,
                "stored_ids": [ref.id for ref in (*memories, *targets)],
                "skipped_ids": [],
                "detail": "stored",
                "admission": {
                    "catalog_id": result.catalog.id,
                    "output_ids": [output.id for output in result.outputs],
                },
                "structured_targets": targets,
                "references": [*memories, *targets],
            }
        )
        async with self.app.authority.write(
            ctx,
            operation + ":facade",
            operation_kind="amemorix.ingest",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return json_object(tx.replay_result)
            return tx.result(response)

    async def _graph(
        self, ctx: CallContext, command: IngestInput, memory: CognitiveRef, operation: str
    ) -> list[CognitiveRef]:
        existing = await self.records(ctx, ("entity",))
        names = list(
            dict.fromkeys(
                (
                    *command.entities,
                    *command.participants,
                    *command.person_ids,
                    *(relation.subject for relation in command.relations),
                    *(relation.object for relation in command.relations),
                )
            )
        )
        entity_refs: dict[str, CognitiveRef] = {}
        adopted: list[CognitiveRef] = []
        for index, name in enumerate(names):
            matches = [
                record
                for record in existing
                if isinstance(record.body, EntityBody)
                and (record.body.label == name or name in record.body.identifiers.values())
            ]
            if len(matches) > 1:
                raise DomainError(
                    "entity_ambiguous", "Host entity name identifies multiple records"
                )
            current = matches[0] if matches else None
            attributes=list(current.body.attributes) if current is not None and isinstance(current.body,EntityBody) else []
            additions=[EntityAttribute(predicate=relation.predicate,value=relation.object,epistemic=EpistemicStance(classification="reported",attribution="amemorix:"+command.external_id,confidence=relation.confidence),evidence=(EvidenceLink(reference=memory,role="host_reported_fact"),)) for relation in command.relations if relation.subject==name]
            if name in command.person_ids and command.source_type=="person_fact":
                additions.append(EntityAttribute(predicate="reported_fact",value=command.text,epistemic=EpistemicStance(classification="reported",attribution="amemorix:person_fact"),evidence=(EvidenceLink(reference=memory,role="host_reported_fact"),)))
            for addition in additions:
                old=next((attribute for attribute in attributes if (attribute.predicate,attribute.value)==(addition.predicate,addition.value)),None)
                if old is None:attributes.append(addition)
                else:
                    evidence=list(old.evidence)
                    for link in addition.evidence:
                        if link not in evidence:evidence.append(link)
                    attributes[attributes.index(old)]=old.model_copy(update={"evidence":tuple(evidence)})
            identifiers = (
                dict(current.body.identifiers)
                if current is not None and isinstance(current.body, EntityBody)
                else {}
            )
            if name in command.person_ids:
                identifiers["maibot:person"] = name
            body = EntityBody(
                label=command.metadata.get("person_name") if name in command.person_ids and isinstance(command.metadata.get("person_name"),str) else name,
                entity_type="person" if name in command.person_ids else "other",
                identifiers=identifiers,
                attributes=tuple(attributes),
            )
            record = await self.app.cognition.adopt(
                ctx,
                AdoptCognition(
                    operation_id=f"{operation}:entity:{index}",
                    id=current.reference.id
                    if current
                    else str(
                        uuid5(NAMESPACE_URL, f"{ctx.namespace_id}:{ctx.subject_id}:entity:{name}")
                    ),
                    expected_revision=current.reference.revision if current else 0,
                    body=body,
                    evidence=(
                        *(current.evidence if current else ()),
                        EvidenceLink(reference=memory, role="host_reported_fact"),
                    ),
                ),
            )
            entity_refs[name] = record.reference
            adopted.append(record.reference)
        for index, relation in enumerate(command.relations):
            record = await self.app.cognition.adopt(
                ctx,
                AdoptCognition(
                    operation_id=f"{operation}:relation:{index}",
                    body=AssociationBody(
                        from_ref=entity_refs[relation.subject],
                        to_ref=entity_refs[relation.object],
                        relation=relation.predicate,
                        basis_class="host_explicit",
                        description=f"Reported by {command.external_id}",
                    ),
                    evidence=(EvidenceLink(reference=memory, role="host_reported_relation"),),
                ),
            )
            adopted.append(record.reference)
        return adopted

    async def maintain(self, ctx: CallContext, command: MaintainInput) -> dict[str, JsonValue]:
        if command.action == "recycle_bin":
            return json_object(
                {
                    "success": True,
                    "items": await self.app.cognition.recycle_bin(ctx, limit=command.limit),
                }
            )
        reference = await self.memory_ref(ctx, command.target)
        result = await self.app.cognition.change_memory(
            ctx,
            MemoryChange(
                operation_id=command.operation_id,
                reference=reference,
                action=command.action,
                hours=command.hours,
                reason=command.reason,
            ),
        )
        return json_object(
            {
                "success": True,
                "detail": command.action,
                "memory": result,
                "reference": result.reference,
            }
        )

    async def profile(self, ctx: CallContext, command: ProfileInput) -> dict[str, JsonValue]:
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
        accepted=[attribute for attribute in record.body.attributes if attribute.epistemic.classification in {"observed","reported"} and not any(link.polarity=="counterexample" for link in attribute.evidence) and (attribute.validity is None or attribute.validity.contains(ctx.clock))][:command.limit]
        uncertain=[attribute for attribute in record.body.attributes if attribute not in accepted][:command.limit]
        return json_object({"success":True,"summary":record.body.description,"traits":[f"{attribute.predicate}: {attribute.value}" for attribute in accepted],"evidence":[{"hash":record.reference.exact(),"content":f"{attribute.predicate}: {attribute.value}","type":"entity_attribute","metadata":{"epistemic":attribute.epistemic,"validity":attribute.validity},"evidence":attribute.evidence} for attribute in accepted],"uncertain":uncertain,"reference":record.reference})

    async def stats(self, ctx: CallContext) -> dict[str, JsonValue]:
        records = await self.records(ctx)
        counts = Counter(record.reference.kind for record in records if not record.deleted)
        jobs = await self.app.execution.list_jobs(ctx)
        async with self.app.authority.read(ctx) as view:
            vector_count = (
                len(
                    (
                        await view.connection.execute(
                            select(vectors.c.reference).where(
                                vectors.c.reference.in_(
                                    [record.reference.exact() for record in records]
                                )
                            )
                        )
                    ).all()
                )
                if records
                else 0
            )
        return json_object(
            {
                "success": True,
                "paragraphs": counts["memory"],
                "entities": counts["entity"],
                "relations": counts["association"],
                "episodes": counts["episode"],
                "cognition": dict(counts),
                "tasks": dict(Counter(job.status for job in jobs)),
                "vectors": vector_count,
            }
        )
