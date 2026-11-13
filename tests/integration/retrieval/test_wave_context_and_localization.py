"""Protect parent-based context mass and snapshot-bound local passage selection."""

import json
from pathlib import Path

import numpy as np
import pytest

import nous_native as native
from nous.content.contracts import ArtifactRef, ContentSelection, SourceSelection
from nous.content.text import TextView
from nous.wave.configuration import WaveConfig
from nous.wave.contracts import (
    CandidateRef,
    DeliveryBudget,
    GroupDescriptor,
    RankEvidence,
    ScoreValue,
    SearchCoverage,
    SnapshotRef,
    WaveSearchResult,
)
from nous.wave.delivery import LocatedGroup, LocatedResult, PassageProposal, select_passages
from nous.wave.program import PortRef, ProgramNode, compile_program, native_segment, resolve_program


def test_context_mass_is_independent_of_parent_fragment_count() -> None:
    def measure(count: int, *, relevance: float = 0.8, distance: float = 0) -> float:
        rows = np.tile(np.array([[0.0, 1.0]], dtype=np.float32), (count, 1))
        parts = [
            {
                "key": f"part-{i}",
                "parent_key": "decision",
                "role": "work",
                "stage": "retrieval",
                "sequence": i,
                "source_key": "source",
                "source_start": i * 10,
                "source_end": (i + 1) * 10,
                "relevance": relevance,
                "distance": distance,
                "half_life": 1.0,
            }
            for i in range(count)
        ]
        result = native.aggregate_context(
            np.array([1.0, 0.0], dtype=np.float32),
            rows,
            json.dumps(parts),
            json.dumps({"context_mix": 0.2, "null_mass": 1.0}),
        )
        vector_selected = json.loads(result.diagnostics_json)["selected_context"]
        lexical_selected = json.loads(
            native.context_masses(
                json.dumps(parts), json.dumps({"context_mix": 0.2, "null_mass": 1.0})
            )
        )["selected_context"]
        assert lexical_selected == vector_selected
        return sum(item["contribution"] for item in vector_selected)

    assert measure(1) == pytest.approx(0.2 * 0.8 / 1.8)
    assert measure(20) == pytest.approx(measure(1))
    assert measure(1, relevance=0.0) == 0.0
    assert measure(1, relevance=1.0, distance=5) == pytest.approx(0.2 / 33)


def test_program_ignores_unconnected_model_work_and_capabilities() -> None:
    program = resolve_program(WaveConfig(), "lexical").definition
    unused_observation = ProgramNode(
        id="unused_observation",
        operator="vcp.observe",
        inputs={
            "signal": PortRef(node="query", port="contextual_signal"),
            "snapshot": PortRef(node="snapshot", port="handle"),
        },
    )
    unused_evaluation = ProgramNode(
        id="unused_evaluation",
        operator="passages.evaluate",
        inputs={
            "passages": PortRef(node="locate", port="passages"),
            "intent": PortRef(node="query", port="intent"),
        },
    )
    expanded = program.model_copy(
        update={"nodes": (*program.nodes, unused_observation, unused_evaluation)}
    )
    compiled = compile_program("lexical-with-unused-work", expanded)
    assert compiled.requirements == frozenset({"text"})
    assert {node.id for node in compiled.nodes}.isdisjoint(
        {"unused_observation", "unused_evaluation"}
    )


