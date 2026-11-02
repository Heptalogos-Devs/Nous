"""Namespace vector storage and subject rebuilds share projection and authority ownership."""

from __future__ import annotations

import json
import math
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveRef, DomainError
from nous_sdk.contracts.common import utc_now
from nous_sdk.contracts.records import RecordsQuery
from nous_sdk.contracts.vector_projection import ProjectionReceipt, ProjectionResult
from nous_sdk.contracts.vector_spaces import (
    CheckVectorSpaces,
    DeleteVectorSpace,
    PersistenceState,
    RebuildVectors,
    RecoverVectors,
    RetryVectorProjection,
    VectorProjectionProgress,
    VectorRebuild,
    VectorRecovery,
    VectorSpace,
    VectorSpaceCheck,
    VectorSpaceDeleted,
    VectorSpaces,
)
from nous_sdk.presentation import body_text
from pydantic import JsonValue, TypeAdapter
from sqlalchemy import delete, func, select, update
from sqlalchemy.dialects.sqlite import insert

from nous.cognition.lifecycle import selectable
from nous.cognition.schema import revisions, vectors
from nous.identity.access import require_namespace_in
from nous.material.derived_schema import derived_regions, representations
from nous.material.service import materials, observations, regions
from nous.platform.execution import JobSpec
from nous.platform.persistence import ReadView, WriteTx
from nous.retrieval.media import media_records_in

from .contracts import Request, Target
from .inputs import resolve_input
from .schema import checks, claims, contents, requests, space_retirements, subscriptions

if TYPE_CHECKING:
    from nous.cognition.records import CognitiveRecords
    from nous.platform.configuration.service import ConfigurationService
    from nous.platform.models import ModelService
    from nous.platform.persistence import Authority

    from .service import VectorProjectionService


def valid_payload(value: JsonValue, dimensions: int) -> bool:
    return (
        dimensions > 0
        and isinstance(value, list)
        and len(value) == dimensions
        and all(isinstance(item, (int, float)) and math.isfinite(item) for item in value)
    )


