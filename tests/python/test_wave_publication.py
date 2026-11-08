"""Published views preserve exact inputs, references and active readers."""

from __future__ import annotations

import asyncio
import json
import mmap
import os
import threading
from collections.abc import Callable
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, cast

import pytest
import sqlalchemy as sa

from nous.application.configuration import open_configuration_operations
from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.common.errors import NousError
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema
from nous.wave.contracts import WaveQuery

if TYPE_CHECKING:
    import nous_native


def test_cognitive_passages_reuse_and_safe_cleanup(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "wave-publication"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            body = (
                "Ordinary context for a lengthy note.\n\n" * 220
            ) + "The closing answer is embervault."
            record = (
                await cognition.records.add_note(
                    body, title="Unmistakable quasarheading", remember=True
                )
            ).record
            title_result = await cognition.retrieval.query(
                CognitiveQuery(text="quasarheading", mode="lexical")
            )
            title_passage = title_result.groups[0].passages[0]
            assert title_result.groups[0].revision_ref == record.revision_ref
            assert title_passage.matched_fields == ("title",)
            assert title_passage.passage_reason == "title_preview"
            assert (
                await runtime.artifacts.read_selection(title_passage.selection.selections[0])
                == title_passage.snippet
            )

            tail_result = await cognition.retrieval.query(
                CognitiveQuery(text="embervault", mode="lexical")
            )
            passage = tail_result.groups[0].passages[0]
            assert "embervault" in passage.snippet
            assert passage.passage_reason == "body_match"
            assert (passage.selection.selections[0].selection.start_byte or 0) > 500
            assert (
                await runtime.artifacts.read_selection(passage.selection.selections[0])
                == passage.snippet
            )
            old_ref = tail_result.context.snapshot_ref
            await cognition.feedback.record_query(tail_result, "retrieval")

            await cognition.records.add_note(
                "A second note shares the same collection.", remember=True
            )
            second = await cognition.retrieval.query(
                CognitiveQuery(text="collection", mode="lexical")
            )
            second_ref = second.context.snapshot_ref
            async with runtime.database.read() as tx:
                summaries = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.snapshots.c.id, schema.snapshots.c.build_summary)
                        )
                    )
                    .mappings()
                    .all()
                )
            latest = next(row for row in summaries if row["id"] == second_ref.id)
            stats = latest["build_summary"]["statistics"]["preparation"]
            assert stats["reused_projection_units"] >= 1
            assert stats["parsed"] <= 1

            await cognition.records.add_note("A third note publishes the next view.", remember=True)
            current = await cognition.retrieval.query(CognitiveQuery(text="third", mode="lexical"))
            second_row = await runtime.wave.snapshots.resolve(second_ref)
            second_component = runtime.wave.snapshots.paths.resolve_owned(
                "index", second_row["index_location"]
            )
            async with runtime.wave.snapshots.borrow(second_ref):
                await runtime.wave.cleanup()
                assert (await runtime.wave.describe(second_ref)).ref == second_ref
            removed = await runtime.wave.cleanup()
            assert second_ref.identifier in cast(list[str], removed["snapshots"])
            assert not second_component.exists()
            orphan = runtime.wave.snapshots.paths.resolve_owned(
                "index", f"wave/components/{'f' * 64}"
            )
            orphan.mkdir()
            (orphan / "assets.msgpack").write_bytes(b"unfinished")
            cleaned = await runtime.wave.cleanup()
            assert orphan.name in cast(list[str], cleaned["staging"])
            assert not orphan.exists()
            assert (await runtime.wave.describe(old_ref)).ref == old_ref
            assert (
                await runtime.wave.describe(current.context.snapshot_ref)
            ).ref == current.context.snapshot_ref
            old_result = await cognition.retrieval.query(
                CognitiveQuery(text="embervault", mode="lexical", snapshot=old_ref.identifier)
            )
            assert "embervault" in old_result.groups[0].passages[0].snippet
            assert (await cognition.records.read(record.revision_ref)).body == body

    asyncio.run(scenario())