def test_fixed_group_localization_competes_over_all_local_units(tmp_path: Path) -> None:
    blank = np.array([], dtype=np.uint64)
    offsets = np.zeros(4, dtype=np.uint64)
    zero = np.array([0], dtype=np.uint64)
    signed = np.array([], dtype=np.int64)
    flags = np.array([], dtype=np.bool_)
    floats = np.array([], dtype=np.float64)
    candidates = native.Candidates(
        json.dumps(
            {
                "keys": ["whole", "head", "tail"],
                "documents": ["doc"] * 3,
                "titles": [""] * 3,
                "group_keys": ["doc"],
                "group_rows": [0, 0, 0],
                "local": [False, True, True],
            }
        ),
        ["whole document", "unrelated introduction", "needle relevant ending"],
        np.array([[1.0, 0.0], [0.0, 1.0], [1.0, 0.0]], dtype=np.float32),
        offsets,
        blank,
        offsets,
        zero,
        blank,
        signed,
        offsets,
        blank,
    )
    concepts = native.Concepts(json.dumps({"keys": [], "labels": [], "aliases": []}), None)
    facts = native.SourceFacts(
        json.dumps({"root_keys": [], "root_checksums": [], "sequence_keys": []}),
        blank,
        zero,
        blank,
        signed,
        flags,
    )
    associations = native.Associations(
        json.dumps({"support_groups": [], "keys": [], "relations": []}),
        blank,
        blank,
        flags,
        floats,
        floats,
        np.array([], dtype=np.int8),
        blank,
        zero,
        blank,
    )
    identity = {
        "snapshot_id": "local-test",
        "space_signature": "space",
        "build_config_signature": "config",
    }
    engine = native.NativeEngine()
    try:
        engine.build(
            candidates,
            concepts,
            facts,
            associations,
            json.dumps(identity),
            str(tmp_path / "snapshot"),
        )
        snapshot = engine.open(str(tmp_path / "snapshot"), json.dumps(identity))
        assert set(snapshot.capabilities) == {"text", "vectors"}
        fixed = native.CandidateSet(
            json.dumps(
                {
                    "identity": identity,
                    "entries": [{"row": 0, "lanes": {"provided": {"score": 1.0, "rank": 1}}}],
                }
            )
        )
        definition = resolve_program(WaveConfig(), "fixed_set").definition
        nodes = tuple(
            node.model_copy(update={"parameters": {"signals": ["lexical", "semantic"]}})
            if node.operator == "passages.locate"
            else node
            for node in definition.nodes
        )
        program = compile_program("fixed-mixed", definition.model_copy(update={"nodes": nodes}))
        _, plan = native_segment(program)
        result = snapshot.execute_program(
            name=program.name,
            program_json=json.dumps(plan),
            text="needle",
            raw_vector=np.array([1.0, 0.0], dtype=np.float32),
            candidate_set=fixed,
            options_json=json.dumps(
                {
                    "grouping": "document",
                    "top_k": 1,
                    "passages_per_group": 1,
                    "query_space_signature": "space",
                }
            ),
        )
        group = json.loads(result.result_json)["groups"][0]
        assert group["rank_basis_row"] == 0
        assert group["passages"][0]["row"] == 2
        assert group["passages"][0]["scores"]["lexical"] > 0
        assert {part["row"] for part in group["passages"]} == {1, 2}
    finally:
        engine.close()


