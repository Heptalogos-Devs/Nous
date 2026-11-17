"""Wave projection units preserve exact content, source support, and cleanup."""

import asyncio
import json
from pathlib import Path
from typing import cast
from unittest.mock import patch

import pytest

from nous.application.inputs import StreamInput
from nous.application.preparation import TextPreparation
from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.cognition.retrieval.operations import QUERY
from nous.cognition.writing.contracts import (
    AddNote,
    AfterWrite,
    CreateJournal,
    ReplaceRecord,
    SourceChoice,
)
from nous.cognition.writing.operations import ADD_NOTE, CREATE_JOURNAL, REPLACE_RECORD
from nous.content.contracts import ContentSelection, ExternalInput, InlineText, ReadSelections
from nous.content.operations import READ_SELECTIONS
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave.contracts import WaveBuild, WaveBuildResult, WaveQuery
from nous.wave.operations import WAVE_BUILD, WAVE_QUERY


def test_imported_cross_document_candidates_reuse_exact_content(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "cross-document"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            first = await runtime.artifacts.ingest("The first document has an orchard.")
            second = await runtime.artifacts.ingest(
                "The crossdocumentneedle comes from another source."
            )
            manifest: dict[str, object] = {
                "name": "cross-document",
                "documents": [
                    {"key": "first", "input": {"artifact": first.ref.identifier}},
                    {"key": "second", "input": {"artifact": second.ref.identifier}},
                ],
                "groups": [
                    {"key": "first", "document_key": "first", "context": {"origin": "manual"}}
                ],
                "candidates": [
                    {"kind": "document", "document": "first", "group_key": "first-layout"},
                    {
                        "kind": "exact",
                        "key": "cross-candidate",
                        "document": "first",
                        "content": {"document": "second"},
                    },
                ],
            }

            async def build() -> WaveBuildResult:
                data = json.dumps(manifest).encode()
                return await runtime.app.call(
                    WAVE_BUILD,
                    WaveBuild(manifest=ExternalInput(key="manifest")),
                    inputs={"manifest": StreamInput((data,), name="collection.json")},
                )

            first_build = await build()
            second_build = await build()
            for built in (first_build, second_build):
                snapshot = built.snapshot.identifier
                result = await runtime.app.call(
                    WAVE_QUERY,
                    WaveQuery(snapshot=snapshot, text="crossdocumentneedle", mode="lexical"),
                )
                hit = next(
                    hit
                    for hit in result.hits
                    if hit.basis.candidate.snapshot == snapshot
                    and hit.basis.candidate.key == "cross-candidate"
                )
                fragment = hit.passage.fragments[0]
                content = fragment.content
                assert content.artifact == second.ref
                read = await runtime.app.call(
                    READ_SELECTIONS, ReadSelections(selections=(content,), representations="none")
                )
                assert read[0].body_text == fragment.text
            statistics = second_build.statistics
            preparation = cast(dict[str, object], statistics["preparation"])
            assert preparation["reused_projection_units"] == 2

            original = TextPreparation.preparation_identity

            async def changed_identity(self: TextPreparation, source: ContentSelection):
                value = await original(self, source)
                if source.artifact == second.ref:
                    value["parser_profile"] = "changed-source-profile"
                return value

            with patch.object(TextPreparation, "preparation_identity", new=changed_identity):
                rebuilt = await build()
            statistics = rebuilt.statistics
            assert (
                cast(dict[str, object], statistics["preparation"])["reused_projection_units"] == 1
            )
            result = await runtime.app.call(
                WAVE_QUERY,
                WaveQuery(
                    snapshot=rebuilt.snapshot.identifier,
                    text="crossdocumentneedle",
                    mode="lexical",
                ),
            )
            assert any(
                hit.basis.candidate.snapshot == rebuilt.snapshot.identifier
                and hit.basis.candidate.key == "cross-candidate"
                for hit in result.hits
            )

    asyncio.run(scenario())


def test_support_graph_reuse_preserves_exact_source_ranges(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "graph"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            source = await runtime.artifacts.ingest("Evidence first line.\nSecond line.")
            manifest: dict[str, object] = {
                "name": "support",
                "documents": [
                    {"key": "source", "input": {"artifact": source.ref.identifier}},
                    {"key": "alias", "input": {"artifact": source.ref.identifier}},
                ],
                "concepts": [
                    {"key": "a", "label": "first", "description": "First concept"},
                    {"key": "b", "label": "second", "description": "Second concept"},
                ],
                "candidates": [
                    {
                        "kind": "exact",
                        "key": "candidate",
                        "document": "source",
                        "content": {"document": "source"},
                        "concepts": ["a"],
                    },
                    {
                        "kind": "exact",
                        "key": "counterexample",
                        "document": "alias",
                        "content": {"document": "alias"},
                        "concepts": ["b"],
                    },
                ],
                "support_groups": [
                    {
                        "key": "basis",
                        "kind": "interpretation",
                        "required_sources": ["source"],
                        "basis_refs": [
                            {"document": "source", "selector": {"start_byte": 0, "end_byte": 8}}
                        ],
                        "origin_ref": {
                            "document": "source",
                            "selector": {"start_byte": 0, "end_byte": 8},
                        },
                    }
                ],
                "associations": [
                    {
                        "key": "link",
                        "source": "a",
                        "target": "b",
                        "relation": "related",
                        "directed": True,
                        "strength": 1.0,
                        "confidence": 1.0,
                        "polarity": "negative",
                        "support_group": "basis",
                    }
                ],
            }

            async def build() -> WaveBuildResult:
                return await runtime.app.call(
                    WAVE_BUILD,
                    WaveBuild(manifest=ExternalInput(key="manifest")),
                    inputs={
                        "manifest": StreamInput(
                            (json.dumps(manifest).encode(),), name="collection.json"
                        )
                    },
                )

            first = await build()
            cast(list[dict[str, object]], manifest["support_groups"])[0]["required_sources"] = [
                "alias"
            ]
            second = await build()
            refs = [built.snapshot.identifier for built in (first, second)]
            first_row = await runtime.wave.snapshots.resolve(refs[0])
            second_row = await runtime.wave.snapshots.resolve(refs[1])
            assert first_row["graph_signature"] == second_row["graph_signature"]
            assert first_row["mapping"]["support_groups"] == second_row["mapping"]["support_groups"]
            assert first_row["mapping"]["associations"] == second_row["mapping"]["associations"]
            support = first_row["mapping"]["support_groups"][0]
            basis = ContentSelection.model_validate(cast(list[object], support["basis_refs"])[0])
            assert basis.artifact == source.ref
            assert basis.selection.start_byte == 0 and basis.selection.end_byte == 8
            assert ContentSelection.model_validate(support["origin_ref"]) == basis
            assert support["roots"] == [f"artifact:{source.ref.id}"]
            result = await runtime.app.call(
                WAVE_QUERY,
                WaveQuery(snapshot=refs[1], text="Evidence", mode="lexical", include_trace=True),
            )
            candidate = next(hit for hit in result.hits if hit.basis.candidate.key == "candidate")
            explanation = candidate.basis.details
            assert explanation["concepts"] == ["a"]
            assert explanation["counterevidence_refs"] == ["counterexample"]
            link = cast(list[dict[str, object]], explanation["counterevidence"])[0]
            assert link["relation"] == "related"
            assert link["source"] == "a" and link["target"] == "b"
            assert (
                cast(list[dict[str, object]], link["evidence"])[0]["root"]
                == f"artifact:{source.ref.id}"
            )

    asyncio.run(scenario())


def test_current_basis_correction_keeps_exact_sources_and_review_state(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "source-review"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            source = await runtime.app.call(
                ADD_NOTE,
                AddNote(body=InlineText(text="Original source text."), remember=True, after=policy),
            )
            journal = await runtime.app.call(
                CREATE_JOURNAL,
                CreateJournal(
                    body=InlineText(text="Reviewmarker is based on the original source."),
                    sources=(
                        SourceChoice(target=source.commit.revision_ref, usage="current_basis"),
                    ),
                    after=policy,
                ),
            )
            initial = await runtime.app.call(
                QUERY,
                CognitiveQuery(text="Reviewmarker", mode="lexical", include_review=True),
            )
            assert initial.groups[0].context.revision_ref == journal.commit.revision_ref
            assert initial.groups[0].context.sources[0].owner_revision == source.commit.revision_ref
            corrected = await runtime.app.call(
                REPLACE_RECORD,
                ReplaceRecord(
                    target=source.commit.ref,
                    expected=source.commit.revision_ref,
                    body=InlineText(text="Corrected source text."),
                    change_kind="correction",
                    reason="Correct the source.",
                    after=policy,
                ),
            )
            revised = await runtime.app.call(
                QUERY,
                CognitiveQuery(text="Reviewmarker", mode="lexical", include_review=True),
            )
            group = revised.groups[0]
            assert group.context.needs_review
            assert group.context.revision_ref == journal.commit.revision_ref
            binding = group.context.sources[0]
            assert binding.owner_revision == source.commit.revision_ref
            assert binding.owner_revision != corrected.commit.revision_ref
            contents = await runtime.app.call(
                READ_SELECTIONS,
                ReadSelections(selections=binding.selections, representations="none"),
            )
            assert "".join(item.body_text or "" for item in contents) == "Original source text."
            without_review = await runtime.app.call(
                QUERY,
                CognitiveQuery(text="Reviewmarker", mode="lexical", include_review=False),
            )
            assert all(
                group.context.revision_ref != journal.commit.revision_ref
                for group in without_review.groups
            )
            historical = await runtime.app.call(
                QUERY,
                CognitiveQuery(
                    text="Reviewmarker", mode="lexical", snapshot=initial.context.snapshot_ref
                ),
            )
            assert not historical.groups[0].context.needs_review
            assert (
                historical.groups[0].context.sources[0].owner_revision == source.commit.revision_ref
            )

    asyncio.run(scenario())


def test_cleanup_retries_unreferenced_component_after_delete_failure(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "cleanup"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            policy = AfterWrite(annotation="skip")
            await runtime.app.call(
                ADD_NOTE, AddNote(body=InlineText(text="old unique"), remember=True, after=policy)
            )
            old = await runtime.app.call(QUERY, CognitiveQuery(text="old", mode="lexical"))
            row = await runtime.wave.snapshots.resolve(old.context.snapshot_ref)
            component = runtime.paths.resolve_owned("index", row["index_location"])
            await runtime.app.call(
                ADD_NOTE, AddNote(body=InlineText(text="new unique"), remember=True, after=policy)
            )
            current = await runtime.app.call(QUERY, CognitiveQuery(text="new", mode="lexical"))
            remove = runtime.wave.snapshots._remove_owned  # pyright: ignore[reportPrivateUsage]

            def interrupted(path: Path, root: Path, name: str) -> None:
                if path == component:
                    raise OSError("component deletion interrupted")
                remove(path, root, name)

            with patch.object(runtime.wave.snapshots, "_remove_owned", new=interrupted):
                with pytest.raises(OSError, match="component deletion interrupted"):
                    await runtime.wave.cleanup()
            assert component.exists()
            await runtime.wave.cleanup()
            assert not component.exists()
            result = await runtime.app.call(
                QUERY,
                CognitiveQuery(text="new", snapshot=current.context.snapshot_ref, mode="lexical"),
            )
            assert result.groups

    asyncio.run(scenario())


def test_grouped_native_readout_prefers_already_ranked_passages(tmp_path: Path) -> None:
    import numpy as np

    import nous_native as native

    count = 3
    empty = np.array([], dtype=np.uint64)
    offsets = np.zeros(count + 1, dtype=np.uint64)
    zero = np.array([0], dtype=np.uint64)
    positions = np.array([], dtype=np.int64)
    flags = np.array([], dtype=np.bool_)
    floats = np.array([], dtype=np.float64)
    metadata = {
        "keys": ["whole", "less-relevant", "ranked-local"],
        "documents": ["document"] * count,
        "titles": [""] * count,
        "group_keys": ["document"],
        "group_rows": [0, 0, 0],
        "local": [False, True, True],
    }
    candidates = native.Candidates(
        json.dumps(metadata),
        ["whole text", "first local text", "stronger local text"],
        np.array([[1.0, 0.0], [0.7, 0.7], [0.99, 0.1]], dtype=np.float32),
        offsets,
        empty,
        offsets,
        zero,
        empty,
        positions,
        offsets,
        empty,
    )
    concepts = native.Concepts(json.dumps({"keys": [], "labels": [], "aliases": []}), None)
    facts = native.SourceFacts(
        json.dumps({"root_keys": [], "root_checksums": [], "sequence_keys": []}),
        empty,
        zero,
        empty,
        positions,
        flags,
    )
    associations = native.Associations(
        json.dumps({"support_groups": [], "keys": [], "relations": []}),
        empty,
        empty,
        flags,
        floats,
        floats,
        np.array([], dtype=np.int8),
        empty,
        zero,
        empty,
    )
    identity = json.dumps(
        {"snapshot_id": "grouped", "space_signature": "space", "build_config_signature": "config"}
    )
    options = json.dumps(
        {
            "grouping": "document",
            "top_k": 1,
            "passages_per_group": 2,
            "query_space_signature": "space",
        }
    )
    engine = native.NativeEngine()
    try:
        engine.build(candidates, concepts, facts, associations, identity, str(tmp_path / "index"))
        snapshot = engine.open(str(tmp_path / "index"), identity)
        query = np.array([1.0, 0.0], dtype=np.float32)
        result = json.loads(
            snapshot.search(mode="hybrid", text="", vector=query, options_json=options).result_json
        )
        group = result["groups"][0]
        assert group["rank_basis_row"] == 0
        assert group["passages"][0]["row"] == 2
        assert group["passages"][0]["reason"] == "local_score"
        assert group["passages"][0]["evidence_row"] == 2
        assert all(passage["row"] != 0 for passage in group["passages"])

        filtered = json.loads(
            snapshot.search(
                mode="hybrid",
                text="",
                vector=query,
                allowed_rows=np.array([0, 1], dtype=np.uint64),
                options_json=options,
            ).result_json
        )
        assert filtered["groups"][0]["passages"][0]["row"] == 1
    finally:
        engine.close()
