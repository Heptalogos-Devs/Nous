# 代码与资料检索

工作区入口 `dev.ps1` 和 `dev.sh` 的 `env` 模式设置当前 shell 的开发环境并创建所需的 `temp/` 目录，不安装工具或配置 MCP。`init` 模式安装缺失的 CodeGraph 和 Zvec-grep、准备本仓库索引并更新用户的 Codex MCP 配置；按用户要求初始化后，重新连接 MCP 或重启 Codex。两者的本仓库索引排除 `legacy/`。

## 检索本仓库

- 仓库已有 CodeGraph 索引时，定位源码或理解调用链先用 CodeGraph。`explore` 可同时获取相关源码和关系；已知符号需要单独定位时可用 MCP `codegraph_search`、CLI `codegraph query` 或 `node`，查调用关系可用 `callers`、`callees`、`impact`，指定文件区分同名符号。检查关键调用关系时结合精确文本搜索和实际源码。
- 不知道名称、需要跨代码和文档查找时，用 Zvec-grep 的 `zvec_grep_search` 或 `zg --hybrid "问题"`；定位源码仍先按上一条使用 CodeGraph。精确词项可用 `zg --fts "符号"`，穷尽引用可用 `zg --rg -F "符号" src`。中文文档可按中文问题搜索，英文代码问题可使用英文描述并按路径收窄。
- 索引可能滞后于文件修改。查询时关注新鲜度，CLI 需要同步结果时使用 `zg --refresh wait --hybrid "问题"`；现有 CodeGraph 索引需要更新时使用 `codegraph sync`。缺少索引时使用其他可用检索方式。

### CodeGraph：结构与上下文

在 MCP 中按需要使用 `codegraph_search`、`codegraph_node`、`codegraph_callers`、`codegraph_callees`、`codegraph_impact` 或 `codegraph_explore`。`node` 需要源码时传 `includeCode=true`；关系查询提供符号所在文件，避免同名符号混淆。`explore` 适合需要一组关联源码时使用，指定文件或符号，并按需要限制 `maxFiles`。

```sh
codegraph query RevisionSelection
codegraph node RevisionSelection --file src/nous/cognition/contracts.py
codegraph callers encode_batches --file src/nous/models/embeddings.py
codegraph explore "src/nous/cognition/writing/operations.py adopt"
```

图关系用于发现调用路径；重要的修改范围需再通过 `zg --rg` 或 `rg` 及源码确认。查看索引状态用 `codegraph status`，更新用 `codegraph sync`。

### Zvec-grep：语义与词项发现

MCP 的 `zvec_grep_search` 以项目目录为 `root`，可按自然语言、词项或组合查询。CLI 示例：

```sh
zg --hybrid "where a cancelled blocking task waits before releasing resources" --limit 5
zg --vector "how journal drafts are adopted" --limit 5
zg --fts "RevisionSelection" --limit 5
zg --rg -F "encode_batches" src tests
zg --refresh wait --hybrid "后台工作重试" --glob 'docs/**'
```

默认向量模型在本地运行；索引保存在项目的 `.zvec-grep/`，模型与服务状态位于 `temp/tools/zvec-grep/`。`zg --status` 查看索引，`zg --index` 更新；显式 `--mode direct` 的查询默认不等待刷新。使用结果中的路径与行号阅读原文，不把历史讨论当成当前实现。

## 参考项目

选用或替换通用机制，或当前设计问题需要借鉴参考实现时，检索适用参考项目的实际代码，阅读上下文，再结合 Nous 的领域语义、接口和依赖决定采用方式。引用或改编代码时保留所需的许可与来源。

参考仓库位于 `temp/reference-code/`。XERJ 是本地词法检索工具：使用 MCP `xerj_search`，按下表选择 `prefix`，使用源码中的英文术语或符号查询。需要本地参考仓库而尚未克隆时自行拉取；其他情况使用现有的可用资料和检索方式。

| 前缀 | 参考项目 | 适用问题 |
| --- | --- | --- |
| `refamemorix` | A-Memorix | 记忆与管理合同、画像、Episode、导入与 Bundle |
| `refmaibot` | MaiBot | 实际消费路径、插件、消息、模型和 Dashboard |
| `refvcp` | VCPToolBox | VCP、RiverMemo、检索与记忆实现 |
| `refmem0` | Mem0 | 长期记忆形成、检索与集成 |
| `refgraphiti` | Graphiti | 知识图谱、时间、实体及关系检索 |

已有 XERJ 配置的 MCP 注册名为 `xerj`，服务地址为 `http://127.0.0.1:19320`。没有 MCP 时可用 CLI。索引过期时，在更新对应参考仓库后以相同前缀重新建索引。以下命令适用于已经按此约定安装的环境；没有对应工具、服务或参考仓库时，改用可用检索方式。

Linux 的已有配置：程序为 `~/.local/bin/xerj`，索引位于 `~/.local/share/xerj/nous-reference/data`。已有服务停止时可执行 `systemctl --user start xerj-nous.service`，不因此新建自启动服务。

```bash
~/.local/bin/xerj search --url http://127.0.0.1:19320 --prefix refamemorix 'memory query retrieval'
~/.local/bin/xerj autoindex temp/reference-code/A_memorix --url http://127.0.0.1:19320 --prefix refamemorix --workers 2 --no-graph --no-semantic --code-analyzer code --max-minutes 0
```

Windows 的已有配置：程序为 `%LOCALAPPDATA%\Programs\xerj\xerj.exe`，索引位于 `%LOCALAPPDATA%\xerj\nous-reference\data`。服务未运行时，先启动本地进程，再检索或重建索引：

```powershell
$xerj = Join-Path $env:LOCALAPPDATA 'Programs\xerj\xerj.exe'
$data = Join-Path $env:LOCALAPPDATA 'xerj\nous-reference\data'
Start-Process -WindowStyle Hidden -FilePath $xerj -ArgumentList @('--insecure', '--bind', '127.0.0.1', '--port', '19320', '--data-dir', "`"$data`"", '--disable-feedback')

& $xerj search --url http://127.0.0.1:19320 --prefix refamemorix 'memory query retrieval'
& $xerj autoindex temp/reference-code/A_memorix --url http://127.0.0.1:19320 --prefix refamemorix --workers 2 --no-graph --no-semantic --code-analyzer code --max-minutes 0
```