def test_budget_preserves_ranked_groups_and_charges_shared_context_once() -> None:
    artifact = ArtifactRef(id="artifact-id", identifier="art(test)", instance_id="instance")

    def part(start: int, end: int) -> ContentSelection:
        return ContentSelection(
            artifact=artifact, selection=SourceSelection(start_byte=start, end_byte=end)
        )

    def group(key: str, rank: int, passage: ContentSelection, relevance: float) -> LocatedGroup:
        candidate = CandidateRef(snapshot="snapshot", key=key)
        return LocatedGroup(
            group=GroupDescriptor(key=key, document_key=key),
            rank=rank,
            basis=RankEvidence(
                candidate=candidate,
                candidate_rank=rank,
                ranking_score=relevance,
                quality_score=None,
                score_kind="rrf",
            ),
            passages=(
                PassageProposal(
                    candidate=candidate,
                    scored_ranges=(passage,),
                    fragments=(passage,),
                    reason="local_score",
                    support_context=(part(0, 2),),
                    scores=(
                        ScoreValue(
                            name="local_relevance",
                            value=relevance,
                            scale="relative",
                            producer="passages.locate",
                        ),
                    ),
                ),
            ),
        )

    coverage = SearchCoverage(
        requested_groups=2,
        returned_groups=2,
        eligible_groups_known=2,
        allowed_groups=2,
        recall_exhausted=True,
        truncated=False,
        reason=None,
        lanes={},
        lane_limit_unit="groups",
        group_scan_limit=10,
        passages_per_group=1,
        candidates=2,
        total_candidates=2,
        max_union_candidates=10,
        top_k=2,
        offered_candidates=2,
        ranked_candidates=2,
        unique_candidates=2,
        candidate_limit_reached=False,
        top_k_truncated=False,
    )
    template = WaveSearchResult(
        snapshot=SnapshotRef(instance_id="instance", id="id", identifier="snapshot"),
        program="test",
        score_kind="rrf",
        used_lanes=(),
        skipped_lanes=(),
        hits=(),
        groups=(),
        grouping="document",
        rank_scope="grouped_lane",
        context_summary={},
        cue_summary=(),
        algorithm_versions={},
        scope={},
        coverage=coverage,
    )
    result = LocatedResult(
        result=template,
        groups=(group("one", 1, part(2, 4), 1.0), group("two", 2, part(4, 6), 0.8)),
        hits=(),
        views={artifact.id: TextView.parse(ContentSelection(artifact=artifact), "abcdef")},
    )
    tight = select_passages(result, 1, DeliveryBudget(amount=4, unit="characters"))
    assert [item.delivery_status for item in tight.groups] == ["selected", "budget_exhausted"]
    assert tight.groups[1].passages == ()
    assert tight.result.coverage.returned_groups == 2
    model_ranked: list[LocatedGroup] = []
    for item, rank in zip(result.groups, (2.0, 1.0), strict=True):
        passage = item.passages[0]
        passage = passage.model_copy(
            update={
                "scores": (
                    *passage.scores,
                    ScoreValue(
                        name="model_rank", value=rank, scale="rank", producer="passages.evaluate"
                    ),
                )
            }
        )
        model_ranked.append(item.model_copy(update={"passages": (passage,)}))
    reranked = select_passages(
        result.model_copy(update={"groups": tuple(model_ranked)}),
        1,
        DeliveryBudget(amount=4, unit="characters"),
    )
    assert [item.delivery_status for item in reranked.groups] == ["budget_exhausted", "selected"]
    enough = select_passages(result, 1, DeliveryBudget(amount=6, unit="characters"))
    assert [item.delivery_status for item in enough.groups] == ["selected", "selected"]
    assert enough.result.context_summary["delivered_characters"] == 6
    assert enough.groups[1].passages[0].support_context == ()
    overlapping = result.model_copy(
        update={"groups": (result.groups[0], group("overlap", 2, part(1, 5), 0.8))}
    )
    deduplicated = select_passages(overlapping, 1, DeliveryBudget(amount=5, unit="characters"))
    assert [item.delivery_status for item in deduplicated.groups] == ["selected", "selected"]
    assert deduplicated.groups[1].passages[0].fragments == (part(4, 5),)
    assert deduplicated.groups[1].passages[0].scored_ranges == (part(1, 5),)
    assert deduplicated.groups[1].passages[0].shared_context == (part(1, 4), part(0, 1))
    assert deduplicated.result.context_summary["delivered_characters"] == 5
    first = group("first", 1, part(0, 3), 1.0)
    second = group("second", 2, part(3, 6), 0.8)
    first = first.model_copy(
        update={"passages": (first.passages[0].model_copy(update={"support_context": ()}),)}
    )
    second = second.model_copy(
        update={"passages": (second.passages[0].model_copy(update={"support_context": ()}),)}
    )
    unicode_result = result.model_copy(
        update={
            "groups": (first, second),
            "views": {artifact.id: TextView.parse(ContentSelection(artifact=artifact), "甲乙")},
        }
    )
    chars = select_passages(unicode_result, 1, DeliveryBudget(amount=2, unit="characters"))
    byte_limited = select_passages(unicode_result, 1, DeliveryBudget(amount=3, unit="bytes"))
    assert [group.delivery_status for group in chars.groups] == ["selected", "selected"]
    assert [group.delivery_status for group in byte_limited.groups] == [
        "selected",
        "budget_exhausted",
    ]


