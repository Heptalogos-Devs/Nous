"""One projection admission and adoption owner, backed by the DBOS embedding queue."""

from __future__ import annotations

from nous_sdk.contracts import CallContext, CognitiveRef, MediaReference
from nous_sdk.contracts.vector_projection import (
    ProjectionReceipt,
    ProjectionResult,
    ProjectVectors,
)
from sqlalchemy import delete, select, update

from nous.cognition.models import ModelServices
from nous.cognition.records import CognitiveRecords
from nous.material.derived import DerivedService
from nous.material.service import MaterialService
from nous.platform.configuration.service import ConfigurationService
from nous.platform.models.embedding_profile import EmbeddingProfile
from nous.platform.persistence import Authority, WriteTx

from . import worker
from .admission import ProjectionAdmission
from .contracts import Content, Request, Target, VectorPolicy
from .inputs import resolve_input, retained_content_in
from .schema import claims, contents, requests, subscriptions


class VectorProjectionService:
    async def source_jobs(self, ctx: CallContext, targets: tuple[Target, ...]) -> tuple[str, ...]:
        async with self.authority.read(ctx) as view:
            for target in targets:
                if isinstance(target, CognitiveRef):
                    await self.cognition.get_in(view, ctx, target)
                else:
                    await self.materials.provenance_in(view, ctx, target)
            values = (
                await view.connection.execute(
                    select(requests.c.record).where(
                        requests.c.namespace_id == ctx.namespace_id,
                    )
                )
            ).scalars()
            selected = {target.exact() for target in targets}
            result: list[str] = []
            for value in values:
                request = Request.model_validate(value)
                if not request.purged and any(
                    target.exact() in selected for target in request.targets
                ):
                    result.append(request.job_id)
            return tuple(result)

    run = worker.run
    prepare = worker.prepare
    adopt = worker.adopt

    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        models: ModelServices,
        configuration: ConfigurationService,
        *,
        materials: MaterialService,
        derivations: DerivedService,
        admission: ProjectionAdmission,
    ) -> None:
        self.authority, self.cognition, self.models = authority, cognition, models
        self.configuration, self.materials = configuration, materials
        self.derivations = derivations
        self.admission = admission

    async def policy_in(self, tx: WriteTx, policy: VectorPolicy) -> None:
        await self.admission.policy_in(tx, policy)

    async def admit_in(
        self,
        tx: WriteTx,
        ctx: CallContext,
        targets: tuple[Target, ...],
        *,
        head_only: bool = False,
        replace_existing: bool = False,
        profile: EmbeddingProfile | None = None,
    ) -> ProjectionReceipt:
        return await self.admission.admit_in(
            tx,
            ctx,
            targets,
            head_only=head_only,
            replace_existing=replace_existing,
            profile=profile,
        )

    def execution(self):
        if self.authority.execution is None:
            raise RuntimeError("Vector projections require the durable execution owner")
        return self.authority.execution

    async def project(
        self,
        ctx: CallContext,
        targets: tuple[CognitiveRef | MediaReference, ...],
        operation_id: str,
        *,
        replace_existing: bool = False,
    ) -> ProjectionResult:
        receipt = await self.submit(
            ctx,
            ProjectVectors(operation_id=operation_id, targets=targets),
            replace_existing=replace_existing,
        )
        request = await self.read_request(receipt.request_id)
        return ProjectionResult.model_validate(await self.execution().wait(ctx, request.job_id))

    async def submit(
        self,
        ctx: CallContext,
        command: ProjectVectors,
        *,
        replace_existing: bool = False,
    ) -> ProjectionReceipt:
        from nous.identity.access import require_namespace_in

        async with self.authority.write(ctx) as tx:
            await require_namespace_in(tx, ctx)
            receipt = await self.admit_in(
                tx, ctx, command.targets, replace_existing=replace_existing
            )
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
            from nous_sdk.contracts import DomainError

            raise DomainError("source_purged", "Projection inputs were permanently erased")
        return request

    async def missing[Selected: Target](
        self,
        ctx: CallContext,
        targets: tuple[Selected, ...],
        *,
        head_only: bool = True,
        profile: EmbeddingProfile | None = None,
    ) -> tuple[Selected, ...]:
        profile = profile or await self.models.prepare_embedding(ctx)
        missing: list[Selected] = []
        from nous_sdk.contracts import DomainError

        async with self.authority.read(ctx) as view:
            for target in targets:
                value = await resolve_input(
                    self, view, ctx, target, profile.resource_key(), head_only=head_only
                )
                if value is not None:
                    content = await retained_content_in(
                        self, view, ctx, target, value, profile, repair=True
                    )
                    if content is None:
                        missing.append(target)
                    else:
                        from nous.cognition.schema import vectors

                        if (
                            await view.connection.scalar(
                                select(vectors.c.reference).where(vectors.c.namespace_id == ctx.namespace_id, vectors.c.reference == target.exact(), vectors.c.space == content.space)
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
            pass
