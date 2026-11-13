"""Wave projection units preserve exact content, source support, and cleanup."""

import asyncio
import json
from pathlib import Path
from typing import cast
from unittest.mock import patch

import pytest

from nous.application.inputs import StreamInput
from nous.application.preparation import TextPreparation
from nous.cognition.retrieval.contracts import QueryAnchor, QueryControls, QueryRequest
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
                    WaveQuery(snapshot=snapshot, text="crossdocumentneedle", program="lexical"),
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
                    program="lexical",
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
                WaveQuery(snapshot=refs[1], text="Evidence", program="lexical", include_trace=True),
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
                QueryRequest(text="Reviewmarker", controls=QueryControls(include_review=True)),
            )
            assert initial.evidence.materials[0].material.ref == journal.commit.revision_ref
            assert (
                initial.evidence.materials[0].material.sources[0].owner_revision
                == source.commit.revision_ref
            )
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
                QueryRequest(text="Reviewmarker", controls=QueryControls(include_review=True)),
            )
            group = revised.evidence.materials[0]
            assert group.material.attributes.values["needs_review"] is True
            assert group.material.ref == journal.commit.revision_ref
            binding = group.material.sources[0]
            assert binding.owner_revision == source.commit.revision_ref
            assert binding.owner_revision != corrected.commit.revision_ref
            contents = await runtime.app.call(
                READ_SELECTIONS,
                ReadSelections(selections=binding.selections, representations="none"),
            )
            assert "".join(item.body_text or "" for item in contents) == "Original source text."
            without_review = await runtime.app.call(
                QUERY,
                QueryRequest(text="Reviewmarker", controls=QueryControls(include_review=False)),
            )
            assert all(
                group.material.ref != journal.commit.revision_ref
                for group in without_review.evidence.materials
            )
            historical = await runtime.app.call(
                QUERY,
                QueryRequest(
                    text="Reviewmarker",
                    anchor=QueryAnchor(snapshot=initial.evidence.snapshot, view="anchored"),
                ),
            )
            assert (
                historical.evidence.materials[0].material.attributes.values["needs_review"] is False
            )
            assert (
                historical.evidence.materials[0].material.sources[0].owner_revision
                == source.commit.revision_ref
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
            old = await runtime.app.call(QUERY, QueryRequest(text="old"))
            row = await runtime.wave.snapshots.resolve(old.evidence.snapshot)
            component = runtime.paths.resolve_owned("index", row["index_location"])
            await runtime.app.call(
                ADD_NOTE, AddNote(body=InlineText(text="new unique"), remember=True, after=policy)
            )
            current = await runtime.app.call(QUERY, QueryRequest(text="new"))
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
                QueryRequest(
                    text="new",
                    anchor=QueryAnchor(snapshot=current.evidence.snapshot, view="anchored"),
                ),
            )
            assert result.evidence.materials

    asyncio.run(scenario())
