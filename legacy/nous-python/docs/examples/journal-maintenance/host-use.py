"""Create, precisely edit and retrieve a Journal through the in-process API."""

from __future__ import annotations

import argparse
import asyncio
from pathlib import Path

from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.cognition.retrieval.operations import QUERY
from nous.cognition.writing.contracts import (
    AfterWrite,
    ApplyEdits,
    CreateJournal,
    DiffRecords,
    ReadRecord,
)
from nous.cognition.writing.operations import APPLY_EDITS, CREATE_JOURNAL, DIFF_RECORDS, READ_RECORD
from nous.content import ReplaceQuote
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import open_instance


async def main(instance: Path) -> None:
    async with open_instance(instance) as runtime:
        created = await runtime.app.call(
            CREATE_JOURNAL,
            CreateJournal(
                namespace="default",
                body=InlineText(text="# 会议记录\n\n会议在周二举行。\n\n会议讨论了缓存更新。\n"),
                after=AfterWrite(annotation="skip"),
            ),
        )
        original = await runtime.app.call(
            READ_RECORD, ReadRecord(target=created.commit.revision_ref)
        )
        # Whole-body scope is safe here: the exact quoted sentence is unique.
        body_scope = original.body_selection
        corrected = await runtime.app.call(
            APPLY_EDITS,
            ApplyEdits(
                target=original.revision_ref,
                expected=original.revision_ref,
                changes=(
                    ReplaceQuote(
                        scope=body_scope, before="会议在周二举行。", after="会议在周三举行。"
                    ),
                ),
                change_kind="correction",
                reason="依据会议日程修正日期",
                after=AfterWrite(annotation="skip"),
            ),
        )
        change = await runtime.app.call(
            DIFF_RECORDS,
            DiffRecords(left=original.revision_ref, right=corrected.commit.revision_ref),
        )
        print(change.unified)
        old = await runtime.app.call(
            READ_RECORD, ReadRecord(target=original.revision_ref, sources=True)
        )
        print("原版本：", old.body)
        found = await runtime.app.call(QUERY, CognitiveQuery(text="会议日期", mode="lexical"))
        for group in found.groups:
            print(
                group.context.revision_ref.identifier, group.title, group.passages[0].delivery.text
            )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "instance", type=Path, help="Existing initialized instance configuration directory"
    )
    asyncio.run(main(parser.parse_args().instance))
