"""Reused projection assets retain their integrity and publication boundary."""

import asyncio
import json
from pathlib import Path

import pytest
import sqlalchemy as sa
from pydantic import ValidationError

from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.common.errors import NousError
from nous.content.contracts import ArtifactRef
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema
from nous.wave.contracts import EvidenceProjection


def test_reused_projection_asset_detects_corruption_and_accepts_equivalent_json(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            first = (
                await cognition.records.add_note("First durable orchard.", remember=True)
            ).record
            baseline = await cognition.retrieval.query(
                CognitiveQuery(text="orchard", mode="lexical")
            )
            async with runtime.database.read() as tx:
                signature = await tx.connection.scalar(
                    sa.select(schema.snapshot_members.c.unit_signature)
                    .where(
                        schema.snapshot_members.c.snapshot_id == baseline.context.snapshot_ref.id
                    )
                    .order_by(schema.snapshot_members.c.ordinal)
                    .limit(1)
                )
            assert signature is not None
            path = runtime.paths.resolve_owned("index", "wave/units") / f"{signature}.json"
            canonical = path.read_bytes()
            value = json.loads(canonical)
            try:
                path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
                second = (
                    await cognition.records.add_note("Second durable orchard.", remember=True)
                ).record
                updated = await cognition.retrieval.query(
                    CognitiveQuery(text="orchard", mode="lexical")
                )
                assert any(group.revision_ref == first.revision_ref for group in updated.groups)
                assert any(group.revision_ref == second.revision_ref for group in updated.groups)
                corrupt = dict(value)
                corrupt["document_key"] += "-corrupt"
                path.write_text(json.dumps(corrupt), encoding="utf-8")
                third = (
                    await cognition.records.add_note("Third unique peach token.", remember=True)
                ).record
                with pytest.raises(NousError) as failure:
                    await cognition.retrieval.query(CognitiveQuery(text="peach", mode="lexical"))
                assert failure.value.code == "cognition.projection_failed"
                cause = failure.value.details["cause"]
                assert isinstance(cause, dict) and cause["code"] == "wave.manifest_invalid"
                async with runtime.database.read() as tx:
                    current = await tx.connection.scalar(
                        sa.select(schema.collections.c.current_snapshot_id).where(
                            schema.collections.c.current_snapshot_id
                            == updated.context.snapshot_ref.id
                        )
                    )
                assert current == updated.context.snapshot_ref.id
                assert not runtime.wave.snapshots._prepared_counts  # pyright: ignore[reportPrivateUsage]
            finally:
                path.write_bytes(canonical)
            restored = await cognition.retrieval.query(CognitiveQuery(text="peach", mode="lexical"))
            assert any(group.revision_ref == third.revision_ref for group in restored.groups)

    asyncio.run(scenario())


def test_projection_roots_reject_unknown_and_duplicate_references() -> None:
    ref = ArtifactRef(instance_id="instance", id="artifact", identifier="artifact")
    for roots in (("missing",), ("known", "known")):
        with pytest.raises(ValidationError):
            EvidenceProjection(roots={"known": (ref,)}, documents={"document": roots})


def test_cross_document_candidate_and_document_layout_survive_incremental_reuse(
    tmp_path: Path,
) -> None:
    from typing import cast

    from nous.wave.contracts import WaveQuery

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

    asyncio.run(scenario())
