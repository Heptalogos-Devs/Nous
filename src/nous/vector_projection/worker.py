"""Claim exact content once, compute bounded paid batches, adopt live subscriptions."""

from __future__ import annotations

import math
from typing import TYPE_CHECKING

from nous_wave.contracts import DomainError, EmbeddingBatch
from nous_wave.contracts.vector_projection import ProjectionResult
from sqlalchemy import select, update

from nous.cognition.schema import vectors

from .contracts import Content, Input, Plan, Request, TargetInput
from .inputs import resolve_input, retained_content_in
from .schema import claims, contents, requests, subscriptions

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
    async with self.authority.write(
        ctx,
        request.id + ":prepare",
        operation_kind="vectors.prepare",
        scope=ctx.require_subject(),
        payload={"request_id": request.id},
    ) as tx:
        if tx.replayed:
            return Request.model_validate(tx.replay_result)
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
                self, tx, ctx, candidate.target, actual, request.profile
            )
            if retained is not None:
                if (
                    await tx.connection.scalar(
                        select(contents.c.key).where(contents.c.key == actual.key)
                    )
                    is None
                ):
                    await tx.connection.execute(
                        contents.insert().values(
                            key=actual.key,
                            namespace_id=ctx.namespace_id,
                            subject_id=ctx.require_subject(),
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
            owned=tuple(owned[key] for key in sorted(owned)),
            dependencies=tuple(sorted(dependencies)),
            omitted=tuple(dict.fromkeys(omitted)),
        )
        current = current.model_copy(update={"plan": plan})
        await tx.connection.execute(
            update(requests)
            .where(requests.c.id == request.id)
            .values(record=current.model_dump(mode="json"))
        )
        return tx.result(current)


async def save_batch(
    self: VectorProjectionService,
    request: Request,
    items: tuple[Input, ...],
    batch: EmbeddingBatch,
    identity: str,
) -> None:
    if (
        len(items) != len(batch.vectors)
        or len(items) != len(batch.call_ids)
        or any(
            len(vector) != batch.dimensions or not all(math.isfinite(value) for value in vector)
            for vector in batch.vectors
        )
        or any(call is None for call in batch.call_ids)
    ):
        raise DomainError(
            "invalid_embedding", "Embedding response has inconsistent vectors or producers"
        )
    ctx = request.context
    async with self.authority.write(
        ctx,
        identity,
        operation_kind="vectors.content",
        scope=ctx.require_subject(),
        payload={"keys": [item.key for item in items], "space": batch.space},
    ) as tx:
        if tx.replayed:
            return
        current = Request.model_validate(
            (
                await tx.connection.execute(
                    select(requests.c.record).where(requests.c.id == request.id)
                )
            ).scalar_one()
        )
        if current.purged:
            raise DomainError("source_purged", "Projection input was erased during embedding")
        for item, vector, call_id in zip(items, batch.vectors, batch.call_ids, strict=True):
            assert call_id is not None
            await self.models.successful_embedding_in(tx, ctx, call_id)
            owner = await tx.connection.scalar(
                select(claims.c.request_id).where(claims.c.key == item.key)
            )
            if owner != request.id:
                raise DomainError(
                    "projection_claim_lost", "The content claim belongs to another attempt"
                )
            if (
                await tx.connection.scalar(select(contents.c.key).where(contents.c.key == item.key))
                is None
            ):
                content = Content(
                    input=item,
                    space=batch.space,
                    model=batch.model,
                    dimensions=batch.dimensions,
                    vector=vector,
                    call_id=call_id,
                    call_source_ids=tuple(
                        sorted({source for entry in items for source in entry.source_ids})
                    ),
                )
                await tx.connection.execute(
                    contents.insert().values(
                        key=item.key,
                        namespace_id=ctx.namespace_id,
                        subject_id=ctx.require_subject(),
                        record=content.model_dump(mode="json"),
                        commit_seq=tx.commit_seq,
                    )
                )
        tx.result({"space": batch.space, "count": len(items)})


async def run(self: VectorProjectionService, identity: str) -> dict[str, object]:
    request = await self.prepare(await self.read_request(identity))
    assert request.plan is not None
    ctx, plan = request.context, request.plan
    text = tuple(item for item in plan.owned if item.text is not None)
    media = tuple(item for item in plan.owned if item.media is not None)
    # The frozen directory fixes batch membership and order across DBOS recovery.
    for kind, items in (("text", text), ("media", media)):
        size = request.profile.route.max_batch_items if kind == "text" else 1
        for offset in range(0, len(items), size):
            group = items[offset : offset + size]
            invocation = f"projection:{request.id}:{kind}:{offset}"
            async with self.authority.read(ctx) as view:
                for target in plan.targets:
                    if target.input.key in {item.key for item in group}:
                        actual = await resolve_input(
                            self,
                            view,
                            ctx,
                            target.target,
                            request.profile.resource_key(),
                            head_only=False,
                        )
                        if actual != target.input:
                            raise DomainError(
                                "projection_input_changed", "Projection source is no longer live"
                            )
            sources = tuple(sorted({source for item in group for source in item.source_ids}))
            if kind == "text":
                batch = await self.models.embed(
                    ctx,
                    tuple(item.text for item in group if item.text is not None),
                    source_ids=sources,
                    invocation_id=invocation,
                    profile=request.profile,
                )
            else:
                batch = await self.models.embed_media(
                    ctx,
                    tuple((item.media,) for item in group if item.media is not None),
                    source_ids=sources,
                    invocation_id=invocation,
                    profile=request.profile,
                )
            await save_batch(self, request, group, batch, invocation)
    for dependency in plan.dependencies:
        owner = await self.read_request(dependency)
        await self.execution().wait(owner.context, owner.job_id)
    result = await self.adopt(request)
    await self.invalidate()
    return result.model_dump(mode="json")


async def adopt(self: VectorProjectionService, request: Request) -> ProjectionResult:
    assert request.plan is not None
    ctx, plan = request.context, request.plan
    async with self.authority.write(
        ctx,
        request.id + ":adopt",
        operation_kind="vectors.adopt",
        scope=ctx.require_subject(),
        payload={"request_id": request.id},
    ) as tx:
        if tx.replayed:
            return ProjectionResult.model_validate(tx.replay_result)
        current = Request.model_validate(
            (
                await tx.connection.execute(
                    select(requests.c.record).where(requests.c.id == request.id)
                )
            ).scalar_one()
        )
        if current.purged:
            raise DomainError("source_purged", "Projection sources were erased")
        adopted: list[str] = []
        omitted = list(plan.omitted)
        spaces: set[str] = set()
        calls: set[str] = set()
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
                    select(contents.c.record).where(contents.c.key == actual.key)
                )
            ).scalar_one()
            content = Content.model_validate(raw)
            await self.models.successful_embedding_in(tx, ctx, content.call_id)
            existing = (
                await tx.connection.execute(
                    select(vectors.c.model, vectors.c.dimensions).where(
                        vectors.c.reference == target.target.exact(),
                        vectors.c.space == content.space,
                    )
                )
            ).first()
            if existing is not None and (existing.model, existing.dimensions) != (
                content.model,
                content.dimensions,
            ):
                raise DomainError(
                    "embedding_space_conflict",
                    "One space cannot mix model identities or dimensions",
                )
            if existing is None:
                await tx.connection.execute(
                    vectors.insert().values(
                        reference=target.target.exact(),
                        space=content.space,
                        model=content.model,
                        dimensions=content.dimensions,
                        vector=list(content.vector),
                        call_id=content.call_id,
                        commit_seq=tx.commit_seq,
                    )
                )
            adopted.append(target.target.exact())
            spaces.add(content.space)
            calls.add(content.call_id)
        return tx.result(
            ProjectionResult(
                request_id=request.id,
                references=tuple(adopted),
                omitted=tuple(dict.fromkeys(omitted)),
                spaces=tuple(sorted(spaces)),
                call_ids=tuple(sorted(calls)),
            )
        )
