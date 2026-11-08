"""Ordinary record changes update only their own lexical candidates."""

import asyncio
from pathlib import Path

import sqlalchemy as sa

from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.wave import schema


def test_archive_first_then_revise_second_does_not_rewrite_other_records(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "instance"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            cognition = await runtime.cognition()
            records = [
                (
                    await cognition.records.add_note(
                        f"common text number {index}", title=f"title {index}", remember=True
                    )
                ).record
                for index in range(32)
            ]
            old = await cognition.retrieval.query(CognitiveQuery(text="common", mode="lexical"))
            old_ref = old.context.snapshot_ref
            async with runtime.wave.snapshots.borrow(old_ref):
                await cognition.records.archive(records[0].ref, expected=records[0].revision_ref)
                await cognition.records.replace(
                    records[1].ref,
                    "common text number 1",
                    expected=records[1].revision_ref,
                    title="updated title",
                )
                current = await cognition.retrieval.query(
                    CognitiveQuery(text="common", mode="lexical")
                )
                async with runtime.database.read() as tx:
                    summary = (
                        await tx.connection.execute(
                            sa.select(schema.snapshots.c.build_summary).where(
                                schema.snapshots.c.id == current.context.snapshot_ref.id
                            )
                        )
                    ).scalar_one()
                delta = summary["statistics"]["timings"]["lexical_delta"]
                assert delta["incremental"] is True
                # One archive and one revision: remove two old keys, add one new key.
                assert delta["changed"] == 3
                retained = await cognition.retrieval.query(
                    CognitiveQuery(
                        text="common text number 0", mode="lexical", snapshot=old_ref.identifier
                    )
                )
                assert any(
                    "number 0" in passage.snippet
                    for group in retained.groups
                    for passage in group.passages
                )

    asyncio.run(scenario())
