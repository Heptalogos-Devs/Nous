"""Read current social evidence, choose scoped conventions, and interpret locally on request."""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    CognitiveRecord,
    EntityBody,
    LanguageConventionBody,
    MemoryBody,
    QueryRequest,
    SocialRelationBody,
)
from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.common import Contract, utc_now
from nous_sdk.contracts.common import ModelProvenance
from nous_sdk.contracts.records import RecordsQuery
from nous_sdk.contracts.social import (
    SymbolCandidates,
    SymbolCarrier,
    SymbolCatalogEntry,
    SymbolDecision,
    SymbolEvidence,
    SymbolInterpretation,
    SymbolMatch,
    SymbolSelection,
    SymbolSemanticInput,
)
from nous_sdk.contracts.targets import CognitiveTarget, MediaTarget
from sqlalchemy import select

from nous.identity.access import require_namespace_in
from nous.material.service import observations
from nous.platform.models.generation import MediaInput
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from nous.cognition.records import CognitiveRecords
    from nous.material.derived import DerivedService
    from nous.material.service import MaterialService
    from nous.platform.models import ModelService
    from nous.platform.persistence import Authority
    from nous.retrieval.service import RetrievalService


class SymbolSnapshot(Contract):
    input: SymbolSemanticInput
    source_ids: tuple[str, ...]
    degradations: tuple[str, ...] = ()


