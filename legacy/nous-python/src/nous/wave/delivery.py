"""Translate one Native readout into ranked, source-aware Wave delivery."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal, cast

from pydantic import BaseModel, ConfigDict, Field, JsonValue, TypeAdapter, ValidationError

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, SourceSelection

from .contracts import (
    CandidateHit,
    CandidateRef,
    ChannelScore,
    DeliveredText,
    GroupDescriptor,
    GroupHit,
    PassageDelivery,
    RankEvidence,
    SearchCoverage,
    SnapshotRef,
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


class _NativeGroup(_NativeValue):
    group_key: str
    group_rank: int
    rank_basis_row: int
    passages: tuple[_NativePassage, ...]


class _NativeReadout(_NativeValue):
    """Native row identities and score fields terminate at the Wave query seam."""

    mode: str
    score_kind: str
    used_lanes: tuple[str, ...]
    skipped_lanes: tuple[dict[str, JsonValue], ...]
    hits: tuple[_NativeHit, ...]
    groups: tuple[_NativeGroup, ...]
    grouping: Literal["candidate", "document"]
    rank_scope: str
    coverage: SearchCoverage
    algorithm_versions: dict[str, str]
    thinking: dict[str, JsonValue] | None
    diagnostics: dict[str, JsonValue] | None


class WaveDelivery:
    """Interpret Native rows against the exact snapshot and deliver original text."""

    def __init__(self, artifacts: ArtifactStore, instance_id: str) -> None:
        self.artifacts = artifacts
        self.instance_id = instance_id

    async def materialize(
        self,
        request: WaveQuery,
        row: SnapshotRow,
        result: nous_native.NativeQueryResult,
        query_summary: dict[str, object],
    ) -> WaveSearchResult:
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

        async def passage(raw: _NativePassage) -> PassageDelivery:
            index = raw.row
            candidate = candidates[index]
            selection = ContentSelection.model_validate(candidate["content"])
            fields = raw.lexical_evidence.matched_fields
            body_range = raw.lexical_evidence.body_range
            reason = raw.reason
            selected: list[ContentSelection] = []
            if body_range:
                represented = ContentSelection.model_validate(candidate["retrieval_content"])
                represented = (await self.artifacts.normalize_selections((represented,)))[0]
                start = represented.selection.start_byte
                assert start is not None
                location = ContentSelection(
                    artifact=represented.artifact,
                    selection=SourceSelection(
                        start_byte=start + body_range[0], end_byte=start + body_range[1]
                    ),
                )
                try:
                    origins = await self.artifacts.map_origins(location, selection.artifact)
                except NousError as error:
                    if error.code != "content.origin_unmapped":
                        raise
                    origins = ()
                previous_precision = None
                for item in origins:
                    current = item.origin
                    if (
                        selected
                        and previous_precision == item.precision == "identity"
                        and selected[-1].artifact == current.artifact
                        and selected[-1].selection.start_byte is not None
                        and selected[-1].selection.end_byte is not None
                        and current.selection.start_byte is not None
                        and current.selection.start_byte >= selected[-1].selection.start_byte
                        and current.selection.start_byte <= selected[-1].selection.end_byte
                        and current.selection.end_byte is not None
                    ):
                        previous = selected[-1]
                        selected[-1] = ContentSelection(
                            artifact=previous.artifact,
                            selection=SourceSelection(
                                start_byte=previous.selection.start_byte,
                                end_byte=max(
                                    previous.selection.end_byte or 0, current.selection.end_byte
                                ),
                            ),
                        )
                    else:
                        selected.append(current)
                    previous_precision = item.precision
                if not selected:
                    reason = "title_preview" if "title" in fields else "preview"
            if not selected:
                whole = (await self.artifacts.normalize_selections((selection,)))[0]
                if reason == "local_score":
                    selected = [whole]
                else:
                    text = await self.artifacts.read_selection(whole)
                    start = whole.selection.start_byte
                    assert start is not None
                    selected = [
                        ContentSelection(
                            artifact=whole.artifact,
                            selection=SourceSelection(
                                start_byte=start, end_byte=start + len(text[:500].encode("utf-8"))
                            ),
                        )
                    ]
            read = await self.artifacts.read_selections(selected)
            evidence_row = raw.evidence_row
            return PassageDelivery(
                candidate=CandidateRef(snapshot=row["identifier"], key=candidate["key"]),
                fragments=tuple(
                    DeliveredText(content=value.content, text=value.text) for value in read
                ),
                reason=reason,
                matched_fields=fields,
                evidence=ranking(raw_hits[evidence_row]) if evidence_row is not None else None,
            )

        hits: list[CandidateHit] = []
        if values.grouping != "document":
            for index, hit in raw_hits.items():
                lexical = hit.lexical_evidence
                fields = lexical.matched_fields
                reason = (
                    "body_match"
                    if lexical.body_range is not None
                    else "title_preview"
                    if "title" in fields
                    else "local_score"
                    if candidates[index]["local"]
                    else "preview"
                )
                delivered = await passage(
                    _NativePassage(
                        row=index, reason=reason, lexical_evidence=lexical, evidence_row=index
                    )
                )
                hits.append(CandidateHit(basis=ranking(hit), passage=delivered))
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
        search_groups: list[GroupHit] = []
        for group in values.groups:
            descriptor = GroupDescriptor.model_validate(mapping["groups"][group.group_key])
            passages: list[PassageDelivery] = []
            seen: set[ContentSelection] = set()
            for part in group.passages:
                delivered = await passage(part)
                fragments: list[DeliveredText] = []
                for fragment in delivered.fragments:
                    if fragment.content not in seen:
                        seen.add(fragment.content)
                        fragments.append(fragment)
                if fragments:
                    passages.append(delivered.model_copy(update={"fragments": tuple(fragments)}))
            group_hit = GroupHit(
                group=descriptor,
                rank=group.group_rank,
                basis=ranking(raw_hits[group.rank_basis_row]),
                passages=tuple(passages),
            )
            search_groups.append(group_hit)
        return WaveSearchResult(
            snapshot=SnapshotRef(
                instance_id=self.instance_id, id=row["id"], identifier=row["identifier"]
            ),
            mode=values.mode,
            score_kind=values.score_kind,
            used_lanes=values.used_lanes,
            skipped_lanes=values.skipped_lanes,
            hits=tuple(hits),
            groups=tuple(search_groups),
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
            thinking=TypeAdapter(dict[str, JsonValue] | None).validate_python(thinking),
            diagnostics=values.diagnostics if request.include_trace else None,
        )
