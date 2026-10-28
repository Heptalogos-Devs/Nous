from __future__ import annotations

import math
from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    DomainError,
    UnitOutput,
)
from sqlalchemy import select

from nous.cognition.projection import body_text
from nous.cognition.schema import vectors
from nous.material.service import require_observation_in

from .schema import outputs

if TYPE_CHECKING:
    from .service import ImportService


async def embed_output(
    self: ImportService, ctx: CallContext, output_id: str, *, required: bool = True
):
    async with self.authority.read(ctx) as view:
        value = (
            await view.connection.execute(select(outputs.c.record).where(outputs.c.id == output_id))
        ).scalar_one()
        output = UnitOutput.model_validate(value)
        observation = await require_observation_in(view, ctx, output.observation_id)
        records = [await self.cognition.get_in(view, ctx, ref) for ref in output.cognition]
    if not records:
        return {"output_id": output_id, "embedded": 0}
    batch = await self.models.embed(
        ctx.model_copy(update={"request_id": f"{output_id}:embed"}),
        tuple(body_text(record.body) for record in records),
        source_ids=(observation["material_id"],),
    )
    if len(batch.vectors) != len(records) or any(
        len(vector) != batch.dimensions or not all(math.isfinite(x) for x in vector)
        for vector in batch.vectors
    ):
        raise DomainError(
            "invalid_embedding",
            "Embedding response has invalid count, dimensions, or coordinates",
        )
    async with self.authority.write(
        ctx,
        f"{output_id}:embed:{batch.space}",
        operation_kind="imports.embed",
        scope=ctx.require_subject(),
        payload={"output_id": output_id, "space": batch.space, "model": batch.model},
    ) as tx:
        if tx.replayed:
            return tx.replay_result
        await require_observation_in(tx, ctx, output.observation_id)
        inserted = 0
        for record, vector in zip(records, batch.vectors, strict=True):
            existing = (
                await tx.connection.execute(
                    select(vectors.c.model, vectors.c.dimensions).where(
                        vectors.c.reference == record.reference.exact(),
                        vectors.c.space == batch.space,
                    )
                )
            ).first()
            if existing is not None:
                if existing.model != batch.model or existing.dimensions != batch.dimensions:
                    raise DomainError(
                        "embedding_space_conflict",
                        "An embedding space cannot mix models or dimensions",
                    )
                continue
            await tx.connection.execute(
                vectors.insert().values(
                    reference=record.reference.exact(),
                    space=batch.space,
                    model=batch.model,
                    dimensions=batch.dimensions,
                    vector=list(vector),
                    call_id=batch.call_id,
                    commit_seq=tx.commit_seq,
                )
            )
            inserted += 1
        return tx.result({"output_id": output_id, "embedded": inserted, "space": batch.space})