class SymbolsService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        models: ModelService,
        material: MaterialService,
        derivations: DerivedService,
        retrieval: RetrievalService,
    ) -> None:
        self.authority = authority
        self.cognition = cognition
        self.models = models
        self.material = material
        self.derivations = derivations
        self.retrieval = retrieval

    async def records_in(
        self, view: ReadView | WriteTx, ctx: CallContext, kinds: tuple[str, ...]
    ) -> tuple[CognitiveRecord, ...]:
        from nous_sdk.contracts.cognitive_identity import CognitiveKind
        from pydantic import TypeAdapter

        query = RecordsQuery(
            kinds=TypeAdapter(tuple[CognitiveKind, ...]).validate_python(kinds), limit=500
        )
        result: list[CognitiveRecord] = []
        while True:
            page = await self.cognition.records_in(view, ctx, query)
            result.extend(entry.record for entry in page.items)
            if page.next_cursor is None:
                return tuple(result)
            query = query.model_copy(update={"cursor": page.next_cursor})

    @staticmethod
    def scoped(body: LanguageConventionBody, command: SymbolSelection, *, express: bool) -> bool:
        listeners = {ref.id for ref in (*command.audience, *command.groups)}
        if body.audience and not listeners.intersection(ref.id for ref in body.audience):
            return False
        if body.groups and not {ref.id for ref in command.groups}.intersection(
            ref.id for ref in body.groups
        ):
            return False
        if body.speakers and (
            command.speaker is None or command.speaker.id not in {ref.id for ref in body.speakers}
        ):
            return False
        if (
            body.targets
            and express
            and not {ref.id for ref in command.targets}.intersection(ref.id for ref in body.targets)
        ):
            return False
        if express and (
            listeners.intersection(ref.id for ref in body.avoid_for)
            or body.carriers
            and not command.allow_media
        ):
            return False
        return True

    async def snapshot_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        command: SymbolSelection,
        *,
        express: bool,
        ranks: dict[str, int] | None = None,
    ) -> SymbolSnapshot:
        await require_namespace_in(view, ctx, capability="social")
        entities: list[SymbolCatalogEntry] = []
        for ref in dict.fromkeys(
            (
                *((command.speaker,) if command.speaker else ()),
                *command.audience,
                *command.groups,
                *command.targets,
            )
        ):
            record = await self.cognition.get_in(view, ctx, ref, purpose="current")
            if not isinstance(record.body, EntityBody):
                raise DomainError(
                    "symbol_entity_invalid",
                    "Speakers, targets and audiences must identify actual Entities",
                )
            entities.append(SymbolCatalogEntry(key=f"e{len(entities)}", record=record))
        messages: list[SymbolCatalogEntry] = []
        for ref in command.messages:
            record = await self.cognition.get_in(view, ctx, ref)
            if not isinstance(record.body, MemoryBody):
                raise DomainError(
                    "symbol_message_invalid", "Related messages require exact Memory references"
                )
            messages.append(SymbolCatalogEntry(key=f"m{len(messages)}", record=record))
        at = command.at or utc_now()
        conventions: list[CognitiveRecord] = []
        for record in await self.records_in(view, ctx, ("language_convention",)):
            assert isinstance(record.body, LanguageConventionBody)
            if self.scoped(record.body, command, express=express) and (
                record.times.validity is None or record.times.validity.contains(at)
            ):
                conventions.append(record)
        # Exact forms lead; alternate forms stay available to the explicit
        # semantic pass, rather than merging meanings by an emotion label.
        forms = {ref.exact() for ref in command.carriers}
        ranks = ranks or {}
        conventions.sort(
            key=lambda record: (
                0
                if isinstance(record.body, LanguageConventionBody)
                and (
                    command.expression
                    and command.expression.casefold() == record.body.expression.casefold()
                    or forms.intersection(ref.exact() for ref in record.body.carriers)
                )
                else 1,
                ranks.get(record.reference.exact(), 101),
                record.reference.exact(),
            )
        )
        budget = min(100, max(command.limit, 4 * command.limit))
        degradations = ["symbol_candidate_limit"] if len(conventions) > budget else []
        conventions = conventions[:budget]
        ids = {entry.record.reference.id for entry in entities}
        relations: list[CognitiveRecord] = []
        for record in await self.records_in(view, ctx, ("social_relation",)):
            body = record.body
            assert isinstance(body, SocialRelationBody)
            if (
                ids.intersection((body.from_entity.id, body.to_entity.id))
                and (record.times.validity is None or record.times.validity.contains(at))
                and (
                    not body.groups
                    or {ref.id for ref in command.groups}.intersection(
                        ref.id for ref in body.groups
                    )
                )
                and (
                    not body.scope
                    or {value.casefold() for value in command.contexts}.intersection(
                        value.casefold() for value in body.scope
                    )
                )
            ):
                relations.append(record)
        if len(relations) > budget:
            relations = relations[:budget]
            degradations.append("symbol_relation_limit")
        unique = {ref.exact(): ref for ref in (*command.carriers, *command.message_sources)}
        if len(unique) > 32:
            raise DomainError(
                "symbol_media_budget", "Select at most 32 distinct symbol/context carriers"
            )
        selected_conventions: list[CognitiveRecord] = []
        for record in conventions:
            assert isinstance(record.body, LanguageConventionBody)
            if (
                command.semantic != "off"
                or express
                or (
                    command.expression
                    and command.expression.casefold() == record.body.expression.casefold()
                    or forms.intersection(ref.exact() for ref in record.body.carriers)
                )
            ):
                candidate_carriers = {ref.exact(): ref for ref in record.body.carriers}
                if len(unique.keys() | candidate_carriers.keys()) > 32:
                    degradations.append("symbol_media_limit")
                    continue
                unique.update(candidate_carriers)
            selected_conventions.append(record)
        conventions = selected_conventions
        media = tuple(
            SymbolCarrier(key=f"media{index}", reference=reference, attachment_index=index)
            for index, reference in enumerate(unique.values())
        )
        source_ids = {
            root
            for record in (
                *conventions,
                *relations,
                *(entry.record for entry in entities),
                *(entry.record for entry in messages),
            )
            for root in record.source_roots
        }
        for carrier in media:
            info = await self.material.provenance_in(view, ctx, carrier.reference)
            source_ids.update(info.material_ids)
            source_ids.update(info.roots)
        material_ids = (
            await view.connection.scalars(
                select(observations.c.material_id).where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.id.in_(source_ids),
                )
            )
        ).all()
        source_ids.update(material_ids)
        return SymbolSnapshot(
            input=SymbolSemanticInput(
                mode="express" if express else "understand",
                locale=ctx.locale,
                selection=command.model_copy(update={"semantic": "off", "invocation_id": None}),
                at=at,
                entities=tuple(entities),
                messages=tuple(messages),
                conventions=tuple(
                    SymbolCatalogEntry(key=f"c{index}", record=record)
                    for index, record in enumerate(conventions)
                ),
                relations=tuple(
                    SymbolCatalogEntry(key=f"r{index}", record=record)
                    for index, record in enumerate(relations)
                ),
                carriers=media,
            ),
            source_ids=tuple(sorted(source_ids)),
            degradations=tuple(dict.fromkeys(degradations)),
        )

    async def authorize_in(
        self, view: ReadView | WriteTx, ctx: CallContext, snapshot: SymbolSnapshot
    ) -> None:
        await require_namespace_in(view, ctx, capability="social")
        from nous.cognition.schema import revisions

        for entry in (
            *snapshot.input.entities,
            *snapshot.input.messages,
            *snapshot.input.conventions,
            *snapshot.input.relations,
        ):
            record = await self.cognition.get_in(view, ctx, entry.record.reference)
            if record.body != entry.record.body:
                raise DomainError(
                    "symbol_context_changed",
                    "A symbol input's currently authorized content changed",
                    reference=record.reference.exact(),
                )
            if entry.key.startswith("m"):
                continue
            head = await view.connection.scalar(
                select(revisions.c.revision).where(
                    revisions.c.namespace_id == ctx.namespace_id,
                    revisions.c.id == record.reference.id,
                    revisions.c.head.is_(True),
                )
            )
            if head != record.reference.revision:
                raise DomainError(
                    "symbol_context_changed",
                    "A symbol input changed after its accepted snapshot",
                    reference=record.reference.exact(),
                )
        for carrier in snapshot.input.carriers:
            await self.material.provenance_in(view, ctx, carrier.reference)

    @staticmethod
    def evidence(snapshot: SymbolSnapshot) -> dict[str, SymbolEvidence]:
        result = {"input": SymbolEvidence(key="input", kind="input")}
        result.update(
            {
                entry.key: SymbolEvidence(
                    key=entry.key,
                    kind="cognition",
                    reference=CognitiveTarget(reference=entry.record.reference),
                )
                for entry in (
                    *snapshot.input.entities,
                    *snapshot.input.messages,
                    *snapshot.input.conventions,
                    *snapshot.input.relations,
                )
            }
        )
        result.update(
            {
                entry.key: SymbolEvidence(
                    key=entry.key, kind="media", reference=MediaTarget(reference=entry.reference)
                )
                for entry in snapshot.input.carriers
            }
        )
        return result

    def exact(self, snapshot: SymbolSnapshot) -> SymbolCandidates:
        command, express = snapshot.input.selection, snapshot.input.mode == "express"
        matches: list[SymbolMatch] = []
        records: list[CognitiveRecord] = []
        evidence = self.evidence(snapshot)
        for entry in snapshot.input.conventions:
            body = entry.record.body
            assert isinstance(body, LanguageConventionBody)
            if body.contexts and not {value.casefold() for value in command.contexts}.intersection(
                value.casefold() for value in body.contexts
            ):
                continue
            basis = None
            if express:
                if (
                    not command.intent
                    or command.intent.casefold() in {value.casefold() for value in body.intentions}
                    or command.intent.casefold() in body.pragmatics.casefold()
                ):
                    basis = "intent"
            elif command.expression and command.expression.casefold() == body.expression.casefold():
                basis = "exact_expression"
            elif {ref.exact() for ref in command.carriers}.intersection(
                ref.exact() for ref in body.carriers
            ):
                basis = "exact_carrier"
            if basis is not None:
                records.append(entry.record)
                matches.append(
                    SymbolMatch(
                        reference=entry.record.reference,
                        basis=basis,
                        score=1,
                        confidence=body.epistemic.confidence,
                        meaning=body.meaning,
                        guidance=body.pragmatics,
                        sources=(evidence[entry.key],),
                    )
                )
            if len(records) >= command.limit:
                break
        return SymbolCandidates(
            candidates=tuple(records),
            matches=tuple(matches),
            ambiguous=len(records) > 1,
            unknown=not records,
            at=snapshot.input.at,
            degradations=snapshot.degradations,
        )

    async def select(
        self, ctx: CallContext, command: SymbolSelection, *, express: bool = False
    ) -> SymbolCandidates:
        if command.semantic == "off":
            async with self.authority.read(ctx) as view:
                return self.exact(await self.snapshot_in(view, ctx, command, express=express))
        ranks: dict[str, int] = {}
        query = command.expression or command.intent
        if query:
            try:
                retrieved = await self.retrieval.query(
                    ctx,
                    QueryRequest(
                        text=json.dumps(query, ensure_ascii=False),
                        mode="lexical",
                        kinds=("language_convention",),
                        limit=min(100, 4 * command.limit),
                    ),
                )
                ranks = {
                    hit.object.reference.exact(): index
                    for index, hit in enumerate(retrieved.hits)
                    if isinstance(hit.object, CognitiveRecord)
                }
            except DomainError as error:
                if error.code != "capability_unavailable":
                    raise
        async with self.authority.read(ctx) as view:
            frozen = await self.snapshot_in(view, ctx, command, express=express, ranks=ranks)
        base = self.exact(frozen)
        media: list[MediaInput] = []
        for carrier in frozen.input.carriers:
            if self.models.preview_media_resolver is None:
                raise DomainError(
                    "media_unavailable", "The native symbol media reader is unavailable"
                )
            media.append(await self.models.preview_media_resolver(ctx, carrier.reference))
        async with self.authority.read(ctx) as view:
            await self.authorize_in(view, ctx, frozen)
        try:
            generated = await self.models.generate(
                ctx,
                "symbols",
                frozen.input.model_dump(mode="json"),
                SymbolDecision,
                source_ids=frozen.source_ids,
                media=tuple(media),
            )
        except DomainError as error:
            if command.semantic != "optional" or error.code != "model_unavailable":
                raise
            return base.model_copy(
                update={
                    "semantic_status": "unavailable",
                    "degradations": (*base.degradations, error.code),
                }
            )
        async with self.authority.read(ctx) as view:
            await self.authorize_in(view, ctx, frozen)
        return self.project_result(frozen, generated.output, generated.producer)

    def project_result(
        self,
        frozen: SymbolSnapshot,
        output: SymbolDecision,
        producer: ModelProvenance,
    ) -> SymbolCandidates:
        command = frozen.input.selection
        evidence = self.evidence(frozen)

        def cited(keys: tuple[str, ...]) -> tuple[SymbolEvidence, ...]:
            selected = {key: evidence[key] for key in keys}
            if "input" in selected:
                requested = {
                    reference.exact() for reference in (*command.carriers, *command.message_sources)
                }
                selected.update(
                    {
                        entry.key: evidence[entry.key]
                        for entry in frozen.input.carriers
                        if entry.reference.exact() in requested
                    }
                )
                selected.update({entry.key: evidence[entry.key] for entry in frozen.input.messages})
            return tuple(selected.values())

        directory = {entry.key: entry.record for entry in frozen.input.conventions}
        records: list[CognitiveRecord] = []
        matches: list[SymbolMatch] = []
        invalid: list[str] = list(frozen.degradations)
        for result in output.matches:
            record = directory.get(result.key)
            if (
                record is None
                or not result.source_keys
                or not set(result.source_keys).issubset(evidence)
            ):
                invalid.append("invalid_match:" + result.key)
                continue
            body = record.body
            assert isinstance(body, LanguageConventionBody)
            records.append(record)
            matches.append(
                SymbolMatch(
                    reference=record.reference,
                    basis="semantic",
                    score=result.score,
                    confidence=result.confidence,
                    meaning=body.meaning,
                    guidance=result.guidance,
                    sources=cited(result.source_keys),
                )
            )
        local = output.local_interpretation
        interpretation = None
        if local is not None:
            if not local.source_keys or not set(local.source_keys).issubset(evidence):
                invalid.append("invalid_local_sources")
            else:
                interpretation = SymbolInterpretation(
                    **local.model_dump(exclude={"source_keys"}),
                    sources=cited(local.source_keys),
                    attribution="specified_speaker" if command.speaker else "source_expression",
                    speaker=command.speaker,
                )
        return SymbolCandidates(
            candidates=tuple(records[: command.limit]),
            matches=tuple(matches[: command.limit]),
            local_interpretation=interpretation,
            ambiguous=output.ambiguous or len(matches) > 1,
            unknown=not matches,
            at=frozen.input.at,
            semantic_status="used",
            producer=producer,
            dependencies=tuple(
                item.reference for item in evidence.values() if item.reference is not None
            ),
            degradations=tuple(invalid),
        )

    async def understand(self, ctx: CallContext, command: SymbolSelection) -> SymbolCandidates:
        return await self.select(ctx, command)

    async def expressions(self, ctx: CallContext, command: SymbolSelection) -> SymbolCandidates:
        return await self.select(ctx, command, express=True)
