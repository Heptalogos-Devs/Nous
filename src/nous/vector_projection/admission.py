"""Transactional projection admission, independent of cognitive readers and workers."""

from __future__ import annotations

from hashlib import sha256
from weakref import WeakKeyDictionary

from nous_sdk.contracts import CallContext, CognitiveRecord
from nous_sdk.contracts.vector_projection import ProjectionReceipt, VectorProjectionPolicy
from sqlalchemy import select, update

from nous.cognition.models import ModelServices
from nous.platform.configuration.service import ConfigurationService
from nous.platform.execution import JobSpec
from nous.platform.models.embedding_profile import EmbeddingProfile
from nous.platform.persistence import Authority, WriteTx
from nous.platform.persistence.authority import canonical_json

from .contracts import Request, Target, VectorPolicy
from .schema import requests


class ProjectionAdmission:
    def __init__(
        self, authority: Authority, models: ModelServices, configuration: ConfigurationService
    ):
        self.authority, self.models, self.configuration = authority, models, configuration
        self._policies: WeakKeyDictionary[WriteTx, VectorPolicy] = WeakKeyDictionary()

    async def policy_in(self, tx: WriteTx, policy: VectorPolicy) -> None:
        """Explicit import policy belongs to its adoption transaction, before adopt_in."""
        # Only an internal per-transaction decision; persisted requests carry the
        # actual targets/context. It is not mutable execution state or a cache.
        self._policies[tx] = policy

    async def changed_in(self, tx: WriteTx, ctx: CallContext, record: CognitiveRecord) -> None:
        from nous.cognition.lifecycle import selectable

        if (
            record.namespace_id != ctx.namespace_id
            or record.namespace_id != ctx.namespace_id
            or record.commit_seq != tx.commit_seq
        ):
            raise ValueError(
                "Automatic projection requires the actual newly written scoped revision"
            )
        if not selectable(record):
            return
        policy = self._policies.get(tx)
        if policy == "disabled":
            return
        if policy is None:
            effective = await self.configuration.effective(ctx, "vector_projection")
            if VectorProjectionPolicy.model_validate(effective["value"]).mode == "disabled":
                self._policies[tx] = "disabled"
                return
            configured = VectorProjectionPolicy.model_validate(effective["value"])
            if configured.kinds is not None and record.reference.kind not in configured.kinds:
                return
            self._policies[tx] = "metadata_allowed"
        await self.admit_in(tx, ctx, (record.reference,), head_only=True)

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
        if tx.authority is not self.authority:
            raise ValueError("Projection admission requires the actual authority write")
        identity = sha256(
            canonical_json(
                (
                    tx.commit_seq,
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
            profile = profile or await self.models.prepare_embedding(ctx)
            job = await tx.enqueue(
                JobSpec(
                    workflow_name="nous.vectors.project",
                    queue_name="embedding",
                    args=(identity,),
                    workflow_id=f"vectors:{identity}",
                    scope=identity,
                )
            )
            request = Request(
                id=identity,
                context=ctx,
                targets=tuple({target.exact(): target for target in targets}.values()),
                profile=profile,
                head_only=head_only,
                job_id=job.workflow_id,
                replace_existing=replace_existing,
                admitted_commit=tx.commit_seq,
            )
            await tx.connection.execute(
                requests.insert().values(
                    id=identity,
                    namespace_id=ctx.namespace_id,
                    record=request.model_dump(mode="json"),
                    commit_seq=tx.commit_seq,
                )
            )
        return ProjectionReceipt(request_id=request.id, job_id=request.job_id)
