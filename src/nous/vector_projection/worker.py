"""Claim exact content once, compute bounded embedding batches, adopt live subscriptions."""

from __future__ import annotations

import math
from typing import TYPE_CHECKING

from nous_sdk.contracts import DomainError, EmbeddingBatch
from nous_sdk.contracts.vector_projection import ProjectionResult
from sqlalchemy import select, update
from sqlalchemy.dialects.sqlite import insert

from nous.cognition.schema import vectors
from nous.platform.models.generation import ModelResult

from .contracts import Content, Input, Plan, Request, TargetInput
from .inputs import resolve_input, retained_content_in
from .schema import claims, contents, requests, subscriptions
from .space_lifecycle import require_live_in

if TYPE_CHECKING:
    from .service import VectorProjectionService


async def prepare(self: VectorProjectionService, request: Request) -> Request:
    if request.plan is not None:
        return request
    ctx, resource = request.context, request.profile.resource_key()
    async with self.authority.read(ctx) as view:
        candidates: list[TargetInput] = []
        omitted: list[str] = []
        for target in request.targets:
            value = await resolve_input(
                self, view, ctx, target, resource, head_only=request.head_only
            )
            if value is None:
                omitted.append(target.exact())
            else:
                candidates.append(TargetInput(target=target, input=value))
        keys = tuple(candidate.input.key for candidate in candidates)
        owners = tuple(
            set(
                (
                    await view.connection.execute(
                        select(claims.c.request_id).where(claims.c.key.in_(keys))
                    )
                ).scalars()
            )
        )
    retired: set[str] = set()
    for identity in owners:
        if identity != request.id:
            owner = await self.read_request(identity)
            state = await self.execution().status(owner.context, owner.job_id)
            if state.status in {"ERROR", "CANCELLED", "MAX_RECOVERY_ATTEMPTS_EXCEEDED"}:
                retired.add(identity)
    async with self.authority.write(ctx) as tx:
        current = Request.model_validate(
            (
                await tx.connection.execute(
                    select(requests.c.record).where(requests.c.id == request.id)
                )
            ).scalar_one()
        )
        if current.purged:
            raise DomainError("source_purged", "Projection inputs were erased")
        targets: list[TargetInput] = []
        owned: dict[str, Input] = {}
        dependencies: set[str] = set()
        for candidate in candidates:
            actual = await resolve_input(
                self, tx, ctx, candidate.target, resource, head_only=request.head_only
            )
            if actual is None:
                omitted.append(candidate.target.exact())
                continue
            if actual != candidate.input:
                raise DomainError("projection_input_changed", "The exact projection input changed")
            targets.append(candidate)
            await tx.connection.execute(
                subscriptions.insert().values(
                    request_id=request.id,
                    reference=candidate.target.exact(),
                    content_key=actual.key,
                )
            )
            retained = await retained_content_in(
                self,
                tx,
                ctx,
                candidate.target,
                actual,
                request.profile,
                repair=request.replace_existing,
            )
            if retained is not None:
                await require_live_in(tx, request, retained.space)
                if request.replace_existing:
                    await tx.connection.execute(
                        contents.delete().where(contents.c.key == actual.key)
                    )
                if (
                    await tx.connection.scalar(
                        select(contents.c.key).where(
                            contents.c.namespace_id == ctx.namespace_id,
                            contents.c.key == actual.key,
                        )
                    )
                    is None
                ):
                    await tx.connection.execute(
                        contents.insert().values(
                            key=actual.key,
                            namespace_id=ctx.namespace_id,
                            record=retained.model_dump(mode="json"),
                            commit_seq=tx.commit_seq,
                        )
                    )
                continue
            owner_id = await tx.connection.scalar(
                select(claims.c.request_id).where(claims.c.key == actual.key)
            )
            if owner_id is None:
                await tx.connection.execute(
                    claims.insert().values(key=actual.key, request_id=request.id)
                )
                owner_id = request.id
            elif owner_id in retired:
                await tx.connection.execute(
                    update(claims)
                    .where(
                        claims.c.key == actual.key,
                        claims.c.request_id == owner_id,
                    )
                    .values(request_id=request.id)
                )
                owner_id = request.id
            if owner_id == request.id:
                owned[actual.key] = actual
            else:
                dependencies.add(str(owner_id))
        plan = Plan(
            targets=tuple(targets),
            owned=tuple(owned.values()),
            dependencies=tuple(sorted(dependencies)),
            omitted=tuple(dict.fromkeys(omitted)),
        )
        current = current.model_copy(update={"plan": plan})
        await tx.connection.execute(
            update(requests)
            .where(requests.c.id == request.id)
            .values(record=current.model_dump(mode="json"))
        )
        return current