def test_structural_support_preserves_disjoint_source_ranges(tmp_path: Path) -> None:
    """Real queries attach table headers and code fences without selecting intervening text."""
    import asyncio

    from nous.application.inputs import StreamInput
    from nous.common.errors import NousError
    from nous.content.contracts import ExternalInput, ReadSelections
    from nous.content.operations import READ_SELECTIONS
    from nous.runtime.bootstrap import initialize_instance, open_instance
    from nous.wave.contracts import (
        CandidateLane,
        CandidateSet,
        CandidateSetEntry,
        WaveBuild,
        WaveQuery,
    )
    from nous.wave.operations import WAVE_BUILD, WAVE_QUERY

    async def scenario() -> None:
        instance = tmp_path / "structural-support"
        await initialize_instance(instance, environment={})
        fixed = resolve_program(WaveConfig(), "fixed_set").definition
        fixed = fixed.model_copy(
            update={
                "nodes": tuple(
                    node.model_copy(update={"parameters": {"signals": ["lexical"]}})
                    if node.operator == "passages.locate"
                    else node
                    for node in fixed.nodes
                )
            }
        )
        lexical = resolve_program(WaveConfig(), "lexical").definition
        duplicate_fusion = lexical.model_copy(
            update={
                "nodes": (
                    *(
                        node.model_copy(
                            update={
                                "inputs": {
                                    **node.inputs,
                                    "ranked": PortRef(node="fused", port="ranked"),
                                }
                            }
                        )
                        if node.operator == "passages.locate"
                        else node
                        for node in lexical.nodes
                    ),
                    ProgramNode(
                        id="fused",
                        operator="rank.fuse",
                        inputs={
                            "rankings": (
                                PortRef(node="rank", port="ranked"),
                                PortRef(node="rank", port="ranked"),
                            )
                        },
                        parameters={"k": 60, "weights": [2.0, 5.0]},
                    ),
                )
            }
        )
        async with open_instance(
            instance,
            environment={},
            overrides={
                "wave": {
                    "programs": {
                        "fixed_text": fixed.model_dump(mode="json"),
                        "duplicate_fusion": duplicate_fusion.model_dump(mode="json"),
                    }
                }
            },
        ) as runtime:
            source = (
                "# Reference manual\n\nThis unrelated gap must not be delivered.\n\n"
                "## Results table\n\n| Name | Value |\n| --- | --- |\n"
                "| before | 1 |\n| tablemarker | 42 |\n| after | 3 |\n\n"
                "## Executable example\n\n```python\nfirst_line = 1\n"
                "unrelated_code_gap = 2\ncodemarker = 42\nlast_line = 3\n```\n"
                "\n## Assertions\n\n"
                "firstmarker firstmarker firstmarker is not supported. Secondmarker is true.\n"
            )
            artifact = await runtime.artifacts.ingest(source, media_type="text/markdown")

            def selected(phrase: str) -> dict[str, object]:
                start = source.encode("utf-8").index(phrase.encode("utf-8"))
                return {
                    "document": "doc",
                    "selector": {
                        "start_byte": start,
                        "end_byte": start + len(phrase.encode("utf-8")),
                    },
                }

            manifest = {
                "name": "structural-support",
                "documents": [{"key": "doc", "input": {"artifact": artifact.ref.identifier}}],
                "groups": [{"key": "doc", "document_key": "doc"}],
                "candidates": [
                    {
                        "kind": "exact",
                        "key": "whole",
                        "document": "doc",
                        "group_key": "doc",
                        "content": {"document": "doc"},
                    },
                    {
                        "kind": "exact",
                        "key": "table",
                        "document": "doc",
                        "group_key": "doc",
                        "content": selected("| tablemarker | 42 |"),
                    },
                    {
                        "kind": "exact",
                        "key": "code",
                        "document": "doc",
                        "group_key": "doc",
                        "content": selected("codemarker = 42"),
                    },
                    {
                        "kind": "exact",
                        "key": "first",
                        "document": "doc",
                        "group_key": "doc",
                        "content": selected("firstmarker firstmarker firstmarker"),
                    },
                    {
                        "kind": "exact",
                        "key": "second",
                        "document": "doc",
                        "group_key": "doc",
                        "content": selected("not supported. Secondmarker is true."),
                    },
                ],
            }
            built = await runtime.app.call(
                WAVE_BUILD,
                WaveBuild(manifest=ExternalInput(key="manifest")),
                inputs={
                    "manifest": StreamInput(
                        (json.dumps(manifest).encode(),), name="collection.json"
                    )
                },
            )
            for query, required, excluded in (
                (
                    "tablemarker",
                    ("# Reference manual", "## Results table", "| Name | Value |", "tablemarker"),
                    "unrelated gap",
                ),
                (
                    "codemarker",
                    (
                        "# Reference manual",
                        "## Executable example",
                        "```python",
                        "codemarker",
                        "```",
                    ),
                    "unrelated_code_gap",
                ),
            ):
                result = await runtime.app.call(
                    WAVE_QUERY,
                    WaveQuery(
                        snapshot=built.snapshot.identifier,
                        text=query,
                        program="lexical",
                        grouping="document",
                        top_k=1,
                        passages_per_group=1,
                        delivery_budget=DeliveryBudget(amount=256, unit="bytes"),
                    ),
                )
                facts = result.program_run
                assert facts is not None and facts.selection == "explicit"
                locator = next(node for node in facts.nodes if node.operator == "passages.locate")
                assert locator.parameters["signals"] == ["lexical"]
                assert locator.parameters["rrf_k"] == 60.0
                assert facts.budget == DeliveryBudget(amount=256, unit="bytes")
                passage = result.groups[0].passages[0]
                parts = (*passage.fragments, *passage.support_context)
                assert all(
                    part.text.encode("utf-8")
                    == source.encode("utf-8")[
                        part.content.selection.start_byte : part.content.selection.end_byte
                    ]
                    for part in parts
                )
                delivered = " ".join(part.text for part in parts)
                assert all(phrase in delivered for phrase in required)
                assert excluded not in delivered
                if query == "codemarker":
                    assert any(part.text.strip() == "```" for part in passage.support_context)
                assert sum(len(part.text.encode("utf-8")) for part in parts) <= 256
            fallback = await runtime.app.call(
                WAVE_QUERY,
                WaveQuery(
                    snapshot=built.snapshot.identifier, text="tablemarker", grouping="document"
                ),
            )
            assert fallback.program_run is not None
            assert fallback.program_run.name == "text_only"
            assert fallback.program_run.selection == "fallback"
            fixed_result = await runtime.app.call(
                WAVE_QUERY,
                WaveQuery(
                    snapshot=built.snapshot.identifier,
                    text="tablemarker",
                    program="fixed_text",
                    grouping="document",
                    candidate_set=CandidateSet(
                        snapshot=built.snapshot.identifier,
                        entries=(
                            CandidateSetEntry(
                                candidate="whole",
                                lanes={"provided": CandidateLane(score=1.0, rank=1)},
                            ),
                        ),
                    ),
                ),
            )
            assert fixed_result.program_run is not None
            assert fixed_result.program_run.options["candidate_set"] == {
                "count": 1,
                "rank_scope": "candidate_lane",
            }
            assert fixed_result.groups[0].passages
            fused_result = await runtime.app.call(
                WAVE_QUERY,
                WaveQuery(
                    snapshot=built.snapshot.identifier,
                    text="tablemarker",
                    program="duplicate_fusion",
                    grouping="document",
                ),
            )
            assert fused_result.program_run is not None
            fusion = next(
                node for node in fused_result.program_run.nodes if node.operator == "rank.fuse"
            )
            assert fusion.parameters["weights"] == [2.0]
            assert fusion.inputs["rankings"] == [{"node": "rank", "port": "ranked"}]
            assert fused_result.groups[0].basis.ranking_score == pytest.approx(2.0 / 61.0)
            with pytest.raises(NousError) as missing_thinking:
                await runtime.app.call(
                    WAVE_QUERY,
                    WaveQuery(
                        snapshot=built.snapshot.identifier,
                        text="tablemarker",
                        program="lexical",
                        explore=True,
                    ),
                )
            assert missing_thinking.value.code == "wave.capability_unavailable"
            overlapping = await runtime.app.call(
                WAVE_QUERY,
                WaveQuery(
                    snapshot=built.snapshot.identifier,
                    text="firstmarker secondmarker",
                    program="lexical",
                    grouping="document",
                    top_k=1,
                    passages_per_group=2,
                    delivery_budget=DeliveryBudget(amount=256, unit="bytes"),
                ),
            )
            passages = {part.candidate.key: part for part in overlapping.groups[0].passages}
            assert set(passages) == {"first", "second"}
            second = passages["second"]
            assert second.shared_context
            assert second.scored_ranges == (
                ContentSelection(
                    artifact=artifact.ref,
                    selection=SourceSelection.model_validate(
                        selected("not supported. Secondmarker is true.")["selector"]
                    ),
                ),
            )
            referenced = (*second.scored_ranges, *second.shared_context)
            read_back = await runtime.app.call(
                READ_SELECTIONS, ReadSelections(selections=referenced, representations="none")
            )
            assert any(
                "not supported. Secondmarker is true." in (item.body_text or "")
                for item in read_back
            )
            assert any("not supported." in (item.body_text or "") for item in read_back[1:])

    asyncio.run(scenario())
