"""A-Memorix ingestion through the formal domain owners."""

from __future__ import annotations

from typing import TYPE_CHECKING
from uuid import NAMESPACE_URL, uuid5

from nous_wave.amemorix.contracts import (
    IngestInput,
)
from nous_wave.contracts import (
    AdmitFiles,
    AdoptCognition,
    AssociationBody,
    CallContext,
    CognitiveRef,
    DomainError,
    EntityAttribute,
    EntityBody,
    EpistemicStance,
    EvidenceLink,
    ImportDocument,
)
from pydantic import JsonValue, TypeAdapter

from .helpers import json_object, json_value, point

if TYPE_CHECKING:
    from .service import AMemorixService


async def ingest(
    self: AMemorixService, ctx: CallContext, command: IngestInput
) -> dict[str, JsonValue]:
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
    import json

    from nous_wave.contracts.imports import ImportRecord

    event_time = point(command.timestamp if command.timestamp is not None else command.time_start)
    record = ImportRecord(
        external_id=command.external_id,
        text=command.text,
        source=command.source_type + (":" + command.chat_id if command.chat_id else ""),
        source_type=command.source_type,
        event_time=event_time,
        origin=origin,
        tags=command.tags,
        metadata={
            **command.metadata,
            "person_ids": list(command.person_ids),
            "participants": list(command.participants),
            "chat_id": command.chat_id,
        },
        structured=json_value(
            {
                "entities": command.entities,
                "relations": command.relations,
                "time_start": command.time_start,
                "time_end": command.time_end,
            }
        ),
    )
    document = ImportDocument(
        text=json.dumps([json_value(record)], ensure_ascii=False),
        format="json",
        filename=command.external_id + ".json",
        media_type="application/json",
        source_type=command.source_type,
        origin=origin,
        event_time=event_time,
        file_identity=f"amemorix:{command.source_type}:{command.external_id}",
    )
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
    if result.catalog is None:
        return json_object(
            {
                "success": True,
                "stored_ids": [],
                "skipped_ids": result.skipped_success_ids,
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
        targets = await self.adopt_host_graph(ctx, command, memories[0], operation)
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


async def adopt_host_graph(
    self: AMemorixService,
    ctx: CallContext,
    command: IngestInput,
    memory: CognitiveRef,
    operation: str,
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
            raise DomainError("entity_ambiguous", "Host entity name identifies multiple records")
        current = matches[0] if matches else None
        attributes = (
            list(current.body.attributes)
            if current is not None and isinstance(current.body, EntityBody)
            else []
        )
        additions = [
            EntityAttribute(
                predicate=relation.predicate,
                value=relation.object,
                epistemic=EpistemicStance(
                    classification="reported",
                    attribution="amemorix:" + command.external_id,
                    confidence=relation.confidence,
                ),
                evidence=(EvidenceLink(reference=memory, role="host_reported_fact"),),
            )
            for relation in command.relations
            if relation.subject == name
        ]
        if name in command.person_ids and command.source_type == "person_fact":
            additions.append(
                EntityAttribute(
                    predicate="reported_fact",
                    value=command.text,
                    epistemic=EpistemicStance(
                        classification="reported", attribution="amemorix:person_fact"
                    ),
                    evidence=(EvidenceLink(reference=memory, role="host_reported_fact"),),
                )
            )
        for addition in additions:
            old = next(
                (
                    attribute
                    for attribute in attributes
                    if (attribute.predicate, attribute.value)
                    == (addition.predicate, addition.value)
                ),
                None,
            )
            if old is None:
                attributes.append(addition)
            else:
                evidence = list(old.evidence)
                for link in addition.evidence:
                    if link not in evidence:
                        evidence.append(link)
                attributes[attributes.index(old)] = old.model_copy(
                    update={"evidence": tuple(evidence)}
                )
        identifiers = (
            dict(current.body.identifiers)
            if current is not None and isinstance(current.body, EntityBody)
            else {}
        )
        if name in command.person_ids:
            identifiers["maibot:person"] = name
        person_name = command.metadata.get("person_name")
        body = EntityBody(
            label=person_name
            if name in command.person_ids and isinstance(person_name, str)
            else name,
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
                    epistemic=EpistemicStance(
                        classification="reported", attribution=command.external_id
                    ),
                ),
                evidence=(EvidenceLink(reference=memory, role="host_reported_relation"),),
            ),
        )
        adopted.append(record.reference)
    return adopted