async def save_batch(
    self: VectorProjectionService,
    request: Request,
    items: tuple[Input, ...],
    result: ModelResult[EmbeddingBatch],
) -> None:
    batch = result.output
    if (
        len(items) != len(batch.vectors)
        or any(
            len(vector) != batch.dimensions or not all(math.isfinite(value) for value in vector)
            for vector in batch.vectors
        )
    ):
        raise DomainError(
            "invalid_embedding", "Embedding response has inconsistent vector dimensions"
        )
    ctx = request.context
    async with self.authority.write(ctx) as tx:
        current = Request.model_validate(
            (
                await tx.connection.execute(
                    select(requests.c.record).where(requests.c.id == request.id)
                )
            ).scalar_one()
        )
        if current.purged:
            raise DomainError("source_purged", "Projection input was erased during embedding")
        await require_live_in(tx, request, batch.space)
        assert current.plan is not None
        prior = (
            await tx.connection.execute(
                select(contents.c.record).where(
                    contents.c.namespace_id == ctx.namespace_id,
                    contents.c.key.in_(target.input.key for target in current.plan.targets)
                )
            )
        ).scalars()
        expected = (batch.space, batch.model, batch.dimensions)
        if any(
            (content.space, content.model, content.dimensions) != expected
            for content in (Content.model_validate(raw) for raw in prior)
        ):
            raise DomainError(
                "embedding_space_conflict",
                "Provider changed the embedding space within the projection directory",
            )
        for item, vector in zip(items, batch.vectors, strict=True):
            owner = await tx.connection.scalar(
                select(claims.c.request_id).where(claims.c.key == item.key)
            )
            if owner != request.id:
                raise DomainError(
                    "projection_claim_lost", "The content claim belongs to another attempt"
                )
            if (
                await tx.connection.scalar(
                    select(contents.c.key).where(
                        contents.c.namespace_id == ctx.namespace_id,
                        contents.c.key == item.key,
                    )
                )
                is None
            ):
                content = Content(
                    input=item,
                    space=batch.space,
                    model=batch.model,
                    dimensions=batch.dimensions,
                    vector=vector,
                    producer=result.producer,
                )
                await tx.connection.execute(
                    contents.insert().values(
                        key=item.key,
                        namespace_id=ctx.namespace_id,
                        record=content.model_dump(mode="json"),
                        commit_seq=tx.commit_seq,
                    )
                )
        {'space': batch.space, 'count': len(items)}


async def run(self: VectorProjectionService, identity: str) -> dict[str, object]:
    return await run_route(self, identity)


async def run_route(self: VectorProjectionService, identity: str) -> dict[str, object]:
    request = await self.prepare(await self.read_request(identity))
    assert request.plan is not None
    ctx, plan = request.context, request.plan
    from nous.platform.models.embedding import vector_space

    async with self.authority.read(ctx) as view:
        spaces = {
            vector_space(request.profile.route, request.profile.route.model),
            *(
                space.space
                for space in await self.models.configured_embedding_spaces_in(
                    view, ctx, request.profile
                )
            ),
        }
        for space in spaces:
            await require_live_in(view, request, space)
    text = tuple(item for item in plan.owned if item.text is not None)
    media = tuple(item for item in plan.owned if item.media is not None)
    # ModelResult is the actual business result; DBOS owns durable retry state.
    for kind, items in (("text", text), ("media", media)):
        if not items:
            continue
        async with self.authority.read(ctx) as view:
            for target in plan.targets:
                if target.input.key in {item.key for item in items}:
                    actual = await resolve_input(
                        self,
                        view,
                        ctx,
                        target.target,
                        request.profile.resource_key(),
                        head_only=request.head_only,
                    )
                    if actual != target.input:
                        raise DomainError(
                            "projection_input_changed", "Projection source is no longer live"
                        )
        sources = tuple(sorted({source for item in items for source in item.source_ids}))
        if kind == "text":
            result = await self.models.embed(
                ctx,
                tuple(item.text for item in items if item.text is not None),
                source_ids=sources,
                profile=request.profile,
                allow_fallback=bool(request.profile.fallbacks),
            )
        else:
            result = await self.models.embed_media(
                ctx,
                tuple((item.media,) for item in items if item.media is not None),
                source_ids=sources,
                profile=request.profile,
                allow_fallback=bool(request.profile.fallbacks),
            )
        await save_batch(self, request, items, result)
    for dependency in plan.dependencies:
        owner = await self.read_request(dependency)
        await self.execution().wait(owner.context, owner.job_id)
    result = await self.adopt(request)
    return result.model_dump(mode="json")


