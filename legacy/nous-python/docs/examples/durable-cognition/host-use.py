"""在同一个宿主中保存正式文稿、执行后台维护并检索。

在已初始化实例上运行：python host-use.py INSTANCE_DIRECTORY
本例默认跳过语言标注，只演示真实后台刷新与精确查询；
配置好 annotate-concepts 任务后使用 --annotate 接上完整语言维护。
"""

from __future__ import annotations

import argparse
from pathlib import Path

from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.cognition.retrieval.operations import QUERY
from nous.cognition.writing.contracts import AfterWrite, CreateJournal, ReadRecord
from nous.cognition.writing.operations import CREATE_JOURNAL, READ_RECORD
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import Runtime
from nous.runtime.host import run_host
from nous.runtime.operations import WAIT_OPERATION, SelectOperation


async def main(runtime: Runtime, namespace: str, annotate: bool) -> None:
    saved = await runtime.app.call(
        CREATE_JOURNAL,
        CreateJournal(
            namespace=namespace,
            body=InlineText(text="# 资源生命周期\n\n取消等待后，需要等实际工作退出再释放资源。\n"),
            after=AfterWrite(
                annotation="run" if annotate else "skip",
                refresh="run",
                mode="lexical",
                execution="background",
            ),
        ),
    )
    print("已保存：", saved.commit.revision_ref.identifier)
    if saved.maintenance.execution == "started":
        completed = await runtime.app.call(
            WAIT_OPERATION, SelectOperation(ref=saved.maintenance.operation_ref)
        )
        print("维护结果：", completed.ref.operation_id, completed.status)
    result = await runtime.app.call(
        QUERY, CognitiveQuery(namespace=namespace, text="取消后何时释放资源？", mode="lexical")
    )
    for group in result.groups:
        view = await runtime.app.call(
            READ_RECORD, ReadRecord(target=group.context.revision_ref, sources=True)
        )
        print(view.revision_ref.identifier, view.body)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("instance", type=Path)
    parser.add_argument("--namespace", default="default")
    parser.add_argument("--annotate", action="store_true")
    args = parser.parse_args()

    async def application(runtime: Runtime) -> None:
        await main(runtime, args.namespace, args.annotate)

    run_host(args.instance, application)
