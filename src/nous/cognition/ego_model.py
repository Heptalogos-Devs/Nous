"""Current sourced Ego facets, narrative and domain knowledge."""

from nous_sdk.contracts import CallContext, CognitiveRecord
from sqlalchemy import select

from nous.identity.access import require_namespace_in

from .lifecycle import selectable
from .records import CognitiveRecords
from .schema import revisions


class EgoService:
    def __init__(self, records: CognitiveRecords):
        self.records = records

    async def ego_profile(self, ctx: CallContext) -> tuple[CognitiveRecord, ...]:
        async with self.records.authority.read(ctx) as view:
            await require_namespace_in(view, ctx, "ego")
            rows = (
                await view.connection.execute(
                    select(revisions.c.record).where(
                        revisions.c.namespace_id == ctx.namespace_id,
                        revisions.c.head.is_(True),
                        revisions.c.kind.in_(
                            ("self_facet", "narrative_thread", "domain_knowledge")
                        ),
                    )
                )
            ).scalars()
            result: list[CognitiveRecord] = []
            for value in rows:
                record = CognitiveRecord.model_validate(value)
                if selectable(record) and await self.records.visible_in(view, ctx, record):
                    result.append(record)
            return tuple(result)
