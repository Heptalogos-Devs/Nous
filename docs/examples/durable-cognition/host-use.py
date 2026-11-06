"""在同一个宿主中保存正式文稿、执行持久维护并检索。

在已初始化实例上运行：python host-use.py INSTANCE_DIRECTORY
本例默认跳过语言标注，只演示真实后台刷新与精确查询；
配置好 annotate-concepts 任务后使用 --annotate 接上完整语言维护。
"""

from __future__ import annotations

import argparse
import asyncio
from pathlib import Path

from nous.cognition import AfterWrite, CognitiveQuery
from nous.runtime.bootstrap import open_instance


async def main(instance: Path, namespace: str, annotate: bool) -> None:
    async with open_instance(instance, workers=True) as runtime:
        cognition = await runtime.cognition(namespace)
        saved = await cognition.records.create_journal(
            "# 资源生命周期\n\n取消等待后，需要等实际工作退出再释放资源。\n",
            after=AfterWrite(
                annotation="run" if annotate else "skip",
                retrieval="refresh",
                retrieval_mode="lexical",
                execution="background",
            ),
        )
        print("已保存：", saved.record.revision_ref.identifier)
        pending = list(saved.jobs)
        seen: set[str] = set()
        while pending:
            job = pending.pop(0)
            if job.workflow_id in seen:
                continue
            seen.add(job.workflow_id)
            completed = await runtime.jobs.wait(job)
            print("维护结果：", job.workflow_id, completed.state)
            pending.extend(completed.related_jobs)
        result = await cognition.retrieval.query(
            CognitiveQuery(text="取消后何时释放资源？", mode="lexical")
        )
        for group in result.groups:
            view = await cognition.records.read(group.revision_ref, sources=True)
            print(view.revision_ref.identifier, view.body)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("instance", type=Path)
    parser.add_argument("--namespace", default="default")
    parser.add_argument("--annotate", action="store_true")
    args = parser.parse_args()
    asyncio.run(main(args.instance, args.namespace, args.annotate))
