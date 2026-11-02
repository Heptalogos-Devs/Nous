# AntiRLHF 宪法

AI / Agents 开发前必读，尤其当你是 GPT 模型，必读 Charter。AntiRLHF 是项目所有者自研的模型治理方案，用于控制 LLM 的局部优化、过度工程、过度测试、无意义兼容等行径。

- 仓库: https://github.com/AntiRLHF-Research/AntiRLHF  
- Charter: https://github.com/AntiRLHF-Research/AntiRLHF/blob/main/docs/charter/Revised.md

---

# Nous

保持 v0.1.0，目前仍是无外部使用者的原型开发项目。禁止任何形式的数据保留、代码兼容、双轨实现等。绝不进行兼容，出现形态错误、数据结构不合适、旧框架影响开发——直接重构、直接修正源头，禁止局部优化、绕过、包装、重复实现。

## 临时数据

统一使用仓库内 temp/ 目录，包含生成产物、参考代码/仓库等。不要将数据、工作树或者其他下载、安装、脚本放进其他位置，尤其是用户目录，这会难以清理。

## 开发环境

本项目跨平台，禁止将开发机器的路径、特有内容写入项目文档或者上传进 Git（除非是需求的平台适配）。不要长期的创建开机自启动、后台任务等服务。

## 代码与参考项目检索

### Nous 项目：CodeGraph

定位代码、理解符号和追踪调用关系时，先使用 CodeGraph MCP 的 `codegraph_explore`，查询中注明相关文件、符号或问题。也可在项目目录执行 `codegraph explore "<符号或问题>"`。CodeGraph 索引不可用时，先询问开发者是否安装并解释，否则使用其他检索工具。

### 参考项目：XERJ

编写代码前，针对当前问题检索相关参考项目的实际实现，阅读命中位置的上下文，再结合 Nous 的领域语义、接口和依赖决定采用方式。引用或改编代码时，保留实际使用所需的许可和来源。

参考仓库位于 Nous 同级的 `reference-code/`。XERJ 是本地词法检索：使用 MCP 工具 `xerj_search`，按下表选择 `prefix`，并用源码中的英文术语或符号查询。

| 前缀 | 参考项目 | 适用问题 |
|---|---|---|
| `refamemorix` | A-Memorix | 记忆与管理合同、画像、Episode、导入与 Bundle |
| `refmaibot` | MaiBot | 实际消费路径、插件、消息、模型和 Dashboard |
| `refvcp` | VCPToolBox | VCP、RiverMemo、检索与记忆实现 |
| `refmem0` | Mem0 | 长期记忆形成、检索与集成 |
| `refgraphiti` | Graphiti | 知识图谱、时间、实体及关系检索 |

XERJ MCP 注册名为 `xerj`，服务地址为 `http://127.0.0.1:19320`。没有 MCP 时，可通过命令行检索。结果提示索引过期时，在更新对应参考仓库后，用同一前缀重新建索引。以下命令以项目目录和 A-Memorix 为例。

**Linux：** 程序位于 `~/.local/bin/xerj`，索引数据位于 `~/.local/share/xerj/nous-reference/data`。服务未运行时，执行 `systemctl --user start xerj-nous.service`。

```bash
~/.local/bin/xerj search --url http://127.0.0.1:19320 --prefix refamemorix 'memory query retrieval'
~/.local/bin/xerj autoindex ../reference-code/A_memorix --url http://127.0.0.1:19320 --prefix refamemorix --workers 2 --no-graph --no-semantic --code-analyzer code --max-minutes 0
```

**Windows：** 程序位于 `%LOCALAPPDATA%\Programs\xerj\xerj.exe`，索引数据位于 `%LOCALAPPDATA%\xerj\nous-reference\data`。服务未运行时，先启动服务，再执行检索或重建索引：

```powershell
$xerj = Join-Path $env:LOCALAPPDATA 'Programs\xerj\xerj.exe'
$data = Join-Path $env:LOCALAPPDATA 'xerj\nous-reference\data'
Start-Process -WindowStyle Hidden -FilePath $xerj -ArgumentList @('--insecure', '--bind', '127.0.0.1', '--port', '19320', '--data-dir', "`"$data`"", '--disable-feedback')

& $xerj search --url http://127.0.0.1:19320 --prefix refamemorix 'memory query retrieval'
& $xerj autoindex ../reference-code/A_memorix --url http://127.0.0.1:19320 --prefix refamemorix --workers 2 --no-graph --no-semantic --code-analyzer code --max-minutes 0
```