async def adopt(self: VectorProjectionService, request: Request) -> ProjectionResult:
    assert request.plan is not None
    ctx, plan = request.context, request.plan
    async with self.authority.write(ctx) as tx:
        current = Request.model_validate(
            (
                await tx.connection.execute(
                    select(requests.c.record).where(requests.c.id == request.id)
                )
            ).scalar_one()
        )
        if current.purged:
            raise DomainError("source_purged", "Projection sources were erased")
        directory: list[Content] = []
        for target in plan.targets:
            raw = await tx.connection.scalar(
                select(contents.c.record).where(
                    contents.c.namespace_id == ctx.namespace_id,
                    contents.c.key == target.input.key,
                )
            )
            if raw is not None:
                directory.append(Content.model_validate(raw))
        identities = {(item.space, item.model, item.dimensions) for item in directory}
        if len(identities) > 1:
            raise DomainError(
                "embedding_space_conflict",
                "The projection directory contains different embedding spaces",
            )
        adopted: list[str] = []
        omitted = list(plan.omitted)
        spaces: set[str] = set()
        for target in plan.targets:
            actual = await resolve_input(
                self,
                tx,
                ctx,
                target.target,
                request.profile.resource_key(),
                head_only=request.head_only,
            )
            if actual is None:
                omitted.append(target.target.exact())
                continue
            if actual != target.input:
                raise DomainError(
                    "projection_input_changed", "Projection input differs from its subscription"
                )
            raw = (
                await tx.connection.execute(
                    select(contents.c.record).where(
                        contents.c.namespace_id == ctx.namespace_id,
                        contents.c.key == actual.key,
                    )
                )
            ).scalar_one()
            content = Content.model_validate(raw)
            await require_live_in(tx, request, content.space)
            if content.producer.task != "embedding" or content.producer.model_id != content.model:
                raise DomainError(
                    "embedding_producer_conflict", "Cached vector provenance differs from its model"
                )
            existing = (
                await tx.connection.execute(
                    select(vectors.c.model, vectors.c.dimensions).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference == target.target.exact(), vectors.c.space == content.space)
                )
            ).first()
            if (
                existing is not None
                and not request.replace_existing
                and (existing.model, existing.dimensions)
                != (
                    content.model,
                    content.dimensions,
                )
            ):
                raise DomainError(
                    "embedding_space_conflict",
                    "One space cannot mix model identities or dimensions",
                )
            if existing is None or request.replace_existing:
                await tx.connection.execute(
                    insert(vectors)
                    .values(
                        namespace_id=ctx.namespace_id,
                        reference=target.target.exact(),
                        space=content.space,
                        model=content.model,
                        dimensions=content.dimensions,
                        vector=list(content.vector),
                        producer=content.producer.model_dump(mode="json"),
                        commit_seq=tx.commit_seq,
                    )
                    .on_conflict_do_update(
                        index_elements=["namespace_id", "reference", "space"],
                        set_={
                            "model": content.model,
                            "dimensions": content.dimensions,
                            "vector": list(content.vector),
                            "producer": content.producer.model_dump(mode="json"),
                            "commit_seq": tx.commit_seq,
                        },
                    )
                )
            adopted.append(target.target.exact())
            spaces.add(content.space)
        from nous.retrieval.maintenance import enqueue_index_in

        await enqueue_index_in(tx, ctx)
        return ProjectionResult(request_id=request.id, references=tuple(adopted), omitted=tuple(dict.fromkeys(omitted)), spaces=tuple(sorted(spaces)))