def test_prepared_recovery_conflict_and_native_component_reuse(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "wave-recovery"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            manifest = Path("docs/examples/wave/collection.json")
            async with runtime.execution.operation("test.wave"):
                view = await runtime.wave.snapshots.import_manifest(manifest)
                profile = runtime.wave.resolve_build_profile(None)
                prepared, concurrent = await asyncio.gather(
                    runtime.wave.prepare_build(view, profile, build_key="recover-example"),
                    runtime.wave.prepare_build(view, profile, build_key="recover-example"),
                )
                assert prepared == concurrent
                with pytest.raises(RuntimeError):
                    async with runtime.database.write() as tx:
                        await runtime.wave.accept_in(tx, prepared)
                        raise RuntimeError("abort adoption")
                recovered = await runtime.wave.prepare_build(
                    view, profile, build_key="recover-example"
                )
                assert recovered == prepared
                component = runtime.wave.snapshots.paths.resolve_owned(
                    "index", prepared.index_location
                )
                marker = component / "manifest.json"
                hidden_marker = component / "manifest.saved"
                marker.rename(hidden_marker)
                try:
                    with pytest.raises(NousError) as protected:
                        await runtime.wave.prepare_build(view, profile, build_key="another-key")
                    assert protected.value.code == "wave.snapshot_invalid"
                    assert component.is_dir() and hidden_marker.is_file()
                finally:
                    hidden_marker.rename(marker)
                meta = runtime.wave.snapshots.paths.resolve_owned(
                    "index", f"{prepared.index_location}/lexical/meta.json"
                )
                original = meta.read_bytes()
                try:
                    meta.write_bytes(original + b" ")
                    with pytest.raises(NousError) as invalid:
                        await runtime.wave.prepare_build(view, profile, build_key="recover-example")
                    assert invalid.value.code == "wave.snapshot_invalid"
                finally:
                    meta.write_bytes(original)
                with pytest.raises(NousError) as error:
                    await runtime.wave.prepare_build(
                        view.model_copy(update={"metadata": {"changed": True}}),
                        profile,
                        build_key="recover-example",
                    )
                assert error.value.code == "wave.build_key_conflict"
                async with runtime.database.write() as tx:
                    first = await runtime.wave.accept_in(tx, recovered)
                    await runtime.wave.promote_in(tx, first.ref)
                runtime.wave.release_prepared(prepared)
                runtime.wave.release_prepared(concurrent)
                runtime.wave.release_prepared(recovered)
                changed = view.model_copy(update={"metadata": {"changed": True}})
                other = await runtime.wave.prepare_build(changed, profile)
                assert other.component_signature == recovered.component_signature
                assert other.index_location == recovered.index_location
                assert (
                    cast(dict[str, object], other.summary["statistics"])["reused_native_component"]
                    is True
                )
                async with runtime.database.write() as tx:
                    second = await runtime.wave.accept_in(tx, other)
                    await runtime.wave.promote_in(tx, second.ref)
                runtime.wave.release_prepared(other)
                await runtime.wave.cleanup()
                inspection = await runtime.wave.inspect(second.ref.identifier)
                assert cast(dict[str, str], inspection["snapshot"])["id"] == second.ref.id
                result = await runtime.wave.query(
                    WaveQuery(snapshot=second.ref.identifier, text="资源", mode="lexical")
                )
                assert result["hits"]

    asyncio.run(scenario())


def test_semantic_whole_hit_delivers_local_candidate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            inputs = request["input"]
            if isinstance(inputs, str):
                inputs = [inputs]
            data = [
                {
                    "object": "embedding",
                    "index": index,
                    "embedding": [1.0, 0.0]
                    if len(text) > 5000 or text == "semantic question"
                    else [0.8, 0.6],
                }
                for index, text in enumerate(inputs)
            ]
            body = json.dumps({"object": "list", "model": "local", "data": data}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:

        async def scenario() -> None:
            instance = tmp_path / "semantic-local"
            await initialize_instance(instance, environment={})
            async with open_configuration_operations(instance, environment={}) as config:
                await config.update(
                    {
                        "providers.local": {
                            "protocol": "openai-chat-completions",
                            "endpoint": f"http://127.0.0.1:{server.server_port}/v1",
                        },
                        "models.vectors": {
                            "provider": "local",
                            "model_name": "local",
                            "capabilities": ["embedding"],
                        },
                        "embedding.spaces.test-space": {
                            "model": "vectors",
                            "deployment_revision": "test",
                            "dimensions": 2,
                            "modalities": ["text"],
                            "normalization": "none",
                            "encoding": {
                                "document_instruction": "",
                                "query_instruction": "",
                                "preprocessing_revision": "test",
                                "input_combination": "text",
                            },
                        },
                        "cognition.search.space": "test-space",
                    }
                )
            async with open_instance(instance, environment={}) as runtime:
                cognition = await runtime.cognition()
                body = "A short paragraph about distant mountains.\n\n" * 200
                note = await cognition.records.add_note(body, remember=True)
                held: list[mmap.mmap] = []
                if os.name == "nt":
                    compute = runtime.wave.snapshots.compute

                    async def hold_built_file[T](
                        call: Callable[[], T], token: nous_native.CancellationToken
                    ) -> T:
                        result = await compute(call, token)
                        if not held and isinstance(result, str):
                            built = json.loads(result)
                            signature = built["identity"]["component_signature"]
                            root = runtime.wave.snapshots.paths.resolve_owned(
                                "index", "wave/components"
                            )
                            component = next(
                                path for path in root.iterdir() if path.name.endswith(signature)
                            )
                            with (component / "lexical/meta.json").open("rb") as file:
                                held.append(mmap.mmap(file.fileno(), 0, access=mmap.ACCESS_READ))
                        return result

                    monkeypatch.setattr(runtime.wave.snapshots, "compute", hold_built_file)
                try:
                    answer = await cognition.retrieval.query(
                        CognitiveQuery(text="semantic question", mode="dense", top_k=1)
                    )
                    if os.name == "nt":
                        assert held
                        await runtime.wave.cleanup()
                finally:
                    for mapping in held:
                        mapping.close()
                async with runtime.wave.snapshots.borrow(answer.context.snapshot_ref):
                    await cognition.records.add_note("A later semantic document.", remember=True)
                    later = await cognition.retrieval.query(
                        CognitiveQuery(text="semantic question", mode="dense", top_k=1)
                    )
                    async with runtime.database.read() as tx:
                        summary = (
                            await tx.connection.execute(
                                sa.select(schema.snapshots.c.build_summary).where(
                                    schema.snapshots.c.id == later.context.snapshot_ref.id
                                )
                            )
                        ).scalar_one()
                    assert summary["statistics"]["timings"]["ann_delta"]["incremental"] is True
                    assert later.groups
                passage = answer.groups[0].passages[0]
                assert passage.candidate_ref.key.endswith(":whole")
                assert passage.passage_reason == "local_candidate"
                assert len(passage.text) < len(body)
                assert (
                    await runtime.artifacts.read_selection(passage.selection.selections[0])
                    == passage.text
                )
                source = await cognition.records.read(note.record.revision_ref)
                manifest = tmp_path / "semantic-manifest.json"
                manifest.write_text(
                    json.dumps(
                        {
                            "name": "semantic-manual",
                            "documents": [
                                {
                                    "key": "doc",
                                    "title": "Mountain title",
                                    "input": {"artifact": source.body_artifact.identifier},
                                }
                            ],
                            "concepts": [
                                {
                                    "key": "mountain",
                                    "label": "Mountain",
                                    "description": "A high natural landform",
                                }
                            ],
                            "source_sequences": [
                                {"document": "doc", "concepts": ["mountain"], "ordered": True}
                            ],
                            "candidates": [
                                {
                                    "kind": "exact",
                                    "key": "doc",
                                    "document": "doc",
                                    "content": {"document": "doc"},
                                    "concepts": ["mountain"],
                                }
                            ],
                        }
                    ),
                    encoding="utf-8",
                )
                async with runtime.execution.operation("test.semantic_recovery"):
                    view = await runtime.wave.snapshots.import_manifest(manifest)
                    profile = runtime.wave.resolve_build_profile("test-space")
                    prepared = await runtime.wave.prepare_build(
                        view, profile, build_key="semantic-recovery"
                    )
                    recovered = await runtime.wave.prepare_build(
                        view, profile, build_key="semantic-recovery"
                    )
                    assert recovered == prepared
                    assert {cast(str, item["kind"]) for item in recovered.vector_mappings} == {
                        "candidate",
                        "concept",
                    }
                    async with runtime.database.write() as tx:
                        first = await runtime.wave.accept_in(tx, recovered)
                        await runtime.wave.promote_in(tx, first.ref)
                    runtime.wave.release_prepared(prepared)
                    runtime.wave.release_prepared(recovered)
                    another = await runtime.wave.prepare_build(
                        view.model_copy(update={"metadata": {"updated": True}}), profile
                    )
                    assert another.graph_signature == recovered.graph_signature
                    assert another.units[0].signature == recovered.units[0].signature
                    async with runtime.database.write() as tx:
                        second = await runtime.wave.accept_in(tx, another)
                        await runtime.wave.promote_in(tx, second.ref)
                    runtime.wave.release_prepared(another)
                    candidate = view.candidates[0]
                    updated = view.model_copy(
                        update={
                            "candidates": (
                                *view.candidates,
                                candidate.model_copy(update={"key": "doc-second"}),
                            )
                        }
                    )
                    derived = await runtime.wave.prepare_build(updated, profile)
                    stats = cast(dict[str, object], derived.summary["statistics"])
                    timing = cast(dict[str, object], stats["timings"])
                    ann = cast(dict[str, object], timing["ann_delta"])
                    assert timing["math_reused"] is True and ann["incremental"] is True
                    async with runtime.database.write() as tx:
                        third = await runtime.wave.accept_in(tx, derived)
                        await runtime.wave.promote_in(tx, third.ref)
                    runtime.wave.release_prepared(derived)
                    await runtime.wave.cleanup()
                    async with runtime.database.read() as tx:
                        candidate_vectors = await tx.connection.scalar(
                            sa.select(sa.func.count()).select_from(schema.projection_vectors)
                        )
                        concept_vectors = await tx.connection.scalar(
                            sa.select(sa.func.count()).select_from(schema.projection_graph_vectors)
                        )
                    assert candidate_vectors and concept_vectors
                    result = await runtime.wave.query(
                        WaveQuery(
                            snapshot=third.ref.identifier, text="semantic question", mode="dense"
                        )
                    )
                    assert result["hits"]

        asyncio.run(scenario())
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_publication_failure_and_cancellation_release_prepared_leases(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "failed-publication"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            await cognition.records.add_note("Lease cleanup observation.", remember=True)
            original = runtime.wave.accept_in

            async def interrupted(tx: object, prepared: object) -> object:
                raise asyncio.CancelledError()

            runtime.wave.accept_in = interrupted  # type: ignore[method-assign, assignment]
            try:
                with pytest.raises(asyncio.CancelledError):
                    await cognition.retrieval.query(
                        CognitiveQuery(text="observation", mode="lexical")
                    )
                assert not runtime.wave.snapshots._prepared_counts  # pyright: ignore[reportPrivateUsage]
                with pytest.raises(asyncio.CancelledError):
                    await runtime.wave.build(Path("docs/examples/wave/collection.json"))
                assert not runtime.wave.snapshots._prepared_counts  # pyright: ignore[reportPrivateUsage]
            finally:
                runtime.wave.accept_in = original  # type: ignore[method-assign]
            found = await cognition.retrieval.query(
                CognitiveQuery(text="observation", mode="lexical")
            )
            assert found.groups

    asyncio.run(scenario())


def test_query_results_cannot_mutate_reused_snapshot(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "snapshot-isolation"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            await cognition.records.add_note(
                "A distinctive isolationtoken body.", title="Original title", remember=True
            )
            initial = await cognition.retrieval.query(
                CognitiveQuery(text="isolationtoken", mode="lexical")
            )
            request = WaveQuery(
                snapshot=initial.context.snapshot_ref.identifier,
                text="isolationtoken",
                mode="lexical",
            )
            first = await runtime.wave.query(request)
            hit = cast(list[dict[str, object]], first["hits"])[0]
            cast(dict[str, object], hit["metadata"])["title"] = "corrupted"
            again = await runtime.wave.query(request)
            assert (
                cast(
                    dict[str, object], cast(list[dict[str, object]], again["hits"])[0]["metadata"]
                )["title"]
                == "Original title"
            )
            inspection = await runtime.wave.inspect(request.snapshot)
            cast(dict[str, object], inspection["metadata"])["injected"] = True
            assert "injected" not in cast(
                dict[str, object], (await runtime.wave.inspect(request.snapshot))["metadata"]
            )

    asyncio.run(scenario())
