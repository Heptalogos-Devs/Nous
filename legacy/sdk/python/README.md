# Nous Python SDK

通过异步 Python 客户端保存资料与记忆、检索内容、读取来源和修订认识。SDK 支持 Python 3.11 及以上版本，可独立安装并连接本地或远程 Nous。

## 安装与连接

在 Nous 仓库根目录安装客户端：

```sh
python -m pip install ./sdk/python
```

准备服务地址、服务授予的凭据文件和一个可读写的 Namespace。程序使用以下环境变量：

| 变量 | 含义 |
| --- | --- |
| `NOUS_URL` | 服务地址，默认 `http://127.0.0.1:7441` |
| `NOUS_CREDENTIAL_FILE` | 凭据文件的实际路径 |
| `NOUS_NAMESPACE` | 已创建的 Namespace 名称或 ID |

`await client.namespace(name)` 解析名称并返回绑定对象。精确 ID 可使用 `await client.namespace_by_id(id)`。后续操作复用这一绑定。

## 保存文本并查询来源

将下面的完整程序保存为 `read_notes.py`。它读取用户指定的 UTF-8 文件，将正文保存为明确记忆，再进行词项查询和来源读取；这条操作无需配置模型。

```python
import asyncio
import os
import sys
from pathlib import Path

from nous_sdk import NousClient


async def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("用法: python read_notes.py <实际文本文件> <查询内容>")
    path = Path(sys.argv[1]).expanduser().resolve(strict=True)
    content = path.read_text(encoding="utf-8-sig")
    credential = Path(os.environ["NOUS_CREDENTIAL_FILE"]).expanduser()
    namespace = os.environ["NOUS_NAMESPACE"]
    url = os.environ.get("NOUS_URL", "http://127.0.0.1:7441")

    async with NousClient(url, credential_file=credential) as client:
        ns = await client.namespace(namespace)
        saved = await ns.remember(content)
        print(saved.reference.exact())

        result = await ns.query(sys.argv[2], profile="lexical", limit=5)
        print(result.text)
        items = result.view().items
        if items:
            detail = await ns.read(items[0].ref, sources=True)
            print(detail.text)


if __name__ == "__main__":
    asyncio.run(main())
```

例如，在配置好上述环境变量后，读取仓库的真实 README：

```sh
python read_notes.py README.md Namespace
```

查询结果提供 `q3:1` 一类带查询组的短引用，可直接传给 `ns.read()`、`ns.sources()` 或 `ns.correct()`。每次继续读取都按当前授权取得内容。完整结果保留类型化对象、评分和执行信息，`result.text` 与 `result.view()` 提供可读投影。

## 查询与修订

`ns.query()` 接受自然语言、NousQL 或完整的 `QueryRequest`。查询时尽量写清已经知道的人物、事项和时间；已有会话、工作、人物引用和附件可以通过可选的 `QueryContext` 提供。NousQL 的显式选择器适合精确指定对象、标签、关系和时间条件。

`ns.correct(ref, content, reason=...)` 保存目标的正式修订；`ns.history(ref)` 读取其修订历史。`ns.read(ref, sources=True)` 同时展开来源，长内容通过返回的游标继续读取。

`ns.remember()` 和 `ns.correct()` 自动分配操作身份。宿主需要在网络中断后重试同一写入时，可显式传入同一个 `operation_id`。

## 生命周期与错误

使用 `async with NousClient(...)` 管理连接生命周期。`NousError` 保留服务端错误码、消息与必要上下文。查询空结果正常返回；后台任务、权限拒绝和版本冲突由各自的正式结果或错误表达。

服务端凭据决定可访问的 Namespace、读写能力与资料范围。SDK 合同可以从 `nous_sdk.contracts` 独立导入。
