"""Bind explicit query expressions to material, concept and time identities."""

from __future__ import annotations

from datetime import datetime, timedelta
from typing import TYPE_CHECKING

from nous.application.contracts import CallContext
from nous.cognition.concept_contracts import ConceptRef, FindConcepts, ReadConcept
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import (
    MaterialRef,
)
from nous.common.errors import NousError
from nous.content.contracts import (
    ArtifactRef,
)
from nous.temporal.values import Instant, Interval, Known, TimePoint, Unbounded, parse_explicit_time
from nous.wave.contracts import (
    CandidateFilter,
    TimeCondition,
    WaveExpression,
)

from .contracts import (
    BoundQueryBranch,
    CognitiveGroupContext,
    QueryAny,
    QueryAtom,
    QueryControls,
    QueryExpression,
)

if TYPE_CHECKING:
    from nous.cognition.concepts import ConceptOperations
    from nous.cognition.materials import Materials
    from nous.temporal.domains import TimeService
    from nous.wave.operations import WaveOperations


def merge_controls(parent: QueryControls, child: QueryControls) -> QueryControls:
    filters = child.filters or parent.filters
    if parent.filters is not None and child.filters is not None:
        left, right = parent.filters.candidate_keys, child.filters.candidate_keys
        keys = (
            right
            if left is None
            else left
            if right is None
            else tuple(key for key in left if key in right)
        )
        filters = CandidateFilter(
            candidate_keys=keys,
            times=(*parent.filters.times, *child.filters.times),
            include_unknown=parent.filters.include_unknown and child.filters.include_unknown,
        )
    returned = (
        tuple(value for value in parent.return_types if value in child.return_types)
        if parent.return_types and child.return_types
        else child.return_types or parent.return_types
    )
    if parent.return_types and child.return_types and not returned:
        filters = (filters or CandidateFilter()).model_copy(update={"candidate_keys": ()})
    return QueryControls(
        selectors=(*parent.selectors, *child.selectors),
        limit=child.limit if child.limit is not None else parent.limit,
        return_types=returned,
        times=(*parent.times, *child.times),
        time_clauses=(*parent.time_clauses, *child.time_clauses),
        filters=filters,
        prefer=(*parent.prefer, *child.prefer),
        avoid=(*parent.avoid, *child.avoid),
        sort=child.sort or parent.sort,
        explore=parent.explore or child.explore,
        diagnostics=parent.diagnostics or child.diagnostics,
        include_review=child.include_review
        if child.include_review is not None
        else parent.include_review,
        include_exploration=child.include_exploration
        if child.include_exploration is not None
        else parent.include_exploration,
    )


