"""Managed preparation, cursor publication and admission with host measurement."""

from hashlib import sha256
from uuid import uuid4

from nous_wave.contracts import CallContext, CognitiveRecord, CognitiveRef, DomainError
from nous_wave.contracts.context import (
    CognitiveTarget,
    ContextAdmission,
    ContextAdmissionCommand,
    ContextCursor,
    ContextPreparation,
    ContextPrepare,
    ContextProjection,
    ContextReport,
    ContextReportResult,
    ContextSegment,
    ContextTarget,
    EpistemicView,
    ManagedContextResponse,
)
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import select, update

from nous.cognition.service import CognitionService, body_text
from nous.material.service import MaterialService
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json
from nous.retrieval.service import RetrievalService

from .models import contexts, feedback, preparations, reports, sessions, tracks


class ManagedContexts:
    authority: Authority
    cognition: CognitionService
    retrieval: RetrievalService | None
    materials: MaterialService | None

    async def _preparation_in(
        self, view: ReadView | WriteTx, ctx: CallContext, preparation_id: str
    ) -> dict[str, JsonValue]:
        value = (
            await view.connection.execute(
                select(preparations.c.record).where(
                    preparations.c.id == preparation_id,
                    preparations.c.namespace_id == ctx.namespace_id,
                    preparations.c.subject_id == ctx.require_subject(),
                )
            )
        ).scalar_one_or_none()
        if value is None:
            raise DomainError("preparation_not_found", "Context preparation is unavailable")
        return TypeAdapter(dict[str, JsonValue]).validate_python(value)

    async def _targets(
        self, view: ReadView | WriteTx, ctx: CallContext, targets: tuple[ContextTarget, ...]
    ) -> None:
        for target in targets:
            if isinstance(target, CognitiveTarget):
                await self.cognition.get_in(view, ctx, target.reference)
            else:
                if self.materials is None:
                    raise DomainError("media_unavailable", "Context media owner is unavailable")
                if target.reference.kind == "material":
                    await self.materials.read(ctx, target.reference.id)
                elif target.reference.kind == "source_region":
                    await self.materials.read_region(ctx, target.reference.id)
                else:
                    raise DomainError(
                        "source_unavailable", "Context media derivation is unavailable"
                    )

    async def prepare(self, ctx: CallContext, command: ContextPrepare) -> ContextPreparation:
        # Managed input begins with no implicit source access. Trusted host scopes
        # enter through the same authenticated boundary used by ordinary queries.
        selected_ctx = (
            ctx
            if ctx.allowed_source_ids is not None
            else ctx.model_copy(update={"allowed_source_ids": ()})
        )
        offered: list[ContextSegment] = []
        async with self.authority.read(selected_ctx) as view:
            for target in command.current_sources:
                if isinstance(target, CognitiveTarget):
                    record = await self.cognition.get_in(view, selected_ctx, target.reference)
                    offered.append(self._segment(record))
                else:
                    await self._targets(view, selected_ctx, (target,))
                    offered.append(
                        ContextSegment(
                            key=f"{target.reference.kind}:{target.reference.id}",
                            text=f"{target.reference.kind}:{target.reference.id}",
                            sources=(target,),
                            authority="source_material",
                        )
                    )
        if command.query is not None:
            if self.retrieval is None:
                raise DomainError("retrieval_unavailable", "Context retrieval owner is unavailable")
            found = await self.retrieval.query(selected_ctx, command.query)
            for hit in found.hits:
                if isinstance(hit.object, CognitiveRecord):
                    offered.append(self._segment(hit.object))
        track_id = sha256(
            f"{ctx.namespace_id}\0{ctx.require_subject()}\0{command.session_id}\0{command.consumer_id}\0{command.instance_id}".encode()
        ).hexdigest()
        scope = selected_ctx.allowed_source_ids
        config_digest = sha256(
            canonical_json(
                {
                    "usage": command.usage,
                    "scope": scope,
                    "max_items": command.max_items,
                    "max_text_bytes": command.max_text_bytes,
                }
            ).encode()
        ).hexdigest()
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="context.prepare",
            scope=ctx.require_subject(),
            payload={"command": command, "source_scope": scope},
        ) as tx:
            if tx.replayed:
                return ContextPreparation.model_validate(tx.replay_result)
            prior_session = (
                await tx.connection.execute(
                    select(sessions.c.record).where(
                        sessions.c.id == command.session_id,
                        sessions.c.namespace_id == ctx.namespace_id,
                        sessions.c.subject_id == ctx.subject_id,
                    )
                )
            ).scalar_one_or_none()
            if prior_session is not None and prior_session.get("closed", False):
                raise DomainError("session_closed", "Context session is closed")
            if prior_session is None:
                await tx.connection.execute(
                    sessions.insert().values(
                        id=command.session_id,
                        namespace_id=ctx.namespace_id,
                        subject_id=ctx.subject_id,
                        record={
                            "id": command.session_id,
                            "closed": False,
                            "recorded_at": tx.recorded_at,
                        },
                    )
                )
            pending = (
                await tx.connection.execute(
                    select(preparations.c.record).where(preparations.c.track_id == track_id)
                )
            ).scalars()
            if any(
                ContextPreparation.model_validate(value["preparation"]).state == "prepared"
                for value in pending
            ):
                raise DomainError(
                    "context_pending",
                    "Cancel or admit the pending context before preparing another input",
                )
            prior = (
                await tx.connection.execute(select(tracks.c.record).where(tracks.c.id == track_id))
            ).scalar_one_or_none()
            old = ContextProjection.model_validate(prior) if prior is not None else None
            append = (
                old is not None
                and old.cursor == command.known_cursor
                and old.config_digest == config_digest
            )
            previous: tuple[ContextSegment, ...] = ()
            if append and old is not None:
                try:
                    await self._targets(
                        tx,
                        selected_ctx,
                        tuple(target for segment in old.segments for target in segment.sources),
                    )
                    previous = old.segments
                except DomainError:
                    append = False
            selected: list[ContextSegment] = list(previous) if append else []
            seen = {segment.key for segment in selected}
            used = sum(len(segment.text.encode()) for segment in selected)
            for segment in offered:
                if segment.key in seen:
                    continue
                await self._targets(tx, selected_ctx, segment.sources)
                size = len(segment.text.encode())
                if len(selected) < command.max_items and used + size <= command.max_text_bytes:
                    selected.append(segment)
                    seen.add(segment.key)
                    used += size
            delta = tuple(selected[len(previous) :]) if append else tuple(selected)
            projection_id = str(uuid4())
            cursor = ContextCursor(
                track_id=track_id,
                epoch_id=old.cursor.epoch_id if append and old is not None else str(uuid4()),
                revision=(old.cursor.revision + bool(delta)) if append and old is not None else 1,
                projection_id=projection_id,
            )
            classes = tuple(
                sorted(
                    {
                        segment.epistemic.classification
                        for segment in selected
                        if segment.epistemic is not None
                    }
                )
            )
            epistemic = EpistemicView(
                classifications=classes,
                narrative_sources=tuple(
                    segment.key
                    for segment in selected
                    if segment.epistemic is not None
                    and segment.epistemic.classification == "narrative"
                ),
                simulated_sources=tuple(
                    segment.key
                    for segment in selected
                    if segment.epistemic is not None
                    and segment.epistemic.classification == "simulated"
                ),
            )
            full = ContextProjection(
                id=projection_id,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                session_id=command.session_id,
                consumer_id=command.consumer_id,
                instance_id=command.instance_id,
                usage=command.usage,
                source_watermark=tx.commit_seq,
                segments=tuple(selected),
                epistemic_view=epistemic,
                cursor=cursor,
                config_digest=config_digest,
                recorded_at=tx.recorded_at,
                commit_seq=tx.commit_seq,
            )
            response = ManagedContextResponse(
                kind="append" if append else "reset",
                cursor=cursor,
                projection=full.model_copy(update={"segments": delta}),
            )
            preparation = ContextPreparation(
                id=str(uuid4()), state="prepared", request=command, response=response
            )
            value = {
                "preparation": preparation.model_dump(mode="json"),
                "full_projection": full.model_dump(mode="json"),
                "source_scope": scope,
            }
            await tx.connection.execute(
                preparations.insert().values(
                    id=preparation.id,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    track_id=track_id,
                    record=value,
                )
            )
            return tx.result(preparation)

    @staticmethod
    def _segment(record: CognitiveRecord) -> ContextSegment:
        epistemic = getattr(record.body, "epistemic", None)
        return ContextSegment(
            key=record.reference.exact(),
            text=body_text(record.body),
            sources=(CognitiveTarget(reference=record.reference),),
            dependencies=tuple(
                CognitiveTarget(reference=edge.reference)
                for edge in record.evidence
                if edge.reference is not None
            ),
            authority="recorded_cognition",
            provenance={
                "source_roots": list(record.source_roots),
                "revision": record.reference.revision,
            },
            epistemic=epistemic,
        )

    async def preparation(self, ctx: CallContext, preparation_id: str) -> ContextPreparation:
        async with self.authority.read(ctx) as view:
            value = await self._preparation_in(view, ctx, preparation_id)
            full = ContextProjection.model_validate(value["full_projection"])
            await self._targets(
                view, ctx, tuple(target for segment in full.segments for target in segment.sources)
            )
            return ContextPreparation.model_validate(value["preparation"])

    async def cancel_preparation(
        self, ctx: CallContext, preparation_id: str, operation_id: str
    ) -> ContextPreparation:
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="context.cancel_preparation",
            scope=ctx.require_subject(),
            payload={"id": preparation_id},
        ) as tx:
            if tx.replayed:
                return ContextPreparation.model_validate(tx.replay_result)
            value = await self._preparation_in(tx, ctx, preparation_id)
            preparation = ContextPreparation.model_validate(value["preparation"])
            if preparation.state == "admitted":
                raise DomainError(
                    "context_already_admitted", "Admitted context cannot be cancelled"
                )
            result = preparation.model_copy(update={"state": "cancelled"})
            value["preparation"] = TypeAdapter(JsonValue).validate_json(result.model_dump_json())
            await tx.connection.execute(
                update(preparations).where(preparations.c.id == preparation_id).values(record=value)
            )
            return tx.result(result)

    async def admit_managed(
        self, ctx: CallContext, command: ContextAdmissionCommand
    ) -> ContextAdmission:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="context.admit_managed",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return ContextAdmission.model_validate(tx.replay_result)
            value = await self._preparation_in(tx, ctx, command.preparation_id)
            preparation = ContextPreparation.model_validate(value["preparation"])
            if preparation.state != "prepared":
                raise DomainError(
                    "context_not_prepared", "Only a pending preparation can be admitted"
                )
            projection = ContextProjection.model_validate(value["full_projection"])
            expected = tuple(segment.key for segment in projection.segments)
            if command.applied_segment_ids != expected:
                raise DomainError(
                    "context_input_mismatch",
                    "Applied segment identities do not match the prepared complete input",
                )
            targets = tuple(target for segment in projection.segments for target in segment.sources)
            # The current caller scope and the accepted preparation scope must both hold.
            await self._targets(tx, ctx, targets)
            accepted_scope = TypeAdapter(tuple[str, ...] | None).validate_python(
                value["source_scope"]
            )
            await self._targets(
                tx, ctx.model_copy(update={"allowed_source_ids": accepted_scope}), targets
            )
            prior = (
                await tx.connection.execute(
                    select(tracks.c.record).where(tracks.c.id == projection.cursor.track_id)
                )
            ).scalar_one_or_none()
            if preparation.response.kind == "append":
                current = ContextProjection.model_validate(prior) if prior is not None else None
                if current is None or current.cursor != preparation.request.known_cursor:
                    raise DomainError(
                        "context_cursor_conflict",
                        "The applied context cursor no longer matches its admitted track",
                    )
            result = ContextAdmission(
                id=str(uuid4()),
                projection_id=projection.id,
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                session_id=projection.session_id,
                consumer_id=projection.consumer_id,
                instance_id=projection.instance_id,
                invocation_id=command.invocation_id,
                rendered_digest=command.rendered_digest,
                history_generation=command.history_generation,
                segments=expected,
                sources=targets,
                measurement=command.measurement,
                recorded_at=tx.recorded_at,
                commit_seq=tx.commit_seq,
            )
            references = [
                target.reference.model_dump(mode="json")
                for target in targets
                if isinstance(target, CognitiveTarget)
            ]
            record = {
                "id": result.id,
                "kind": "managed",
                "executor": projection.consumer_id,
                "purpose": projection.usage.purpose,
                "references": references,
                "token_upper_bound": command.measurement.input_tokens,
                "commit_seq": tx.commit_seq,
                "managed_admission": result.model_dump(mode="json"),
            }
            await tx.connection.execute(
                contexts.insert().values(
                    id=result.id,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    record=record,
                )
            )
            if prior is None:
                await tx.connection.execute(
                    tracks.insert().values(
                        id=projection.cursor.track_id,
                        namespace_id=ctx.namespace_id,
                        subject_id=ctx.subject_id,
                        record=projection.model_dump(mode="json"),
                    )
                )
            else:
                await tx.connection.execute(
                    update(tracks)
                    .where(tracks.c.id == projection.cursor.track_id)
                    .values(record=projection.model_dump(mode="json"))
                )
            value["preparation"] = TypeAdapter(JsonValue).validate_json(
                preparation.model_copy(update={"state": "admitted"}).model_dump_json()
            )
            await tx.connection.execute(
                update(preparations).where(preparations.c.id == preparation.id).values(record=value)
            )
            for target in targets:
                event = {
                    "id": str(uuid4()),
                    "context_id": result.id,
                    "target": target.model_dump(mode="json"),
                    "kind": "model_contact",
                    "invocation_id": result.invocation_id,
                    "commit_seq": tx.commit_seq,
                }
                await tx.connection.execute(
                    feedback.insert().values(id=event["id"], context_id=result.id, record=event)
                )
            return tx.result(result)

    async def report_managed(self, ctx: CallContext, command: ContextReport) -> ContextReportResult:
        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="context.report",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                return ContextReportResult.model_validate(tx.replay_result)
            value = (
                await tx.connection.execute(
                    select(contexts.c.record).where(
                        contexts.c.id == command.admission_id,
                        contexts.c.namespace_id == ctx.namespace_id,
                        contexts.c.subject_id == ctx.subject_id,
                    )
                )
            ).scalar_one_or_none()
            if value is None or "managed_admission" not in value:
                raise DomainError(
                    "context_admission_not_found", "Managed context admission is unavailable"
                )
            admission = ContextAdmission.model_validate(value["managed_admission"])
            if admission.invocation_id != command.invocation_id:
                raise DomainError(
                    "context_invocation_mismatch",
                    "Outcome belongs to a different actual invocation",
                )
            for use in command.uses:
                if use.target not in admission.sources:
                    raise DomainError(
                        "context_use_not_admitted",
                        "Reported use was not present in the admitted model input",
                    )
                await self._targets(tx, ctx, (use.target,))
                event = {
                    "id": use.event_id,
                    "context_id": admission.id,
                    "target": use.target.model_dump(mode="json"),
                    "kind": "actual_use",
                    "use_kind": use.kind,
                    "invocation_id": command.invocation_id,
                    "occurred_at": command.occurred_at.isoformat(),
                    "commit_seq": tx.commit_seq,
                }
                await tx.connection.execute(
                    feedback.insert().values(id=use.event_id, context_id=admission.id, record=event)
                )
            result = ContextReportResult(
                id=str(uuid4()),
                admission_id=admission.id,
                invocation_id=command.invocation_id,
                outcome=command.outcome,
                occurred_at=command.occurred_at,
                uses=command.uses,
                commit_seq=tx.commit_seq,
            )
            await tx.connection.execute(
                reports.insert().values(
                    id=result.id,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.subject_id,
                    admission_id=admission.id,
                    record=result.model_dump(mode="json"),
                )
            )
            return tx.result(result)

    async def purge_references_in(
        self, tx: WriteTx, ctx: CallContext, references: tuple[CognitiveRef, ...]
    ) -> None:
        """Erase cached source-derived text in the same source purge transaction."""
        from nous.material.service import materials

        retired = {reference.id for reference in references}
        original_rows = (
            await tx.connection.execute(
                select(materials.c.id, materials.c.record).where(
                    materials.c.namespace_id == ctx.namespace_id
                )
            )
        ).all()
        purged_materials = {str(row.id) for row in original_rows if row.record.get("purged", False)}

        def scrub(value: JsonValue) -> JsonValue:
            if isinstance(value, list):
                return [scrub(item) for item in value]
            if not isinstance(value, dict):
                return value
            result = {key: scrub(item) for key, item in value.items()}
            targets = value.get("sources", [])
            matches = False
            if isinstance(targets, list):
                for target in targets:
                    if isinstance(target, dict) and isinstance(
                        reference := target.get("reference"), dict
                    ):
                        identity = reference.get("id")
                        if (target.get("kind") == "cognition" and identity in retired) or (
                            target.get("kind") == "media" and identity in purged_materials
                        ):
                            matches = True
            if matches and "text" in result:
                result["text"] = ""
                result["provenance"] = {"purged": True}
            return result

        for table in (preparations, tracks):
            rows = (
                await tx.connection.execute(
                    select(table.c.id, table.c.record).where(
                        table.c.namespace_id == ctx.namespace_id,
                        table.c.subject_id == ctx.subject_id,
                    )
                )
            ).all()
            for row in rows:
                old = TypeAdapter(JsonValue).validate_python(row.record)
                cleaned = scrub(old)
                if cleaned != old:
                    if (
                        table is preparations
                        and isinstance(cleaned, dict)
                        and isinstance(preparation := cleaned.get("preparation"), dict)
                        and preparation.get("state") == "prepared"
                    ):
                        preparation["state"] = "cancelled"
                    await tx.connection.execute(
                        update(table).where(table.c.id == row.id).values(record=cleaned)
                    )
