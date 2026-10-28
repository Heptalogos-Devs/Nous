from __future__ import annotations

from typing import TYPE_CHECKING

from nous_wave.contracts import (
    CallContext,
    CognitiveRecord,
    ConceptBody,
    DomainError,
    EntityBody,
    EpisodeBody,
    QueryRequest,
)
from sqlalchemy import select

from nous.cognition.projection import references_in
from nous.cognition.schema import revisions, vectors
from nous.identity.service import require_subject_in
from nous.material.service import SourceRecord, observations

from .assets import (
    VECTOR_ROWS,
    CandidateView,
    RetrievalRecord,
    record_text,
)
from .media import media_records_in
from .planning import BoundPlan, time_matches

if TYPE_CHECKING:
    from .service import RetrievalService


async def eligible(
    self: RetrievalService, ctx: CallContext, request: QueryRequest, plan: BoundPlan
) -> CandidateView:
    async with self.authority.read(ctx) as view:
        await require_subject_in(view, ctx)
        statement = select(revisions.c.record).where(
            revisions.c.namespace_id == ctx.namespace_id,
            revisions.c.subject_id == ctx.subject_id,
        )
        if request.as_of_commit is not None:
            statement = statement.where(revisions.c.commit_seq <= request.as_of_commit)
        rows = [
            CognitiveRecord.model_validate(row)
            for row in (await view.connection.execute(statement)).scalars()
        ]
        if "asof_time" in plan:
            rows = [row for row in rows if row.times.recorded.coordinate <= plan["asof_time"]]
        exact = bool(request.reference and "@" in request.reference) or any(
            cue.get("selector") == "ref"
            and any("@" in locator for locator in cue.get("locators", []))
            for cue in plan["selectors"]
        )
        if not request.history and not exact:
            heads: dict[str, CognitiveRecord] = {}
            for row in rows:
                if (
                    row.reference.id not in heads
                    or heads[row.reference.id].reference.revision < row.reference.revision
                ):
                    heads[row.reference.id] = row
            rows = list(heads.values())
        candidates: list[RetrievalRecord] = [
            *rows,
            *(await media_records_in(view, ctx, self.materials)),
        ]
        roots = await self.cognition.active_roots_in(view, ctx)
        allowed: list[RetrievalRecord] = []
        source_rows: dict[str, SourceRecord] = {
            row["id"]: row["record"]
            for row in (
                await view.connection.execute(
                    select(observations.c.id, observations.c.record).where(
                        observations.c.namespace_id == ctx.namespace_id,
                        observations.c.subject_id == ctx.subject_id,
                    )
                )
            ).mappings()
        }
        for row in candidates:
            if request.as_of_commit is not None and row.commit_seq > request.as_of_commit:
                continue
            if "asof_time" in plan and row.times.recorded.coordinate > plan["asof_time"]:
                continue
            if row.deleted and not request.include_deleted:
                continue
            if (
                not request.include_inactive
                and not exact
                and isinstance(row, CognitiveRecord)
                and isinstance(row.body, (ConceptBody, EpisodeBody))
                and row.body.state != "active"
            ):
                continue
            if (
                isinstance(row, CognitiveRecord)
                and row.lifecycle is not None
                and row.lifecycle.activation != "active"
                and not request.include_inactive
                and not exact
            ):
                continue
            if not time_matches(row, request.times):
                continue
            if not await self.cognition.visible_in(view, ctx, row, roots):
                continue
            live = [oid for oid in row.source_roots if oid in roots]
            if request.source_ids is not None and not all(
                oid in request.source_ids or roots[oid] in request.source_ids for oid in live
            ):
                continue
            if request.source_type and not any(
                source_rows[oid]["source_type"] == request.source_type for oid in live
            ):
                continue
            if any(source_rows[oid]["source_type"] in plan["exclude_sources"] for oid in live):
                continue
            allowed.append(row)
        deduped: dict[str, RetrievalRecord] = {}
        for item in allowed:
            previous = deduped.get(item.reference.exact())
            if previous is None or item.commit_seq > previous.commit_seq:
                deduped[item.reference.exact()] = item
        allowed = list(deduped.values())
        assets = VECTOR_ROWS.validate_python(
            (await view.connection.execute(select(vectors))).mappings().all()
        )
        return CandidateView(allowed, assets, view.commit_seq, source_rows, roots)


