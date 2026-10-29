"""One projection admission and adoption owner, backed by the DBOS embedding queue."""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from hashlib import sha256
from weakref import WeakKeyDictionary

from nous_wave.contracts import CallContext, CognitiveRef, MediaReference
from nous_wave.contracts.vector_projection import (
    ProjectionReceipt,
    ProjectionResult,
    ProjectVectors,
    VectorProjectionPolicy,
)
from sqlalchemy import delete, select, update

from nous.cognition.models import ModelServices
from nous.cognition.service import CognitionService
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.execution import JobSpec
from nous.platform.persistence import Authority, WriteTx
from nous.platform.persistence.authority import canonical_json

from . import worker
from .contracts import Content, Request, Target, VectorPolicy
from .inputs import resolve_input, retained_content_in
from .schema import claims, contents, requests, subscriptions


class VectorProjectionService:
    run = worker.run
    prepare = worker.prepare
    adopt = worker.adopt

    def __init__(
        self,
        authority: Authority,
        cognition: CognitionService,
        models: ModelServices,
        configuration: ConfigurationService,
        *,
        materials: MaterialService,
        derivations: DerivedService,
        invalidate: Callable[[], Awaitable[None]],
    ) -> None:
        self.authority, self.cognition, self.models = authority, cognition, models
        self.configuration, self.materials = configuration, materials
        self.derivations, self.invalidate = derivations, invalidate
        self._policies: WeakKeyDictionary[WriteTx, VectorPolicy] = WeakKeyDictionary()

    def execution(self):
        if self.authority.execution is None:
            raise RuntimeError("Vector projections require the durable execution owner")
        return self.authority.execution

    async def policy_in(self, tx: WriteTx, policy: VectorPolicy) -> None:
        """Explicit import policy belongs to its adoption transaction, before adopt_in."""
        # Only an internal per-transaction decision; persisted requests carry the
        # actual targets/context. It is not mutable execution state or a cache.
        self._policies[tx] = policy

    async def changed_in(self, tx: WriteTx, ctx: CallContext, reference: CognitiveRef) -> None:
        policy = self._policies.get(tx)
        if policy == "disabled":
            return
        if policy is None:
            effective = await self.configuration.effective(ctx, "vector_projection")
            if VectorProjectionPolicy.model_validate(effective["value"]).mode == "disabled":
                self._policies[tx] = "disabled"
                return
            self._policies[tx] = "metadata_allowed"
        await self.admit_in(tx, ctx, (reference,), head_only=True)

    async def admit_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        targets: tuple[Target, ...],
        *,
        head_only: bool = False,
    ) -> ProjectionReceipt:
        if tx.authority is not self.authority or tx.replayed:
            raise ValueError("Projection admission requires the actual authority write")
        identity = sha256(
            canonical_json(
                (
                    tx.operation_key,
                    ctx.model_dump(mode="json"),
                    head_only,
                )
            ).encode()
        ).hexdigest()
        raw = await tx.connection.scalar(select(requests.c.record).where(requests.c.id == identity))
        if raw is not None:
            request = Request.model_validate(raw)
            unique = {target.exact(): target for target in (*request.targets, *targets)}
            request = request.model_copy(update={"targets": tuple(unique.values())})
            await tx.connection.execute(
                update(requests)
                .where(requests.c.id == identity)
                .values(record=request.model_dump(mode="json"))
            )
        else:
            profile = await self.models.prepare_embedding(ctx)
            job = await tx.enqueue(
                JobSpec(
                    workflow_name="nous.vectors.project",
                    queue_name="embedding",
                    args=(identity,),
                    workflow_id=f"vectors:{identity}",
                )
            )
            request = Request(
                id=identity,
                context=ctx,
                targets=tuple({target.exact(): target for target in targets}.values()),
                profile=profile,
                head_only=head_only,
                job_id=job.workflow_id,
            )
            await tx.connection.execute(
                requests.insert().values(
                    id=identity,
                    namespace_id=ctx.namespace_id,
                    subject_id=ctx.require_subject(),
                    record=request.model_dump(mode="json"),
                    commit_seq=tx.commit_seq,
                )
            )
        return ProjectionReceipt(request_id=request.id, job_id=request.job_id)

    async def project(
        self,
        ctx: CallContext,
        targets: tuple[CognitiveRef | MediaReference, ...],
        operation_id: str,
    ) -> ProjectionResult:
        receipt = await self.submit(ctx, ProjectVectors(operation_id=operation_id, targets=targets))
        return ProjectionResult.model_validate(await self.execution().wait(ctx, receipt.job_id))

    async def submit(self, ctx: CallContext, command: ProjectVectors) -> ProjectionReceipt:
        from nous.identity.service import require_subject_in

        async with self.authority.write(
            ctx,
            command.operation_id,
            operation_kind="vectors.submit",
            scope=ctx.require_subject(),
            payload=command,
        ) as tx:
            if tx.replayed:
                receipt = ProjectionReceipt.model_validate(tx.replay_result)
            else:
                await require_subject_in(tx, ctx)
                receipt = await self.admit_in(tx, ctx, command.targets)
                tx.result(receipt)
        return receipt

    async def read_request(self, identity: str) -> Request:
        async with self.authority.read() as view:
            raw = (
                await view.connection.execute(
                    select(requests.c.record).where(requests.c.id == identity)
                )
            ).scalar_one()
        request = Request.model_validate(raw)
        if request.purged:
            from nous_wave.contracts import DomainError

            raise DomainError("source_purged", "Projection inputs were permanently erased")
        return request

    async def missing(
        self, ctx: CallContext, targets: tuple[CognitiveRef, ...]
    ) -> tuple[CognitiveRef, ...]:
        profile = await self.models.prepare_embedding(ctx)
        missing: list[CognitiveRef] = []
        async with self.authority.read(ctx) as view:
            for target in targets:
                value = await resolve_input(
                    self, view, ctx, target, profile.resource_key(), head_only=True
                )
                if value is not None:
                    content = await retained_content_in(self, view, ctx, target, value, profile)
                    if content is None:
                        missing.append(target)
                    else:
                        from nous.cognition.schema import vectors

                        if (
                            await view.connection.scalar(
                                select(vectors.c.reference).where(
                                    vectors.c.reference == target.exact(),
                                    vectors.c.space == content.space,
                                )
                            )
                            is None
                        ):
                            missing.append(target)
        return tuple(missing)

    async def purge_materials_in(self, tx: WriteTx, material_ids: tuple[str, ...]) -> None:
        selected = set(material_ids)
        keys: list[str] = []
        for raw in (
            await tx.connection.execute(
                select(contents.c.record).where(
                    contents.c.namespace_id == tx.ctx.namespace_id,
                )
            )
        ).scalars():
            content = Content.model_validate(raw)
            if selected.intersection(content.call_source_ids):
                keys.append(content.input.key)
        if keys:
            await tx.connection.execute(delete(contents).where(contents.c.key.in_(keys)))
            await tx.connection.execute(delete(claims).where(claims.c.key.in_(keys)))
        erased: set[str] = set()
        for raw in (
            await tx.connection.execute(
                select(requests.c.record).where(
                    requests.c.namespace_id == tx.ctx.namespace_id,
                )
            )
        ).scalars():
            request = Request.model_validate(raw)
            if request.plan and any(
                selected.intersection(target.input.source_ids) for target in request.plan.targets
            ):
                erased.add(request.id)
                await tx.connection.execute(
                    update(requests)
                    .where(requests.c.id == request.id)
                    .values(
                        record=request.model_copy(
                            update={"targets": (), "plan": None, "purged": True}
                        ).model_dump(mode="json")
                    )
                )
                await tx.connection.execute(
                    delete(subscriptions).where(subscriptions.c.request_id == request.id)
                )
        if erased:
            await self.authority.redact_receipts_in(
                tx,
                lambda value: (
                    isinstance(value, dict)
                    and (
                        isinstance(value.get("id"), str)
                        and value["id"] in erased
                        or isinstance(value.get("request_id"), str)
                        and value["request_id"] in erased
                    )
                ),
            )
