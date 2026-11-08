"""Reused projection assets retain their integrity and publication boundary."""

import asyncio
import json
from pathlib import Path
from typing import cast

import pytest
import sqlalchemy as sa
from pydantic import ValidationError
from sqlalchemy.exc import IntegrityError

from nous.common.errors import NousError
from nous.content import schema as content_schema
from nous.content.contracts import ArtifactRef, SourceSelection
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema
from nous.wave.contracts import EvidenceProjection, SnapshotRef, WaveQuery


def test_pending_snapshot_recovers_after_rollback_and_reopen(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            async with runtime.execution.operation("test.wave"):
                view = await runtime.wave.snapshots.import_manifest(
                    Path("docs/examples/wave/collection.json")
                )
                profile = runtime.wave.resolve_build_profile(None)
                async with runtime.wave.prepare_build(
                    view, profile, build_key="pending"
                ) as prepared:
                    ref = prepared.ref
                    with pytest.raises(NousError):
                        await runtime.wave.describe(ref)
                    assert (await runtime.wave.status())["snapshots"] == 0
                    with pytest.raises(NousError):
                        async with runtime.database.write() as tx:
                            await runtime.wave.promote_in(tx, ref)
                    with pytest.raises(NousError):
                        async with runtime.database.write() as tx:
                            await runtime.wave.retain_in(
                                tx, ref, owner_kind="test", owner_id="test"
                            )
                    with pytest.raises(RuntimeError):
                        async with runtime.database.write() as tx:
                            await runtime.wave.accept_in(tx, prepared)
                            raise RuntimeError("abort")
                    with pytest.raises(NousError):
                        await runtime.wave.describe(ref)
                await runtime.wave.cleanup()
        async with open_instance(instance, environment={}) as runtime:
            async with runtime.wave.prepare_build(view, profile, build_key="pending") as recovered:
                assert recovered.ref == ref
                async with runtime.database.write() as tx:
                    descriptor = await runtime.wave.accept_in(tx, recovered)
                    await runtime.wave.promote_in(tx, descriptor.ref)
            result = await runtime.wave.query(
                WaveQuery(snapshot=ref.identifier, text="资源", mode="lexical")
            )
            assert result["hits"]

    asyncio.run(scenario())


def test_pending_snapshot_retains_sql_units_through_cleanup(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "pending-shared"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            async with runtime.execution.operation("test.wave"):
                view = await runtime.wave.snapshots.import_manifest(
                    Path("docs/examples/wave/collection.json")
                )
                profile = runtime.wave.resolve_build_profile(None)
                async with runtime.wave.prepare_build(
                    view, profile, build_key="original"
                ) as initial:
                    async with runtime.database.write() as tx:
                        first = await runtime.wave.accept_in(tx, initial)
                        await runtime.wave.promote_in(tx, first.ref)
                changed = view.model_copy(update={"metadata": {"pending": True}})
                async with runtime.wave.prepare_build(
                    changed, profile, build_key="pending-shared"
                ) as pending:
                    pending_ref = pending.ref
                async with runtime.database.read() as tx:
                    shared = set(
                        (
                            await tx.connection.scalars(
                                sa.select(schema.snapshot_members.c.unit_signature).where(
                                    schema.snapshot_members.c.snapshot_id == pending_ref.id
                                )
                            )
                        ).all()
                    )
                    assert shared
                empty = view.model_copy(update={"candidates": ()})
                async with runtime.wave.prepare_build(empty, profile, build_key="later") as later:
                    async with runtime.database.write() as tx:
                        newest = await runtime.wave.accept_in(tx, later)
                        await runtime.wave.promote_in(tx, newest.ref)
                assert first.ref.identifier in cast(
                    list[str], (await runtime.wave.cleanup())["snapshots"]
                )
                async with runtime.database.read() as tx:
                    retained = set(
                        (
                            await tx.connection.scalars(
                                sa.select(schema.projection_units.c.signature).where(
                                    schema.projection_units.c.signature.in_(shared)
                                )
                            )
                        ).all()
                    )
                assert retained == shared
                async with runtime.wave.prepare_build(
                    changed, profile, build_key="pending-shared"
                ) as recovered:
                    assert recovered.ref == pending_ref
                    async with runtime.database.write() as tx:
                        adopted = await runtime.wave.accept_in(tx, recovered)
                        await runtime.wave.promote_in(tx, adopted.ref)
                assert (
                    await runtime.wave.query(
                        WaveQuery(snapshot=adopted.ref.identifier, text="资源", mode="lexical")
                    )
                )["hits"]

    asyncio.run(scenario())


def test_cleanup_resumes_after_sql_commit_and_component_delete_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def scenario() -> None:
        instance = tmp_path / "cleanup-recovery"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            async with runtime.execution.operation("test.wave"):
                view = await runtime.wave.snapshots.import_manifest(
                    Path("docs/examples/wave/collection.json")
                )
                profile = runtime.wave.resolve_build_profile(None)
                async with runtime.wave.prepare_build(view, profile, build_key="first") as prepared:
                    async with runtime.database.write() as tx:
                        first = await runtime.wave.accept_in(tx, prepared)
                        await runtime.wave.promote_in(tx, first.ref)
                row = await runtime.wave.snapshots.resolve(first.ref)
                old_component = runtime.paths.resolve_owned("index", row["index_location"])
                changed = view.model_copy(
                    update={
                        "documents": (
                            view.documents[0].model_copy(update={"title": "Changed"}),
                            *view.documents[1:],
                        )
                    }
                )
                async with runtime.wave.prepare_build(
                    changed, profile, build_key="second"
                ) as prepared:
                    async with runtime.database.write() as tx:
                        current = await runtime.wave.accept_in(tx, prepared)
                        await runtime.wave.promote_in(tx, current.ref)

                remove = runtime.wave.snapshots._remove_owned  # pyright: ignore[reportPrivateUsage]

                def interrupted(path: Path, root: Path, name: str) -> None:
                    if path == old_component:
                        raise OSError("interrupted component deletion")
                    remove(path, root, name)

                monkeypatch.setattr(runtime.wave.snapshots, "_remove_owned", interrupted)
                with pytest.raises(OSError, match="interrupted component deletion"):
                    await runtime.wave.cleanup()
                assert old_component.exists()
                monkeypatch.setattr(runtime.wave.snapshots, "_remove_owned", remove)
                await runtime.wave.cleanup()
                assert not old_component.exists()
                result = await runtime.wave.query(
                    WaveQuery(snapshot=current.ref.identifier, text="资源", mode="lexical")
                )
                assert result["hits"]

    asyncio.run(scenario())


def test_projection_roots_are_validated_and_retained(tmp_path: Path) -> None:
    ref = ArtifactRef(instance_id="instance", id="artifact", identifier="artifact")
    for roots in (("missing",), ("known", "known")):
        with pytest.raises(ValidationError):
            EvidenceProjection(roots={"known": (ref,)}, documents={"document": roots})

    async def scenario() -> None:
        instance = tmp_path / "roots"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            extra = await runtime.artifacts.ingest("An independent source without a candidate.")
            async with runtime.execution.operation("test.wave"):
                view = await runtime.wave.snapshots.import_manifest(
                    Path("docs/examples/wave/collection.json")
                )
                evidence = view.evidence.model_copy(
                    update={"roots": {**view.evidence.roots, "extra": (extra.ref,)}}
                )
                profile = runtime.wave.resolve_build_profile(None)
                with_extra = view.model_copy(update={"evidence": evidence})
                async with runtime.wave.prepare_build(
                    with_extra, profile, build_key="complete-roots"
                ) as prepared:
                    async with runtime.database.write() as tx:
                        first = await runtime.wave.accept_in(tx, prepared)
                        await runtime.wave.promote_in(tx, first.ref)
                assert view.manifest_artifact is not None
                async with runtime.database.read() as tx:
                    duplicate = await tx.connection.scalar(
                        sa.select(sa.func.count())
                        .select_from(schema.snapshot_artifacts)
                        .where(
                            schema.snapshot_artifacts.c.snapshot_id == first.ref.id,
                            schema.snapshot_artifacts.c.artifact_id == view.manifest_artifact.id,
                        )
                    )
                assert duplicate == 0
                with pytest.raises(IntegrityError):
                    async with runtime.database.write() as tx:
                        await tx.connection.execute(
                            content_schema.artifacts.delete().where(
                                content_schema.artifacts.c.id == view.manifest_artifact.id
                            )
                        )
                async with runtime.wave.prepare_build(
                    with_extra, profile, build_key="complete-roots"
                ) as recovered:
                    assert recovered.ref == first.ref

                expected = len(evidence.roots)
                row = await runtime.wave.snapshots.resolve(first.ref)
                assert row["mapping"]["roots"]["extra"] == [extra.ref.model_dump(mode="json")]
                inspection = await runtime.wave.inspect(first.ref.identifier)
                assert inspection["source_roots"] == expected
                query = await runtime.wave.query(
                    WaveQuery(snapshot=first.ref.identifier, text="资源", mode="lexical")
                )
                assert (
                    query["hits"]
                    and cast(dict[str, object], query["scope"])["source_roots"] == expected
                )
                with pytest.raises(IntegrityError):
                    async with runtime.database.write() as tx:
                        await tx.connection.execute(
                            content_schema.artifacts.delete().where(
                                content_schema.artifacts.c.id == extra.ref.id
                            )
                        )

                async with runtime.wave.prepare_build(view, profile) as prepared:
                    async with runtime.database.write() as tx:
                        second = await runtime.wave.accept_in(tx, prepared)
                        await runtime.wave.promote_in(tx, second.ref)
                assert first.ref.identifier in cast(
                    list[str], (await runtime.wave.cleanup())["snapshots"]
                )
                async with runtime.database.read() as tx:
                    retained = await tx.connection.scalar(
                        sa.select(sa.func.count())
                        .select_from(schema.snapshot_artifacts)
                        .where(schema.snapshot_artifacts.c.artifact_id == extra.ref.id)
                    )
                assert retained == 0
                assert await runtime.artifacts.read_text(extra.ref) == (
                    "An independent source without a candidate."
                )

    asyncio.run(scenario())


def test_cross_document_candidate_and_document_layout_survive_incremental_reuse(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from nous.application.preparation import TextPreparation
    from nous.content.contracts import ContentSelection

    async def scenario() -> None:
        instance, manifest = tmp_path / "instance", tmp_path / "manifest.json"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            first = await runtime.artifacts.ingest("The first document has an orchard.")
            second = await runtime.artifacts.ingest(
                "The crossdocumentneedle comes from another source."
            )
            value = {
                "name": "cross-document",
                "documents": [
                    {"key": "first", "input": {"artifact": first.ref.identifier}},
                    {"key": "second", "input": {"artifact": second.ref.identifier}},
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
            manifest.write_text(json.dumps(value), encoding="utf-8")
            first_build = await runtime.wave.build(manifest)
            second_build = await runtime.wave.build(manifest)
            snapshots = [
                cast(dict[str, str], build["snapshot"])["identifier"]
                for build in (first_build, second_build)
            ]
            for snapshot in snapshots:
                result = await runtime.wave.query(
                    WaveQuery(
                        snapshot=snapshot,
                        text="crossdocumentneedle",
                        mode="lexical",
                        grouping="candidate",
                    )
                )
                hits = cast(list[dict[str, object]], result["hits"])
                match = next(hit for hit in hits if hit["candidate"] == "cross-candidate")
                content = cast(dict[str, object], match["content"])
                assert cast(dict[str, str], content["artifact"])["id"] == second.ref.id
            statistics = cast(dict[str, object], second_build["statistics"])
            preparation = cast(dict[str, object], statistics["preparation"])
            assert cast(int, preparation["reused_projection_units"]) == 2

            identity = TextPreparation.preparation_identity

            async def changed_identity(self: TextPreparation, source: ContentSelection):
                result = await identity(self, source)
                if source.artifact == second.ref:
                    result["parser_profile"] = "changed-source-profile"
                return result

            monkeypatch.setattr(TextPreparation, "preparation_identity", changed_identity)
            rebuilt = await runtime.wave.build(manifest)
            stats = cast(dict[str, object], rebuilt["statistics"])
            assert cast(dict[str, object], stats["preparation"])["reused_projection_units"] == 1
            result = await runtime.wave.query(
                WaveQuery(
                    snapshot=cast(dict[str, str], rebuilt["snapshot"])["identifier"],
                    text="crossdocumentneedle",
                    mode="lexical",
                )
            )
            assert any(
                hit["candidate"] == "cross-candidate"
                for hit in cast(list[dict[str, object]], result["hits"])
            )

    asyncio.run(scenario())


def test_graph_reuse_preserves_resolved_support_and_exact_sources(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "resolved-graph"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            async with runtime.execution.operation("test.wave"):
                view = await runtime.wave.snapshots.import_manifest(
                    Path("docs/examples/wave/collection.json")
                )
                original = next(doc for doc in view.documents if doc.key == "transactions")
                alias = original.model_copy(update={"key": "transaction-alias"})
                evidence = view.evidence.model_copy(
                    update={
                        "documents": {
                            **view.evidence.documents,
                            alias.key: view.evidence.documents[original.key],
                        }
                    }
                )
                initial_group = view.support_groups[0]
                exact_source = initial_group.basis_refs[0].model_copy(
                    update={"selector": SourceSelection(start_line=1, end_line=1)}
                )
                exact_group = initial_group.model_copy(
                    update={
                        "basis_refs": (exact_source, *initial_group.basis_refs[1:]),
                        "origin_ref": exact_source,
                    }
                )
                first_view = view.model_copy(
                    update={
                        "documents": (*view.documents, alias),
                        "evidence": evidence,
                        "support_groups": (exact_group, *view.support_groups[1:]),
                    }
                )
                first_group = first_view.support_groups[0]
                sources = tuple(
                    alias.key if key == original.key else key
                    for key in first_group.required_sources
                )
                second_view = first_view.model_copy(
                    update={
                        "support_groups": (
                            first_group.model_copy(update={"required_sources": sources}),
                            *first_view.support_groups[1:],
                        )
                    }
                )
                profile = runtime.wave.resolve_build_profile(None)
                refs: list[SnapshotRef] = []
                for key, selected in (("original", first_view), ("alias", second_view)):
                    async with runtime.wave.prepare_build(
                        selected, profile, build_key=key
                    ) as prepared:
                        async with runtime.database.write() as tx:
                            descriptor = await runtime.wave.accept_in(tx, prepared)
                            await runtime.wave.promote_in(tx, descriptor.ref)
                        refs.append(descriptor.ref)
                async with runtime.database.read() as tx:
                    graphs = (
                        await tx.connection.scalars(
                            sa.select(schema.snapshots.c.graph_signature).where(
                                schema.snapshots.c.id.in_([ref.id for ref in refs])
                            )
                        )
                    ).all()
                assert len(graphs) == 2 and len(set(graphs)) == 1
                first = await runtime.wave.snapshots.resolve(refs[0])
                second = await runtime.wave.snapshots.resolve(refs[1])
                assert first["mapping"]["support_groups"] == second["mapping"]["support_groups"]
                assert first["mapping"]["associations"] == second["mapping"]["associations"]
                group = next(
                    item
                    for item in first["mapping"]["support_groups"]
                    if item["key"] == first_group.key
                )
                assert set(cast(list[str], group["roots"])) == {
                    root
                    for key in first_group.required_sources
                    for root in first_view.evidence.documents[key]
                }
                basis = cast(list[dict[str, object]], group["basis_refs"])
                origin = cast(dict[str, object], group["origin_ref"])
                assert len(basis) == len(first_group.basis_refs)
                assert (
                    cast(dict[str, str], basis[0]["artifact"])["identifier"]
                    == original.input.artifact
                )
                assert origin == basis[0]
                assert cast(dict[str, object], origin["selection"])["start_line"] == 1
                assert cast(dict[str, object], origin["selection"])["end_line"] == 1
                result = await runtime.wave.query(
                    WaveQuery(snapshot=refs[1].identifier, text="资源", mode="lexical")
                )
                assert result["hits"]

    asyncio.run(scenario())
