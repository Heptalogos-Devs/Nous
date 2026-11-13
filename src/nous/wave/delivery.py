"""Translate one Native readout into ranked, source-aware Wave delivery."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal, cast

from pydantic import BaseModel, ConfigDict, Field, JsonValue, TypeAdapter, ValidationError

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, SourceSelection
from nous.content.selections import subtract_selection
from nous.content.text import TextView

from .contracts import (
    CandidateHit,
    CandidateRef,
    ChannelScore,
    DeliveredText,
    DeliveryBudget,
    DeliveryStatus,
    GroupDescriptor,
    GroupHit,
    PassageDelivery,
    ProgramRunFacts,
    RankEvidence,
    ScoreValue,
    SearchCoverage,
    SnapshotRef,
    WaveBranchCoverage,
    WaveQuery,
    WaveSearchResult,
)
from .mapping import SnapshotRow

if TYPE_CHECKING:
    import nous_native


class _NativeValue(BaseModel):
    model_config = ConfigDict(extra="ignore", frozen=True)


class _LexicalEvidence(_NativeValue):
    matched_fields: tuple[Literal["body", "title"], ...] = ()
    body_range: tuple[int, int] | None = None


class _NativeLink(_NativeValue):
    row: int
    associations: tuple[int, ...]


class _FoldedRow(_NativeValue):
    row: int
    similarity: float


class _NativeHit(_NativeValue):
    row: int
    rank: int
    candidate_rank: int | None = None
    ranking_score: float
    quality_score: float | None = None
    score_kind: str
    channels: tuple[str, ...] = ()
    channel_ranks: dict[str, int] | None = None
    channel_scores: dict[str, float] | None = None
    concept_rows: tuple[int, ...] = ()
    chosen_path: int | None = None
    paths: tuple[dict[str, JsonValue], ...] = ()
    folded_rows: tuple[_FoldedRow, ...] = ()
    counterevidence_links: tuple[_NativeLink, ...] = ()
    independent_support: tuple[dict[str, JsonValue], ...] = ()
    trace: JsonValue = None
    lexical_evidence: _LexicalEvidence = Field(default_factory=_LexicalEvidence)


class _NativePassage(_NativeValue):
    row: int
    reason: Literal["body_match", "local_score", "title_preview", "preview"]
    lexical_evidence: _LexicalEvidence = Field(default_factory=_LexicalEvidence)
    evidence_row: int | None = None
    scores: dict[str, float | None] = Field(default_factory=dict)


class _NativeGroup(_NativeValue):
    group_key: str
    group_rank: int
    rank_basis_row: int
    passages: tuple[_NativePassage, ...]


class _NativeReadout(_NativeValue):
    """Native row identities and score fields terminate at the Wave query seam."""

    program: str
    score_kind: str
    used_lanes: tuple[str, ...]
    skipped_lanes: tuple[dict[str, JsonValue], ...]
    hits: tuple[_NativeHit, ...]
    groups: tuple[_NativeGroup, ...]
    grouping: Literal["candidate", "document"]
    rank_scope: str
    coverage: SearchCoverage
    branch_coverage: tuple[WaveBranchCoverage, ...] = ()
    algorithm_versions: dict[str, str]
    thinking: dict[str, JsonValue] | None
    diagnostics: dict[str, JsonValue] | None


class PassageProposal(_NativeValue):
    candidate: CandidateRef
    scored_ranges: tuple[ContentSelection, ...]
    fragments: tuple[ContentSelection, ...]
    support_context: tuple[ContentSelection, ...] = ()
    shared_context: tuple[ContentSelection, ...] = ()
    reason: Literal["body_match", "local_score", "title_preview", "preview"]
    matched_fields: tuple[Literal["body", "title"], ...] = ()
    evidence: RankEvidence | None = None
    scores: tuple[ScoreValue, ...] = ()
    assessment: dict[str, JsonValue] | None = None


class LocatedGroup(_NativeValue):
    group: GroupDescriptor
    rank: int
    basis: RankEvidence
    passages: tuple[PassageProposal, ...]
    delivery_status: DeliveryStatus = "not_selected"


class LocatedHit(_NativeValue):
    basis: RankEvidence
    passage: PassageProposal


class LocatedResult(_NativeValue):
    result: WaveSearchResult
    groups: tuple[LocatedGroup, ...]
    hits: tuple[LocatedHit, ...]
    views: dict[str, TextView]


class WaveDelivery:
    """Interpret Native rows against the exact snapshot and deliver original text."""

    def __init__(self, artifacts: ArtifactStore, instance_id: str) -> None:
        self.artifacts = artifacts
        self.instance_id = instance_id

    async def locate(
        self,
        request: WaveQuery,
        row: SnapshotRow,
        result: nous_native.NativeQueryResult,
        query_summary: dict[str, object],
        program_run: ProgramRunFacts | None = None,
    ) -> LocatedResult:
        mapping = row["mapping"]
        candidates = mapping["candidates"]
        concepts = mapping["concepts"]
        try:
            values = _NativeReadout.model_validate_json(result.result_json)
        except ValidationError as error:
            raise NousError(
                "wave.invalid_native_result", "Native returned an invalid query result", cause=error
            ) from error
        raw_hits = {hit.row: hit for hit in values.hits}

        def ranking(hit: _NativeHit) -> RankEvidence:
            index = hit.row
            candidate = candidates[index]
            ranks = hit.channel_ranks or {}
            scores = hit.channel_scores or {}
            names = dict.fromkeys((*hit.channels, *ranks, *scores))
            chosen = hit.chosen_path
            paths = hit.paths
            links = hit.counterevidence_links
            support_groups = {group["key"]: group for group in mapping["support_groups"]}
            counterevidence: list[dict[str, object]] = []
            for link in links:
                related = candidates[link.row]
                for association_index in link.associations:
                    association = mapping["associations"][association_index]
                    counterevidence.append(
                        {
                            "candidate": related["key"],
                            "content": related["content"],
                            "relation": association["relation"],
                            "source": association["source"],
                            "target": association["target"],
                            "support_group": support_groups[association["support_group"]],
                            "evidence": [
                                {"root": root, "artifacts": mapping["roots"][root]}
                                for root in association["roots"]
                            ],
                        }
                    )
            explanation: dict[str, object] = {
                "concepts": [concepts[i]["key"] for i in hit.concept_rows],
                "sources": candidate["roots"],
                "chosen_path": candidate["paths"][chosen] if chosen is not None else None,
                "paths": [
                    {**path, "path": candidate["paths"][cast(int, path["path"])]}
                    for path in paths
                    if path.get("path") is not None
                ],
                "folded_refs": [
                    {
                        "candidate": candidates[item.row]["key"],
                        "similarity": item.similarity,
                    }
                    for item in hit.folded_rows
                ],
                "counterevidence_refs": [candidates[link.row]["key"] for link in links],
                "counterevidence": counterevidence,
            }
            if request.include_trace:
                explanation["trace"] = hit.trace
                explanation["independent_support"] = [
                    {
                        **item,
                        "source": concepts[cast(int, item["source"])]["key"],
                        "target": concepts[cast(int, item["target"])]["key"],
                    }
                    for item in hit.independent_support
                ]
            return RankEvidence(
                candidate=CandidateRef(snapshot=row["identifier"], key=candidate["key"]),
                candidate_rank=hit.candidate_rank or hit.rank,
                ranking_score=hit.ranking_score,
                quality_score=hit.quality_score,
                score_kind=hit.score_kind,
                channels=tuple(
                    ChannelScore(channel=name, rank=ranks.get(name), score=scores.get(name))
                    for name in names
                ),
                details=TypeAdapter(dict[str, JsonValue]).validate_python(explanation),
            )

        if values.grouping == "document":
            raw_parts = [part for group in values.groups for part in group.passages]
        else:
            raw_parts = [
                _NativePassage(
                    row=index,
                    reason="local_score",
                    lexical_evidence=hit.lexical_evidence,
                    evidence_row=index,
                )
                for index, hit in raw_hits.items()
            ]
        selections = tuple(
            ContentSelection.model_validate(candidates[part.row]["content"]) for part in raw_parts
        )
        normalized = await self.artifacts.normalize_selections(selections)
        artifacts = {content.artifact.id: content.artifact for content in normalized}
        views = await self.artifacts.text_views(
            tuple(ContentSelection(artifact=ref) for ref in artifacts.values())
        )
        text_views = dict(zip(artifacts, views, strict=True))

        def closure_ranges() -> list[tuple[ContentSelection, ...]]:
            ranges: list[tuple[ContentSelection, ...]] = []
            for content in normalized:
                closure = text_views[content.artifact.id].sentence_closure(content)
                start, end = content.selection.start_byte, content.selection.end_byte
                closed_start, closed_end = closure.selection.start_byte, closure.selection.end_byte
                if start is None or end is None or closed_start is None or closed_end is None:
                    raise NousError(
                        "wave.invalid_native_result", "Passage text lacks exact byte coordinates"
                    )
                context: list[ContentSelection] = []
                if closed_start < start:
                    context.append(
                        ContentSelection(
                            artifact=content.artifact,
                            selection=SourceSelection(start_byte=closed_start, end_byte=start),
                        )
                    )
                if end < closed_end:
                    context.append(
                        ContentSelection(
                            artifact=content.artifact,
                            selection=SourceSelection(start_byte=end, end_byte=closed_end),
                        )
                    )
                covered = [content, *context]
                for structural in text_views[content.artifact.id].structural_context(content):
                    remaining = subtract_selection(structural, covered)
                    context.extend(remaining)
                    covered.extend(remaining)
                ranges.append(tuple(context))
            return ranges

        contexts = await self.artifacts.content.execution.blocking(closure_ranges)
        delivered: list[PassageProposal] = []
        for raw, content, ranges in zip(raw_parts, normalized, contexts, strict=True):
            candidate = candidates[raw.row]
            delivered.append(
                PassageProposal(
                    candidate=CandidateRef(snapshot=row["identifier"], key=candidate["key"]),
                    scored_ranges=(content,),
                    fragments=(content,),
                    support_context=ranges,
                    reason=raw.reason,
                    matched_fields=raw.lexical_evidence.matched_fields,
                    evidence=ranking(raw_hits[raw.evidence_row])
                    if raw.evidence_row is not None
                    else None,
                    scores=tuple(
                        ScoreValue(
                            name=name,
                            value=score,
                            scale=(
                                "reciprocal_rank"
                                if name in {"local_rrf", "rrf_reference", "context_lexical"}
                                else "rrf_theoretical_reference"
                                if name == "local_relevance"
                                else "rank"
                                if name in {"lexical_rank", "semantic_rank"}
                                else "weight"
                                if name in {"lexical_weight", "semantic_weight"}
                                else name
                            ),
                            producer="passages.locate",
                            based_on=(
                                tuple(
                                    signal
                                    for signal in (
                                        "lexical_rank",
                                        "lexical_weight",
                                        "semantic_rank",
                                        "semantic_weight",
                                        "context_lexical",
                                    )
                                    if raw.scores.get(signal) is not None
                                )
                                if name == "local_rrf"
                                else ("local_rrf", "rrf_reference")
                                if name == "local_relevance"
                                else ()
                            ),
                        )
                        for name, score in raw.scores.items()
                        if score is not None
                    ),
                )
            )
        deliveries = iter(delivered)
        hits: list[LocatedHit] = []
        if values.grouping != "document":
            hits = [
                LocatedHit(basis=ranking(hit), passage=next(deliveries))
                for hit in raw_hits.values()
            ]
        thinking = values.thinking
        if isinstance(thinking, dict):
            thinking = cast(dict[str, object], thinking)
            for stage_value in cast(list[dict[str, object]], thinking.get("stages", [])):
                for item in cast(list[dict[str, object]], stage_value.get("results", [])):
                    candidate = candidates[cast(int, item.pop("row"))]
                    item["candidate"] = candidate["key"]
                    chosen = item.get("chosen_path")
                    item["chosen_path"] = (
                        candidate["paths"][cast(int, chosen)] if chosen is not None else None
                    )
                    item["parents"] = [
                        candidates[index]["key"]
                        for index in cast(list[int], item.get("parents", []))
                    ]
                    route = item.get("route")
                    if isinstance(route, dict):
                        route = cast(dict[str, object], route)
                        for field, output_field in (
                            ("sourceId", "source"),
                            ("targetId", "target"),
                            ("previousId", "previous"),
                        ):
                            concept_id = route.pop(field, None)
                            if concept_id is None:
                                route[output_field] = None
                                continue
                            if (
                                not isinstance(concept_id, int)
                                or isinstance(concept_id, bool)
                                or not 1 <= concept_id <= len(concepts)
                            ):
                                raise NousError(
                                    "wave.invalid_native_result",
                                    "Thinking route references an unknown concept",
                                )
                            route[output_field] = concepts[concept_id - 1]["key"]
        search_groups: list[LocatedGroup] = []
        for group in values.groups:
            descriptor = GroupDescriptor.model_validate(mapping["groups"][group.group_key])
            passages: list[PassageProposal] = []
            seen: set[ContentSelection] = set()
            for _part in group.passages:
                part_delivery = next(deliveries)
                fragments: list[ContentSelection] = []
                for fragment in part_delivery.fragments:
                    if fragment not in seen:
                        seen.add(fragment)
                        fragments.append(fragment)
                if fragments:
                    passages.append(
                        part_delivery.model_copy(update={"fragments": tuple(fragments)})
                    )
            group_hit = LocatedGroup(
                group=descriptor,
                rank=group.group_rank,
                basis=ranking(raw_hits[group.rank_basis_row]),
                passages=tuple(passages),
            )
            search_groups.append(group_hit)
        template = WaveSearchResult(
            snapshot=SnapshotRef(
                instance_id=self.instance_id, id=row["id"], identifier=row["identifier"]
            ),
            program=values.program,
            program_run=program_run,
            score_kind=values.score_kind,
            used_lanes=values.used_lanes,
            skipped_lanes=values.skipped_lanes,
            hits=(),
            groups=(),
            grouping=values.grouping,
            rank_scope=values.rank_scope,
            context_summary=TypeAdapter(dict[str, JsonValue]).validate_python(query_summary),
            cue_summary=tuple({"label": cue.label, "is_core": cue.is_core} for cue in request.cues),
            algorithm_versions=values.algorithm_versions,
            scope={
                "source_roots": len(mapping["roots"]),
                "candidate_filter": request.candidate_filter is not None,
            },
            coverage=values.coverage,
            branch_coverage=values.branch_coverage,
            thinking=TypeAdapter(dict[str, JsonValue] | None).validate_python(thinking),
            diagnostics=values.diagnostics if request.include_trace else None,
        )
        return LocatedResult(
            result=template, groups=tuple(search_groups), hits=tuple(hits), views=text_views
        )

    async def read(self, result: LocatedResult) -> WaveSearchResult:
        """Read only the ranges selected for final evidence delivery."""
        proposals = (
            [part for group in result.groups for part in group.passages]
            if result.groups
            else [hit.passage for hit in result.hits]
        )
        refs = tuple(ref for part in proposals for ref in (*part.fragments, *part.support_context))
        values = iter(await self.artifacts.read_selections(refs) if refs else ())

        def deliver(part: PassageProposal) -> PassageDelivery:
            fragments = tuple(
                DeliveredText(content=item.content, text=item.text)
                for item in (next(values) for _ in part.fragments)
            )
            support = tuple(
                DeliveredText(content=item.content, text=item.text)
                for item in (next(values) for _ in part.support_context)
            )
            return PassageDelivery(
                candidate=part.candidate,
                scored_ranges=part.scored_ranges,
                fragments=fragments,
                support_context=support,
                shared_context=part.shared_context,
                reason=part.reason,
                matched_fields=part.matched_fields,
                evidence=part.evidence,
                scores=part.scores,
                assessment=part.assessment,
            )

        groups = tuple(
            GroupHit(
                group=group.group,
                rank=group.rank,
                basis=group.basis,
                passages=tuple(deliver(part) for part in group.passages),
                delivery_status=group.delivery_status,
            )
            for group in result.groups
        )
        hits = tuple(
            CandidateHit(basis=hit.basis, passage=deliver(hit.passage)) for hit in result.hits
        )
        return result.result.model_copy(update={"groups": groups, "hits": hits})


def select_passages(
    result: LocatedResult,
    passages_per_group: int,
    budget: DeliveryBudget | None,
) -> LocatedResult:
    """Choose source ranges and count their union without reading delivery text."""
    if budget is not None and budget.unit == "tokens":
        raise NousError("wave.capability_unavailable", "A tokenizer is required for token budgets")
    unit = budget.unit if budget is not None else "characters"
    maximum = budget.amount if budget is not None else None
    groups, hits = list(result.groups), list(result.hits)
    available = [
        (group_index, passage_index, part)
        for group_index, group in enumerate(groups)
        for passage_index, part in enumerate(group.passages)
    ]
    if not available:
        available = [(index, 0, hit.passage) for index, hit in enumerate(hits)]
    selected: dict[tuple[int, int], PassageProposal] = {}
    covered: list[ContentSelection] = []
    lengths: dict[ContentSelection, int] = {}
    counts: dict[int, int] = {}
    used = 0

    def incremental(part: PassageProposal) -> PassageProposal | None:
        temporary = list(covered)
        fragments: list[ContentSelection] = []
        for content in part.fragments:
            remaining = subtract_selection(content, temporary)
            fragments.extend(remaining)
            temporary.extend(remaining)
        if not fragments:
            return None
        support: list[ContentSelection] = []
        for content in part.support_context:
            remaining = subtract_selection(content, temporary)
            support.extend(remaining)
            temporary.extend(remaining)
        return part.model_copy(
            update={
                "fragments": tuple(fragments),
                "support_context": tuple(support),
            }
        )

    def length(part: PassageProposal) -> int:
        total = 0
        for content in (*part.fragments, *part.support_context):
            if content not in lengths:
                view = result.views[content.artifact.id]
                start, end = view.selection_bounds(content)
                data = view.raw[start - view.byte_offset : end - view.byte_offset]
                lengths[content] = len(data) if unit == "bytes" else len(data.decode("utf-8"))
            total += lengths[content]
        return total

    def priority(part: PassageProposal) -> tuple[int, float, float] | None:
        model_rank = next(
            (value.value for value in part.scores if value.name == "model_rank"), None
        )
        relevance = next(
            (value.value for value in part.scores if value.name == "local_relevance"), 1.0
        )
        if model_rank is None and relevance <= 0.0:
            return None
        return int(model_rank is None), model_rank or 0.0, -relevance

    ranked = sorted(
        (
            (score, group_index, passage_index, part)
            for group_index, passage_index, part in available
            if (score := priority(part)) is not None
        ),
        key=lambda value: value[:3],
    )
    while ranked:
        chosen: tuple[int, int, int, int, PassageProposal, PassageProposal] | None = None
        for index, (_, group_index, passage_index, part) in enumerate(ranked):
            if counts.get(group_index, 0) >= passages_per_group:
                continue
            item = incremental(part)
            if item is None:
                continue
            cost = length(item)
            if maximum is not None and used + cost > maximum:
                continue
            chosen = index, group_index, passage_index, cost, item, part
            break
        if chosen is None:
            break
        index, group_index, passage_index, cost, item, original = chosen
        shared: list[ContentSelection] = []
        for content in (*original.fragments, *original.support_context):
            shared.extend(
                subtract_selection(content, (*item.fragments, *item.support_context, *shared))
            )
        item = item.model_copy(update={"shared_context": tuple(shared)})
        selected[(group_index, passage_index)] = item
        used += cost
        counts[group_index] = counts.get(group_index, 0) + 1
        covered.extend((*item.fragments, *item.support_context))
        ranked.pop(index)
    if groups:
        delivered_groups: list[LocatedGroup] = []
        for group_index, group in enumerate(groups):
            passages = tuple(
                selected[(group_index, index)]
                for index in range(len(group.passages))
                if (group_index, index) in selected
            )
            if passages:
                status = "selected"
            elif not group.passages:
                status = "no_candidates"
            else:
                eligible = [part for part in group.passages if priority(part) is not None]
                remaining = [item for part in eligible if (item := incremental(part)) is not None]
                if not eligible:
                    status = "not_selected"
                elif not remaining:
                    status = "overlap"
                elif maximum is not None and all(
                    used + length(item) > maximum for item in remaining
                ):
                    status = "budget_exhausted"
                else:
                    status = "not_selected"
            delivered_groups.append(
                group.model_copy(update={"passages": passages, "delivery_status": status})
            )
        groups = delivered_groups
    else:
        hits = [
            hit.model_copy(update={"passage": selected[(index, 0)]})
            for index, hit in enumerate(hits)
            if (index, 0) in selected
        ]
    summary = dict(result.result.context_summary)
    summary[f"delivered_{unit}"] = used
    return result.model_copy(
        update={
            "groups": tuple(groups),
            "hits": tuple(hits),
            "result": result.result.model_copy(update={"context_summary": summary}),
        }
    )
