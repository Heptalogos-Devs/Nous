"""在同一个宿主中保存正式文稿、执行后台维护并检索。

在已初始化实例上运行：python host-use.py INSTANCE_DIRECTORY
本例默认跳过语言标注，只演示真实后台刷新与精确查询；
配置好 annotate-concepts 任务后使用 --annotate 接上完整语言维护。
"""

from __future__ import annotations

import argparse
from pathlib import Path

from nous.cognition.material_contracts import ReadMaterial
from nous.cognition.materials import READ_MATERIAL
from nous.cognition.retrieval.contracts import QueryRequest, SaveEvidence
from nous.cognition.retrieval.operations import QUERY, SAVE_EVIDENCE
from nous.cognition.writing.contracts import AfterWrite, CreateJournal
from nous.cognition.writing.operations import CREATE_JOURNAL
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
        QUERY, QueryRequest(namespace=namespace, text="取消后何时释放资源？")
    )
    for group in result.evidence.materials:
        view = await runtime.app.call(
            READ_MATERIAL, ReadMaterial(ref=group.material.ref, sources=True)
        )
        print(view.material.title, [item.body_text for item in view.selections])
    if result.evidence.passages:
        evidence_ref = await runtime.app.call(SAVE_EVIDENCE, SaveEvidence(evidence=result.evidence))
        passage = result.evidence.passages[0]
        view = await runtime.app.call(
            READ_MATERIAL, ReadMaterial(delivery=evidence_ref, key=passage.key)
        )
        print("已保存证据：", evidence_ref.identifier, [item.body_text for item in view.selections])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("instance", type=Path)
    parser.add_argument("--namespace", default="default")
    parser.add_argument("--annotate", action="store_true")
    args = parser.parse_args()

    async def application(runtime: Runtime) -> None:
        await main(runtime, args.namespace, args.annotate)

    run_host(args.instance, application)