class QueryBinder:
    """Bind one expression against a captured snapshot and reference time."""

    def __init__(
        self,
        materials: Materials,
        concepts: ConceptOperations,
        wave: WaveOperations,
        time: TimeService,
        reference_time: datetime,
    ):
        self.materials, self.concepts, self.wave, self.time = materials, concepts, wave, time
        self.reference_time = reference_time

    async def time_conditions(self, controls: QueryControls) -> tuple[TimeCondition, ...]:
        conditions = list(controls.times)
        for clause in controls.time_clauses:
            if (
                sum(
                    bool(value)
                    for value in (clause.at, clause.within, clause.from_value or clause.to_value)
                )
                != 1
            ):
                raise NousError(
                    "input.invalid_query", "Choose exactly one time range, at, or within"
                )
            if clause.within:
                if clause.domain != "utc":
                    raise NousError(
                        "time.clock_required", "A relative range requires a clock for this domain"
                    )
                raw = clause.within
                split = 0
                while split < len(raw) and raw[split].isdigit():
                    split += 1
                amount, unit = raw[:split], raw[split:]
                factors = {"ms": 0.001, "s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800}
                if not amount or unit not in factors or int(amount) < 1:
                    raise NousError("input.invalid_query", "Invalid relative time duration")
                now = self.reference_time
                start_value = parse_explicit_time(
                    (now - timedelta(seconds=int(amount) * factors[unit])).isoformat()
                )
                end_value = parse_explicit_time(now.isoformat())
                assert isinstance(start_value, Instant) and isinstance(end_value, Instant)
                interval = Interval(
                    start=Known(at=start_value.at), end=Known(at=end_value.at), meaning="duration"
                )
            elif clause.at:
                value = parse_explicit_time(
                    clause.at, clause.domain, "UTC" if clause.domain == "utc" else None
                )
                if isinstance(value, Interval):
                    interval = value
                elif isinstance(value, Instant):
                    interval = Interval(
                        start=Known(at=value.at),
                        end=Known(
                            at=TimePoint(domain_id=value.at.domain_id, ticks=value.at.ticks + 1)
                        ),
                        meaning="duration",
                    )
                else:
                    raise NousError("time.invalid_value", "An exact time is required")
            else:
                start_value = (
                    parse_explicit_time(
                        clause.from_value, clause.domain, "UTC" if clause.domain == "utc" else None
                    )
                    if clause.from_value
                    else None
                )
                end_value = (
                    parse_explicit_time(
                        clause.to_value, clause.domain, "UTC" if clause.domain == "utc" else None
                    )
                    if clause.to_value
                    else None
                )
                start = (
                    Known(at=start_value.at)
                    if isinstance(start_value, Instant)
                    else start_value.start
                    if isinstance(start_value, Interval)
                    else Unbounded()
                )
                end = (
                    Known(at=end_value.at)
                    if isinstance(end_value, Instant)
                    else end_value.start
                    if isinstance(end_value, Interval)
                    else Unbounded()
                )
                if (
                    clause.from_value
                    and start_value is not None
                    and not isinstance(start_value, (Instant, Interval))
                ):
                    raise NousError("time.invalid_value", "Invalid range start")
                if (
                    clause.to_value
                    and end_value is not None
                    and not isinstance(end_value, (Instant, Interval))
                ):
                    raise NousError("time.invalid_value", "Invalid range end")
                interval = Interval(start=start, end=end, meaning="duration")
            await self.time.validate((interval,))
            conditions.append(TimeCondition(axis=clause.axis, range=interval))
        return tuple(conditions)

    async def _bind(
        self, controls: QueryControls, namespace: NamespaceRef, snapshot: str, call: CallContext
    ) -> tuple[CandidateFilter | None, tuple[MaterialRef, ...], tuple[ConceptRef, ...]]:
        materials: list[MaterialRef] = []
        bound: list[MaterialRef] = []
        concepts: list[ConceptRef] = []
        resources: list[ArtifactRef] = []
        for selector in controls.selectors:
            if selector.kind == "ref":
                resolved = await self.materials.resolve_identifier(
                    selector.value, namespace=namespace
                )
                bound.append(resolved)
                materials.append(resolved)
            elif selector.kind == "resource":
                resolved = await self.materials.resolve_identifier(
                    selector.value, namespace=namespace
                )
                bound.append(resolved)
                view = await self.materials.resolve(resolved, namespace=namespace)
                if view.body is None:
                    raise NousError("material.no_content", "Selected resource has no content")
                resources.append(view.body.artifact)
            elif selector.kind == "tag":
                if selector.value.startswith("con("):
                    found = (
                        await self.concepts.read(
                            ReadConcept(target=selector.value, namespace=namespace), call
                        ),
                    )
                else:
                    options = await self.concepts.find(
                        FindConcepts(namespace=namespace, text=selector.value, limit=100), call
                    )
                    found = tuple(
                        item
                        for item in options
                        if selector.value.casefold()
                        in {item.label.casefold(), *(alias.casefold() for alias in item.aliases)}
                    )
                if len(found) != 1:
                    raise NousError(
                        "cognition.concept_ambiguous" if found else "cognition.concept_not_found",
                        "Concept selector must identify one concept",
                        details={
                            "selector": selector.value,
                            "candidates": [item.ref.identifier for item in found],
                        },
                    )
                concepts.append(found[0].ref.concept_ref)
            else:
                raise NousError(
                    "input.unsupported_selector",
                    "This selector has no registered domain",
                    details={"selector": selector.kind},
                )
        allowed: tuple[str, ...] | None = None
        unavailable = set(controls.return_types) - {
            *self.materials.searchable_kinds(),
            "cognition",
        }
        if unavailable:
            raise NousError(
                "cognition.return_type_unavailable",
                "Requested material types are not available in retrieval",
                details={"types": sorted(unavailable)},
            )
        if materials or resources or controls.return_types:
            directory = await self.wave.group_candidates(snapshot)
            keys: list[str] = []
            for group, candidates in directory.values():
                context = CognitiveGroupContext.model_validate(group.context)
                if materials and context.material not in materials:
                    continue
                if resources and not any(part.artifact in resources for part in context.sources):
                    continue
                if (
                    controls.return_types
                    and context.kind not in controls.return_types
                    and "cognition" not in controls.return_types
                ):
                    continue
                keys.extend(candidates)
            allowed = tuple(keys)
        prior = controls.filters
        if prior and prior.candidate_keys is not None:
            allowed = tuple(
                key for key in prior.candidate_keys if allowed is None or key in allowed
            )
        times = (*((prior.times) if prior else ()), *(await self.time_conditions(controls)))
        filters = (
            CandidateFilter(
                candidate_keys=allowed,
                times=times,
                include_unknown=prior.include_unknown if prior else False,
            )
            if allowed is not None or times
            else None
        )
        return filters, tuple(bound), tuple(concepts)

    async def bind_expression(
        self,
        expression: QueryExpression,
        inherited: QueryControls,
        namespace: NamespaceRef,
        snapshot: str,
        call: CallContext,
        *,
        root: bool = False,
        path: tuple[int, ...] = (),
    ) -> tuple[
        WaveExpression,
        tuple[MaterialRef, ...],
        tuple[ConceptRef, ...],
        tuple[BoundQueryBranch, ...],
    ]:
        controls = merge_controls(inherited, expression.controls)
        if isinstance(expression, QueryAtom) and expression.selectors:
            controls = controls.model_copy(
                update={
                    "selectors": tuple(dict.fromkeys((*controls.selectors, *expression.selectors)))
                }
            )
        local = expression.controls
        if isinstance(expression, QueryAtom):
            filters, materials, concepts = await self._bind(controls, namespace, snapshot, call)
            return (
                WaveExpression(
                    kind="term",
                    text=expression.text,
                    candidate_filter=filters,
                    core_concepts=tuple(item.identifier for item in concepts),
                    prefer=() if root else local.prefer,
                    avoid=() if root else local.avoid,
                    limit=None if root else local.limit,
                    sort=None if root else local.sort,
                ),
                materials,
                concepts,
                (
                    BoundQueryBranch(
                        path=path,
                        controls=controls.model_copy(
                            update={
                                "times": await self.time_conditions(controls),
                                "time_clauses": (),
                            }
                        ),
                        materials=materials,
                        concepts=concepts,
                    ),
                ),
            )
        child_controls = controls.model_copy(
            update={"limit": None, "sort": None, "prefer": (), "avoid": ()}
        )
        children = [
            await self.bind_expression(
                child, child_controls, namespace, snapshot, call, path=(*path, index)
            )
            for index, child in enumerate(expression.children)
        ]
        return (
            WaveExpression(
                kind="any" if isinstance(expression, QueryAny) else "all",
                text=expression.text,
                children=tuple(item[0] for item in children),
                prefer=() if root else local.prefer,
                avoid=() if root else local.avoid,
                limit=None if root else local.limit,
                sort=None if root else local.sort,
            ),
            tuple(dict.fromkeys(ref for item in children for ref in item[1])),
            tuple(dict.fromkeys(ref for item in children for ref in item[2])),
            tuple(branch for item in children for branch in item[3]),
        )

    @staticmethod
    def expression_text(expression: QueryExpression) -> str:
        values = [expression.text]
        if not isinstance(expression, QueryAtom):
            values.extend(QueryBinder.expression_text(child) for child in expression.children)
        return " ".join(value for value in values if value).strip()