class VectorSpaceService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        models: ModelService,
        configuration: ConfigurationService,
        projection: VectorProjectionService,
    ) -> None:
        self.authority, self.cognition, self.models = authority, cognition, models
        self.configuration, self.projection = configuration, projection

    async def eligible_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        *,
        include_media: bool = False,
        kinds: tuple[str, ...] = (),
    ) -> tuple[Target, ...]:
        await require_namespace_in(view, ctx)
        command = RecordsQuery(limit=1000)
        targets: list[Target] = []
        while True:
            page = await self.cognition.records_in(view, ctx, command)
            for entry in page.items:
                if not selectable(entry.record) or not body_text(entry.record.body).strip():
                    continue
                reference = entry.record.reference
                if kinds and reference.kind not in kinds:
                    continue
                targets.append(reference)
            if page.next_cursor is None:
                break
            command = command.model_copy(update={"cursor": page.next_cursor})
        if include_media:
            targets.extend(
                row.reference
                for row in await media_records_in(
                    view,
                    ctx,
                    self.projection.materials,
                    self.projection.derivations,
                    include_text=False,
                )
            )
        return tuple({target.exact(): target for target in targets}.values())

    async def references_in(self, view: ReadView | WriteTx, ctx: CallContext) -> set[str]:
        if ctx.source_groups():
            if ctx.namespace_id is None:
                raise DomainError(
                    "vector_scope_forbidden",
                    "Namespace vector management requires complete source scope",
                )
            return {ref.exact() for ref in await self.eligible_in(view, ctx, include_media=True)}
        statement = select(revisions.c.id, revisions.c.kind, revisions.c.revision).where(
            revisions.c.namespace_id == ctx.namespace_id
        )
        if ctx.namespace_id is not None:
            statement = statement.where(revisions.c.namespace_id == ctx.namespace_id)
        selected = {
            f"{row.kind}:{row.id}@{row.revision}"
            for row in (await view.connection.execute(statement)).all()
        }
        originals = select(observations.c.material_id).where(
            observations.c.namespace_id == ctx.namespace_id
        )
        if ctx.namespace_id is not None:
            originals = originals.where(observations.c.namespace_id == ctx.namespace_id)
        original_ids = set((await view.connection.scalars(originals)).all())
        if ctx.namespace_id is None:
            original_ids.update(
                (
                    await view.connection.scalars(
                        select(materials.c.id).where(materials.c.namespace_id == ctx.namespace_id)
                    )
                ).all()
            )
        selected.update("material:" + value for value in original_ids)
        selected.update(
            "source-region:" + value
            for value in (
                await view.connection.scalars(
                    select(regions.c.id).where(
                        regions.c.namespace_id == ctx.namespace_id,
                        regions.c.material_id.in_(original_ids),
                    )
                )
            ).all()
        )
        for table, prefix in (
            (representations, "representation"),
            (derived_regions, "derived-region"),
        ):
            query = select(table).where(table.c.namespace_id == ctx.namespace_id)
            if ctx.namespace_id is not None:
                query = query.where(table.c.namespace_id == ctx.namespace_id)
            for row in (await view.connection.execute(query)).mappings():
                identity = (
                    f"{row['id']}@{row['revision']}" if table is representations else str(row["id"])
                )
                selected.add(f"{prefix}:{identity}")
        return selected

    async def catalog(self, ctx: CallContext, *, include_media: bool = False) -> VectorSpaces:
        try:
            profile = await self.models.prepare_embedding(ctx)
        except DomainError as error:
            if error.code != "model_unavailable":
                raise
            profile = None
        configured = bool(profile and profile.route.model and profile.route.base_url)
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            configured_ids = (
                {
                    space.space: space
                    for space in await self.models.configured_embedding_spaces_in(
                        view, ctx, profile
                    )
                }
                if profile
                else {}
            )
            if ctx.namespace_id is not None:
                eligible = {
                    ref.exact()
                    for ref in await self.eligible_in(view, ctx, include_media=include_media)
                }
            else:
                eligible: set[str] = set()
            owned = await self.references_in(view, ctx)
            rows = (
                (
                    await view.connection.execute(
                        select(vectors).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference.in_(owned))
                    )
                )
                .mappings()
                .all()
            )
            grouped: dict[str, list[tuple[str, str, int, int, bool]]] = {}
            for row in rows:
                size = len(json.dumps(row["vector"], ensure_ascii=False).encode())
                grouped.setdefault(str(row["space"]), []).append(
                    (
                        str(row["reference"]),
                        str(row["model"]),
                        int(row["dimensions"]),
                        size,
                        valid_payload(
                            TypeAdapter(JsonValue).validate_python(row["vector"]),
                            int(row["dimensions"]),
                        ),
                    )
                )
            deleted = {
                str(row[0]): int(row[1])
                for row in (
                    await view.connection.execute(
                        select(space_retirements.c.space_id, space_retirements.c.commit_seq).where(
                            space_retirements.c.namespace_id == ctx.namespace_id
                        )
                    )
                ).all()
            }
            inventory: list[VectorSpace] = []
            for identity in sorted(set(grouped) | set(configured_ids) | set(deleted)):
                values = grouped.get(identity, [])
                assigned = configured_ids.get(identity)
                model = assigned.model if assigned else values[0][1] if values else ""
                dimension = assigned.dimensions if assigned else values[0][2] if values else None
                inventory.append(
                    VectorSpace(
                        space_id=identity,
                        model=model,
                        dimensions=dimension,
                        configured=assigned is not None,
                        route_ids=tuple(
                            profile.route_ids[index] for index in assigned.route_indexes
                        )
                        if assigned and profile and profile.route_ids
                        else (),
                        vector_count=len(values),
                        invalid_vectors=sum(not row[4] for row in values),
                        payload_bytes=sum(row[3] for row in values),
                        missing_references=len(eligible - {row[0] for row in values if row[4]})
                        if ctx.namespace_id
                        else None,
                        can_delete=ctx.namespace_id is None
                        and not ctx.source_groups()
                        and bool(values),
                        deleted_at_commit=deleted.get(identity),
                    )
                )
            return VectorSpaces(
                namespace_id=ctx.namespace_id,
                authority_commit_seq=view.commit_seq,
                spaces=tuple(inventory),
                eligible_references=len(eligible),
                configured_space_complete=not eligible
                or any(
                    eligible.issubset({row[0] for row in grouped.get(space, []) if row[4]})
                    for space in configured_ids
                )
                if configured and ctx.namespace_id
                else None,
                configured=configured,
                include_media=include_media,
            )

    async def delete(self, ctx: CallContext, command: DeleteVectorSpace) -> VectorSpaceDeleted:
        if ctx.namespace_id is not None or ctx.source_groups():
            raise DomainError(
                "vector_scope_forbidden",
                "Deleting shared vector storage requires complete Namespace management",
            )
        async with self.authority.write(ctx) as tx:
            await require_namespace_in(tx, ctx)
            owned = await self.references_in(tx, ctx)
            rows = list(
                (
                    await tx.connection.scalars(
                        select(vectors.c.reference).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference.in_(owned), vectors.c.space == command.space_id)
                    )
                ).all()
            )
            cached = list(
                (
                    await tx.connection.scalars(
                        select(contents.c.key).where(
                            contents.c.namespace_id == ctx.namespace_id,
                            contents.c.record["space"].as_string() == command.space_id,
                        )
                    )
                ).all()
            )
            await tx.connection.execute(
                delete(vectors).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference.in_(owned), vectors.c.space == command.space_id)
            )
            if cached:
                await tx.connection.execute(delete(contents).where(contents.c.key.in_(cached)))
                await tx.connection.execute(delete(claims).where(claims.c.key.in_(cached)))
                await tx.connection.execute(
                    delete(subscriptions).where(subscriptions.c.content_key.in_(cached))
                )
            await tx.connection.execute(
                insert(space_retirements)
                .values(
                    namespace_id=ctx.namespace_id,
                    space_id=command.space_id,
                    commit_seq=tx.commit_seq,
                    reason=command.reason,
                )
                .on_conflict_do_update(
                    index_elements=["namespace_id", "space_id"],
                    set_={"commit_seq": tx.commit_seq, "reason": command.reason},
                )
            )
            from nous.retrieval.maintenance import enqueue_index_in

            await enqueue_index_in(tx, ctx)
            result = VectorSpaceDeleted(space_id=command.space_id, vectors_deleted=len(rows), contents_deleted=len(cached), commit_seq=tx.commit_seq)
        return result

    async def rebuild(self, ctx: CallContext, command: RebuildVectors) -> VectorRebuild:
        profile = await self.models.prepare_embedding(ctx)
        if command.batch_size is not None:
            profile = profile.model_copy(
                update={
                    "route": profile.route.model_copy(
                        update={
                            "max_batch_items": min(
                                command.batch_size, profile.route.max_batch_items
                            )
                        }
                    ),
                    "fallbacks": tuple(
                        route.model_copy(
                            update={
                                "max_batch_items": min(command.batch_size, route.max_batch_items)
                            }
                        )
                        for route in profile.fallbacks
                    ),
                }
            )
        async with self.authority.write(ctx) as tx:
            eligible = await self.eligible_in(
                tx, ctx, include_media=command.include_media, kinds=command.kinds
            )
            configured_spaces = await self.models.configured_embedding_spaces_in(tx, ctx, profile)
            coverage = {space.space: set[str]() for space in configured_spaces}
            for row in (
                await tx.connection.execute(
                    select(vectors).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference.in_((ref.exact() for ref in eligible)), vectors.c.space.in_(coverage))
                )
            ).mappings():
                if valid_payload(
                    TypeAdapter(JsonValue).validate_python(row["vector"]), int(row["dimensions"])
                ):
                    coverage[str(row["space"])].add(str(row["reference"]))
            indexed = max(coverage.values(), key=len, default=set[str]())
            candidates = tuple(
                ref for ref in eligible if command.mode == "all" or ref.exact() not in indexed
            )
            selected_values: list[Target] = []
            for target in candidates[: command.limit]:
                value = await resolve_input(
                    self.projection, tx, ctx, target, profile.resource_key(), head_only=True
                )
                if value is None:
                    continue
                selected_values.append(target)
            selected = tuple(selected_values)
            receipt = None
            if selected and not command.dry_run:
                receipt = await self.projection.admit_in(
                    tx, ctx, selected, head_only=True, replace_existing=True, profile=profile
                )
            return VectorRebuild(mode=command.mode, references=selected, total_eligible=len(eligible), remaining=max(0, len(candidates) - len(selected)), projection=receipt, commit_seq=tx.commit_seq)

    async def recover(self, ctx: CallContext, command: RecoverVectors) -> VectorRecovery:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            stored = [
                Request.model_validate(raw)
                for raw in (
                    await view.connection.scalars(
                        select(requests.c.record)
                        .where(
                            requests.c.namespace_id == ctx.namespace_id,
                            requests.c.record["context"]["caller_id"].as_string() == ctx.caller_id,
                        )
                        .order_by(requests.c.commit_seq.desc())
                    )
                ).all()
            ]
            roots = stored
        resumed: list[ProjectionReceipt] = []
        remaining = command.limit
        for request in roots[: command.limit]:
            if not remaining:
                break
            if request.purged or request.context.source_groups() != ctx.source_groups():
                continue
            if len(request.targets) > remaining:
                continue
            if command.kinds and any(
                not isinstance(target, CognitiveRef) or target.kind not in command.kinds
                for target in request.targets
            ):
                continue
            status = await self.projection.execution().status(ctx, request.job_id)
            if status.status not in {"ERROR", "CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}:
                continue
            async with self.authority.read(ctx) as view:
                current_retry = await view.connection.scalar(select(requests.c.record).where(requests.c.id == request.id))
                retries = Request.model_validate(current_retry).retry_count
                if retries >= command.max_retry:
                    continue
            resumed.append(
                await self.retry(
                    ctx,
                    RetryVectorProjection(
                        operation_id=command.operation_id + ":retry:" + request.id,
                        request_id=request.id,
                    ),
                )
            )
            remaining -= len(request.targets)
        backfill = await self.rebuild(
            ctx,
            RebuildVectors(
                operation_id=command.operation_id + ":missing",
                kinds=command.kinds,
                limit=remaining,
            ),
        )
        return VectorRecovery(resumed=tuple(resumed), backfill=backfill)

    async def progress(self, ctx: CallContext, request_id: str) -> VectorProjectionProgress:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            raw = await view.connection.scalar(
                select(requests.c.record).where(
                    requests.c.id == request_id,
                    requests.c.namespace_id == ctx.namespace_id,
                )
            )
            if raw is None:
                raise DomainError("projection_not_found", "Vector projection is unavailable")
            request = Request.model_validate(raw)
            plans = [
                Request.model_validate(row)
                for row in (
                    await view.connection.scalars(
                        select(requests.c.record).where(
                            requests.c.namespace_id == ctx.namespace_id,
                            requests.c.record["job_id"].as_string() == request.job_id,
                        )
                    )
                ).all()
            ]
            keys = {
                target.input.key
                for attempt in plans
                if attempt.plan
                for target in attempt.plan.targets
            }
            prepared = int(
                await view.connection.scalar(
                    select(func.count())
                    .select_from(contents)
                    .where(contents.c.namespace_id == ctx.namespace_id, contents.c.key.in_(keys))
                )
                or 0
            )
        status = await self.projection.execution().status(ctx, request.job_id)
        result = (
            ProjectionResult.model_validate(
                await self.projection.execution().wait(ctx, request.job_id)
            )
            if status.status == "SUCCESS"
            else None
        )
        return VectorProjectionProgress(
            request_id=request.id,
            job_id=request.job_id,
            status=status.status,
            targets=len(request.targets),
            prepared_contents=prepared,
            adopted_references=len(result.references) if result else 0,
            spaces=result.spaces if result else (),
            error=str(status.error) if status.error else None,
        )

    async def retry(self, ctx: CallContext, command: RetryVectorProjection) -> ProjectionReceipt:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
        original = await self.progress(ctx, command.request_id)
        if original.status not in {"ERROR", "CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}:
            raise DomainError(
                "projection_not_failed", "Only failed or cancelled projections can resume"
            )
        async with self.authority.write(ctx) as tx:
            rows = (
                await tx.connection.execute(
                    select(requests.c.id, requests.c.record).where(
                        requests.c.namespace_id == ctx.namespace_id,
                        requests.c.record["job_id"].as_string() == original.job_id,
                    )
                )
            ).all()
            current = next(
                Request.model_validate(row.record) for row in rows if row.id == command.request_id
            )
            for target in current.targets:
                if isinstance(target, CognitiveRef):
                    await self.cognition.get_in(tx, ctx, target)
                else:
                    await self.projection.materials.provenance_in(tx, ctx, target)
            if (
                current.context.caller_id != ctx.caller_id
                or current.context.source_groups() != ctx.source_groups()
            ):
                raise DomainError(
                    "projection_retry_forbidden",
                    "Projection recovery requires its accepted caller and source scope",
                )
            job = await self.projection.execution().enqueue_in(
                tx, JobSpec('nous.vectors.project', 'embedding', args=(command.request_id,), scope=ctx.namespace_id)
            )
            for row in rows:
                value = Request.model_validate(row.record).model_copy(
                    update={"job_id": job.workflow_id, "retry_count": current.retry_count + 1}
                )
                await tx.connection.execute(
                    update(requests)
                    .where(requests.c.id == row.id)
                    .values(record=value.model_dump(mode="json"))
                )
            return ProjectionReceipt(request_id=command.request_id, job_id=job.workflow_id)

    async def check(self, ctx: CallContext, command: CheckVectorSpaces) -> VectorSpaceCheck:
        model_status = TypeAdapter(dict[str, JsonValue]).validate_python(
            await self.models.status(ctx)
        )
        role_values = model_status.get("roles")
        embedding = role_values.get("embedding") if isinstance(role_values, dict) else None
        routes = embedding.get("routes") if isinstance(embedding, dict) else None
        credential_ready = (
            any(isinstance(row, dict) and row.get("credential_ready") is True for row in routes)
            if isinstance(routes, list)
            else False
        )
        try:
            profile = await self.models.prepare_embedding(ctx)
        except DomainError as error:
            if error.code != "model_unavailable":
                raise
            profile = None
        configured = bool(profile and profile.route.model and profile.route.base_url)
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            if ctx.namespace_id is not None:
                await require_namespace_in(view, ctx)
            if ctx.namespace_id is not None:
                await require_namespace_in(view, ctx)
            owned = await self.references_in(view, ctx)
            rows = (
                (
                    await view.connection.execute(
                        select(vectors).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference.in_(owned))
                    )
                )
                .mappings()
                .all()
            )
            invalid = tuple(
                str(row["reference"])
                for row in rows
                if not valid_payload(
                    TypeAdapter(JsonValue).validate_python(row["vector"]), int(row["dimensions"])
                )
            )
            watermark = view.commit_seq
            text = command.sample_text
            sources: tuple[str, ...] = ()
            if command.sample_reference is not None:
                record = await self.cognition.get_in(view, ctx, command.sample_reference)
                text, sources = body_text(record.body), record.source_roots
        space = None
        producer = None
        if command.probe:
            if profile is None:
                raise DomainError("model_unavailable", "The embedding role is unavailable")
            if not text:
                raise DomainError(
                    "embedding_probe_input_required",
                    "An embedding probe requires actual supplied text or a real cognitive reference",
                )
            result = await self.models.embed(
                ctx,
                (text,),
                source_ids=sources,
                profile=profile,
                cache_policy="none",
            )
            space, producer = result.output.space, result.producer
        result = VectorSpaceCheck(
            checked_at=utc_now(),
            authority_commit_seq=watermark,
            checked_vectors=len(rows),
            invalid_references=invalid,
            configured=configured,
            credential_ready=credential_ready,
            provider_probed=command.probe,
            provider_ready=True if command.probe else None,
            probe_space=space,
            producer=producer,
        )
        async with self.authority.write(ctx) as tx:
            await tx.connection.execute(insert(checks).values(
                namespace_id=ctx.namespace_id, caller_id=ctx.caller_id,
                record=result.model_dump(mode="json"), commit_seq=tx.commit_seq,
            ).on_conflict_do_update(index_elements=[checks.c.namespace_id, checks.c.caller_id],
                set_={"record": result.model_dump(mode="json"), "commit_seq": tx.commit_seq}))
            return result

    async def backfill_status(self, ctx: CallContext) -> dict[str, int]:
        profile = await self.models.prepare_embedding(ctx)
        async with self.authority.read(ctx) as view:
            eligible = {
                target.exact() for target in await self.eligible_in(view, ctx, kinds=("memory",))
            }
            configured_spaces = await self.models.configured_embedding_spaces_in(view, ctx, profile)
            coverage = {space.space: set[str]() for space in configured_spaces}
            for row in (
                await view.connection.execute(
                    select(vectors).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference.in_(eligible), vectors.c.space.in_(coverage))
                )
            ).mappings():
                if valid_payload(
                    TypeAdapter(JsonValue).validate_python(row["vector"]), int(row["dimensions"])
                ):
                    coverage[str(row["space"])].add(str(row["reference"]))
            indexed = max(coverage.values(), key=len, default=set[str]())
            missing = eligible - indexed
            rows = (
                (
                    await view.connection.execute(
                        select(requests.c.record)
                        .where(
                            requests.c.namespace_id == ctx.namespace_id,
                            requests.c.record["context"]["caller_id"].as_string() == ctx.caller_id,
                        )
                        .order_by(requests.c.commit_seq.desc())
                    )
                )
                .scalars()
                .all()
            )
        states: dict[str, str] = {}
        seen_jobs: set[str] = set()
        for raw in rows:
            request = Request.model_validate(raw)
            if (
                request.job_id in seen_jobs
                or request.profile.resource_key() != profile.resource_key()
            ):
                continue
            targets = {target.exact() for target in request.targets}.intersection(
                missing
            ).difference(states)
            if not targets:
                continue
            seen_jobs.add(request.job_id)
            status = await self.projection.execution().status(ctx, request.job_id)
            label = (
                "running"
                if status.status == "PENDING"
                else "failed"
                if status.status in {"ERROR", "CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}
                else "pending"
            )
            states.update({target: label for target in targets})
        return {
            "done": len(indexed),
            "running": sum(value == "running" for value in states.values()),
            "failed": sum(value == "failed" for value in states.values()),
            "pending": len(missing)
            - sum(value in {"running", "failed"} for value in states.values()),
        }

    async def persistence(self, ctx: CallContext) -> PersistenceState:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            return PersistenceState(committed_sequence=view.commit_seq)

    async def last_check(self, ctx: CallContext) -> VectorSpaceCheck | None:
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            raw = await view.connection.scalar(
                select(checks.c.record).where(
                    checks.c.namespace_id == ctx.namespace_id,
                    checks.c.caller_id == ctx.caller_id,
                )
            )
            if raw is None:
                return None
            value = raw
            result = VectorSpaceCheck.model_validate(value)
            owned = await self.references_in(view, ctx)
            return result.model_copy(
                update={
                    "invalid_references": tuple(
                        ref for ref in result.invalid_references if ref in owned
                    )
                }
            )