def selectors(
    self: RetrievalService,
    records: list[RetrievalRecord],
    plan: BoundPlan,
    sources: dict[str, SourceRecord],
) -> tuple[list[RetrievalRecord], bool]:
    candidates, direct = records, False
    for cue in plan["selectors"]:
        if cue["kind"] != "selector":
            continue
        selector, locators = cue.get("selector", ""), cue.get("locators", [])
        matched: list[set[str]] = []
        for locator in locators:
            if selector == "ref":
                values = [
                    record
                    for record in records
                    if record.reference.exact() == locator
                    or f"{record.reference.kind}:{record.reference.id}" == locator
                ]
                if "@" not in locator and values:
                    values = [
                        max(
                            values,
                            key=lambda record: (
                                record.reference.revision
                                if isinstance(record, CognitiveRecord)
                                else record.commit_seq
                            ),
                        )
                    ]
                direct = True
            elif selector == "object":
                values = [
                    record
                    for record in records
                    if any(
                        oid in sources and locator in sources[oid]["origin"].values()
                        for oid in record.source_roots
                    )
                ]
            elif selector == "resource":
                values = [
                    record
                    for record in records
                    if any(
                        oid in sources
                        and sources[oid]["material_id"] == locator.removeprefix("material:")
                        for oid in record.source_roots
                    )
                ]
            else:
                kind = {"tag": "concept", "entity": "entity", "schema": "schema"}.get(selector)
                values = [
                    record
                    for record in records
                    if record.reference.kind == kind
                    and locator.casefold()
                    in [
                        str(
                            getattr(
                                getattr(record, "body", None),
                                "label",
                                getattr(getattr(record, "body", None), "title", ""),
                            )
                        ).casefold(),
                        *(
                            str(alias).casefold()
                            for alias in getattr(getattr(record, "body", None), "aliases", ())
                        ),
                        record.reference.exact().casefold(),
                    ]
                ]
                if len(values) > 1:
                    raise DomainError(
                        "ambiguous_name",
                        "Selector matches multiple cognitive objects",
                        candidates=[value.reference.model_dump() for value in values],
                    )
                if values:
                    target_ids = {value.reference.id for value in values}
                    explicit_members = {
                        ref.id
                        for value in values
                        if isinstance(value, CognitiveRecord)
                        and isinstance(value.body, (ConceptBody, EntityBody))
                        for ref in (
                            value.body.members
                            if isinstance(value.body, ConceptBody)
                            else tuple(mention.reference for mention in value.body.mentions)
                        )
                    }
                    values = [
                        record
                        for record in records
                        if record.reference.id in target_ids | explicit_members
                        or any(
                            ref.id in target_ids
                            for ref in references_in(
                                record.body if isinstance(record, CognitiveRecord) else None
                            )
                        )
                        or any(
                            link.reference and link.reference.id in target_ids
                            for link in (
                                record.evidence if isinstance(record, CognitiveRecord) else ()
                            )
                        )
                    ]
            if not values:
                raise DomainError("selector_not_found", f"No accessible object matches {locator}")
            matched.append({value.reference.exact() for value in values})
        selected = matched[0].intersection(*matched[1:]) if matched else set[str]()
        candidates = [record for record in candidates if record.reference.exact() in selected]
    return candidates, direct


def order(
    self: RetrievalService, ranked: list[tuple[RetrievalRecord, float]], plan: BoundPlan
) -> list[tuple[RetrievalRecord, float]]:
    for preference in plan["preferences"]:
        text = preference.get("text")
        if text:
            direction = -1 if preference["negative"] else 1
            ranked = [
                (
                    record,
                    score + direction * 0.1 * (text.casefold() in record_text(record).casefold()),
                )
                for record, score in ranked
            ]
    ranked.sort(key=lambda item: (-item[1], item[0].reference.exact()))
    sort = plan["sort"]
    if sort:

        def key(item: tuple[RetrievalRecord, float]) -> tuple[bool, int, float, str]:
            value = getattr(item[0].times, sort["axis"])
            coordinate = (
                value.start
                if sort["axis"] == "validity" and value
                else value.coordinate
                if value
                else None
            )
            known = value is not None and value.domain == sort["domain"] and coordinate is not None
            return (
                not known,
                -int(coordinate)
                if known and coordinate is not None and sort["direction"] == "desc"
                else int(coordinate)
                if known and coordinate is not None
                else 0,
                -item[1],
                item[0].reference.exact(),
            )

        ranked.sort(key=key)
    return ranked
