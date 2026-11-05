"""Create, precisely edit and retrieve a Journal through the in-process API."""
from __future__ import annotations

import argparse
import asyncio
from pathlib import Path

from nous.cognition import AfterWrite, CognitiveQuery
from nous.content import ReplaceQuote
from nous.runtime.bootstrap import open_instance


async def main(instance: Path) -> None:
    async with open_instance(instance) as runtime:
        cognition = await runtime.cognition(namespace="default")
        created = await cognition.records.create_journal(
            "# 会议记录\n\n会议在周二举行。\n\n会议讨论了缓存更新。\n",
            after=AfterWrite(annotation="skip", retrieval="defer"),
        )
        original = await cognition.records.read(created.record.revision_ref)
        # Whole-body scope is safe here: the exact quoted sentence is unique.
        body_scope = original.body_selection
        corrected = await cognition.records.apply(
            original.revision_ref,
            [ReplaceQuote(scope=body_scope, before="会议在周二举行。", after="会议在周三举行。")],
            intent="correction",
            reason="依据会议日程修正日期",
            after=AfterWrite(annotation="skip", retrieval="defer"),
        )
        change = await cognition.records.diff(
            original.revision_ref, corrected.record.revision_ref
        )
        print(change.unified)
        old = await cognition.records.read(original.revision_ref, sources=True)
        print("原版本：", old.body)
        found = await cognition.retrieval.query(CognitiveQuery(text="会议日期", mode="lexical"))
        for group in found.groups:
            print(group.revision_ref.identifier, group.title, group.passages[0].text)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("instance", type=Path, help="Existing initialized instance configuration directory")
    asyncio.run(main(parser.parse_args().instance))
