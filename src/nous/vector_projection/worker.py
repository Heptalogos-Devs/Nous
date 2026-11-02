"""Claim exact content once, compute bounded paid batches, adopt live subscriptions."""

from __future__ import annotations

import math
from typing import TYPE_CHECKING

from nous_sdk.contracts import DomainError, EmbeddingBatch
from nous_sdk.contracts.model_routing import ModelCallOutcome
from nous_sdk.contracts.vector_projection import ProjectionResult
from sqlalchemy import select, update
from sqlalchemy.dialects.sqlite import insert

from nous.cognition.schema import vectors
from nous.platform.errors import NousError
from nous.platform.models.outcomes import may_continue

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
    async with self.authority.write(
        ctx,
        request.id + ":prepare",
        operation_kind="vectors.prepare",
        scope=ctx.namespace_id,
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
                        select(contents.c.key).where(contents.c.key == actual.key)
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
        scope=ctx.namespace_id,
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
        await require_live_in(tx, request, batch.space)
        assert current.plan is not None
        prior = (
            await tx.connection.execute(
                select(contents.c.record).where(
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
                        record=content.model_dump(mode="json"),
                        commit_seq=tx.commit_seq,
                    )
                )
        tx.result({"space": batch.space, "count": len(items)})


async def run(self: VectorProjectionService, identity: str) -> dict[str, object]:
    request = await self.read_request(identity)
    # Route selection covers the entire logical directory. A rejected later batch
    # restarts that directory in the next space; paid earlier batches stay retained.
    for index, route in enumerate((request.profile.route, *request.profile.fallbacks)):
        route_id = request.profile.route_ids[index] if request.profile.route_ids else None
        profile = request.profile.model_copy(
            update={
                "route": route,
                "fallbacks": (),
                "route_ids": (route_id,) if route_id is not None else (),
            }
        )
        attempt_id = f"{identity}:route:{index}"
        async with self.authority.write(
            request.context,
            attempt_id,
            operation_kind="vectors.route",
            scope=request.context.namespace_id,
            payload={
                "request_id": identity,
                "profile": profile.model_dump(mode="json", exclude_unset=True),
            },
        ) as tx:
            if not tx.replayed:
                attempt = request.model_copy(
                    update={
                        "id": attempt_id,
                        "profile": profile,
                        "plan": None,
                        "parent_request_id": request.id,
                    }
                )
                await tx.connection.execute(
                    requests.insert().values(
                        id=attempt.id,
                        namespace_id=request.context.namespace_id,
                        record=attempt.model_dump(mode="json"),
                        commit_seq=tx.commit_seq,
                    )
                )
                tx.result({"request_id": attempt_id})
        try:
            result = await run_route(self, attempt_id)
            result["request_id"] = identity
            return result
        except DomainError as error:
            if error.code != "model_unavailable" or not request.profile.fallback_policy.unavailable:
                raise
            if index == len(request.profile.fallbacks):
                raise
        except NousError as error:
            attempts = error.context.get("attempts")
            if error.code != "model_routes_exhausted" or not isinstance(attempts, list):
                raise
            if not attempts or any(
                not isinstance(item, dict)
                or not may_continue(
                    request.profile.fallback_policy,
                    ModelCallOutcome.model_validate(item.get("outcome")),
                )
                for item in attempts
            ):
                raise
            if index == len(request.profile.fallbacks):
                raise
    raise RuntimeError("Projection has no embedding route")


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
    # The model execution owner performs physical batching and retains each call.
    for kind, items in (("text", text), ("media", media)):
        if not items:
            continue
        invocation = f"projection:{request.id}:{kind}"
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
            batch = await self.models.embed(
                ctx,
                tuple(item.text for item in items if item.text is not None),
                source_ids=sources,
                invocation_id=invocation,
                profile=request.profile,
                retry_authorization=request.retry_operation_id,
            )
        else:
            batch = await self.models.embed_media(
                ctx,
                tuple((item.media,) for item in items if item.media is not None),
                source_ids=sources,
                invocation_id=invocation,
                profile=request.profile,
                retry_authorization=request.retry_operation_id,
            )
        await save_batch(self, request, items, batch, invocation)
    for dependency in plan.dependencies:
        owner = await self.read_request(dependency)
        await self.execution().wait(owner.context, owner.job_id)
    result = await self.adopt(request)
    return result.model_dump(mode="json")


async def adopt(self: VectorProjectionService, request: Request) -> ProjectionResult:
    assert request.plan is not None
    ctx, plan = request.context, request.plan
    async with self.authority.write(
        ctx,
        request.id + ":adopt",
        operation_kind="vectors.adopt",
        scope=ctx.namespace_id,
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
        directory: list[Content] = []
        for target in plan.targets:
            raw = await tx.connection.scalar(
                select(contents.c.record).where(contents.c.key == target.input.key)
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
            await require_live_in(tx, request, content.space)
            await self.models.successful_embedding_in(tx, ctx, content.call_id)
            existing = (
                await tx.connection.execute(
                    select(vectors.c.model, vectors.c.dimensions).where(
                        vectors.c.reference == target.target.exact(),
                        vectors.c.space == content.space,
                    )
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
                        reference=target.target.exact(),
                        space=content.space,
                        model=content.model,
                        dimensions=content.dimensions,
                        vector=list(content.vector),
                        call_id=content.call_id,
                        commit_seq=tx.commit_seq,
                    )
                    .on_conflict_do_update(
                        index_elements=["reference", "space"],
                        set_={
                            "model": content.model,
                            "dimensions": content.dimensions,
                            "vector": list(content.vector),
                            "call_id": content.call_id,
                            "commit_seq": tx.commit_seq,
                        },
                    )
                )
            adopted.append(target.target.exact())
            spaces.add(content.space)
            calls.add(content.call_id)
        from nous.retrieval.maintenance import enqueue_index_in

        await enqueue_index_in(tx, ctx)
        return tx.result(
            ProjectionResult(
                request_id=request.id,
                references=tuple(adopted),
                omitted=tuple(dict.fromkeys(omitted)),
                spaces=tuple(sorted(spaces)),
                call_ids=tuple(sorted(calls)),
            )
        )
