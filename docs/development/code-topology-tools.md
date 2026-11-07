# 代码调用与数据流工具

现有的 CodeGraph 和 Zvec-grep 用于源码定位，使用方式见[代码与资料检索](../agents/retrieval.md)。Code-Graph-RAG 和 symgraph 可用于进一步展开调用链、查找模块耦合；CodeQL 可针对具体问题查询数据流。图上的关系应对照调用点和源码判断，跨 Python 与 Rust 的调用还需查看 PyO3 桥接。

## Code-Graph-RAG

当前 Windows 工作区将 Code-Graph-RAG 安装在 `temp/venvs/code-graph-rag/`，索引放在 `temp/tools/code-graph-rag/`。Codex 的 `nous-code-graph-rag` MCP 使用只读工具、Jedi 解析和 `io` 数据流捕获；配置变化后重新连接 MCP 或重启 Codex。`.cgrignore` 排除已退役的 `legacy/`。

从仓库根目录更新索引并查看调用者：

```powershell
$env:CGR_HOME = Join-Path (Get-Location) 'temp/tools/code-graph-rag'
$env:SQLITE_GRAPH_PATH = Join-Path $env:CGR_HOME 'nous.sqlite'
$env:PYTHON_FRONTEND = 'jedi'
$env:CGR_CAPTURE = 'io'
$env:CGR_SKIP_EMBEDDINGS = '1'
$cgr = Join-Path (Get-Location) 'temp/venvs/code-graph-rag/Scripts/cgr.exe'
& $cgr start --repo-path . --update-graph --no-embeddings
& $cgr graph resolve 'WaveOperations.query' --repo-path .
```

`resolve` 输出包含项目名的完整限定名，可将其传给 `cgr graph callers` 或 `cgr graph callees`。MCP 提供相应的 `callers`、`callees` 和 `flow_verdict` 工具。数据流查询返回 `UNKNOWN` 时，需要根据具体代码继续追踪。

## symgraph

`temp/tools/symgraph/symgraph.exe` 分别为 `src/` 和 `native/` 建索引，避免两种语言的同名符号相互干扰。Codex 使用 `nous-symgraph-python` 和 `nous-symgraph-native` 两个 MCP。查询同名方法时指定文件；模块环、扇入扇出和耦合排序用于发现线索。

```powershell
$symgraph = Join-Path (Get-Location) 'temp/tools/symgraph/symgraph.exe'
$env:SYMGRAPH_ROOT = Join-Path (Get-Location) 'src'
$env:SYMGRAPH_DB = Join-Path (Get-Location) 'temp/tools/symgraph/nous-python.db'
& $symgraph index $env:SYMGRAPH_ROOT
& $symgraph callees query --file nous/wave/operations.py
& $symgraph module-graph --granularity dir
```

分析 Rust 时，将 `SYMGRAPH_ROOT` 改为 `native/`，将 `SYMGRAPH_DB` 改为 `temp/tools/symgraph/nous-native.db`，再运行 `index` 和查询。

## CodeQL

CodeQL CLI 位于 `temp/tools/codeql/codeql/`，可针对 Python 或 Rust 等语言分别建库、编写查询。当前 Python 实验库位于 `temp/work/topology-tools/codeql-python-runtime/`，只包含 `src/`；实验查询和所需库包分别位于 `temp/work/topology-tools/` 和 `temp/tools/codeql/packs/`。实验库是快照，源码变更后需要重新建库。

Windows 上建 Python 库时，设置 `python.python_executable_name=python`，并让可用的 Python 位于 `PATH` 前部。CodeQL 的查询包和使用范围见[官方说明](https://docs.github.com/en/code-security/concepts/code-scanning/codeql/codeql-cli)。
