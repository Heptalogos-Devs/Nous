# Nous 开发指引

本文件适用于整个仓库。修改 `src/nous/` 时，同时遵循该目录的 [AGENTS.md](src/nous/AGENTS.md)。

## AntiRLHF

开发前必读 [AntiRLHF Charter](https://github.com/AntiRLHF-Research/AntiRLHF/blob/main/docs/charter/Revised.md)、[CONSTITUTION](https://github.com/AntiRLHF-Research/AntiRLHF/blob/main/CONSTITUTION.zh-CN.md)，尤其是 GPT 模型。AntiRLHF 是项目所有者自研的模型治理方案，针对局部优化、过度工程、过度测试、无意义兼容，以及用流程代替工程判断的行为。

将这些要求落实到实际选择：找到问题的责任归属，完成用户需要的使用通路，修改不合适的结构，并清除被替代的实现。不要用额外报告、检查清单或反复声明遵循规范来代替工作。

## 项目与当前工作

Nous 是独立安装的长期认知服务，面向记忆、知识、来源、时间、检索与认知成长。Python 主应用统一领域操作和正式入口，必要的 Native 计算承担性能热点。

当前运行代码在 `src/nous/`。`legacy/` 是退役的研究材料，不参与运行；需要其中的能力时按现行合同迁入，不恢复整套旧运行栈。

按任务阅读相关入口，不必把所有设计和历史讨论全读一遍：

| 要了解的内容 | 入口 |
| --- | --- |
| 当前能力、目标与后续工作 | [docs/CURRENT.md](docs/CURRENT.md) |
| 所有权、依赖方向与产品边界 | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| 设计专题、实施计划与文档职责 | [docs/README.md](docs/README.md) |
| 已有实现、符号及调用方 | [src/nous/INDEX.md](src/nous/INDEX.md) |
| 安装、运行与产品入口 | [README.md](README.md) |

`docs/design/` 定义现行合同，`docs/plans/` 安排具体工作的依赖和交付，`docs/sources/` 保留原始讨论。原始讨论中的提案不能直接当成已批准决定。代码与设计不一致时，结合当前需求判断应修改哪一侧，不以旧实现或旧测试为最终标准。

## 工程选择

- 暂保持 `v0.1.0`。项目没有外部使用者，不建立旧数据保留、备份迁移、代码兼容或双轨实现。结构、数据形态或框架不合适时，直接修正负责者，迁移实际调用方，删除被替代的代码和测试。
- 禁止为保住错误结构做局部绕过、包装和重复实现。先判断一个行为、数据或状态应由谁负责，再选择实现位置。现有模块可复用，也可因真实需求重新划分职责、重构。
- 优先采用已选成熟依赖的正式能力。不要自行重写协议客户端、媒体解析、数据库机制等已有能力，也不要为每个概念机械建立 service、repository、manager 或转发层。
- 保持供应商中立。LLM 服务经统一网关访问，New API 是可用网关之一；生产代码不为某个外部平台增加特殊业务。按明确协议、能力和任务配置工作，不根据模型名猜测能力。
- 完成一项能力时，同时处理实际生产者、消费者、配置和用户入口。不要留下只在内部方法或演示代码中成立的功能。

## 验证

禁止 TDD 和红绿循环。不得添加浅层测试、字段存在性测试，或重复验证依赖库已经负责的行为。除确有保护价值的关键契约、安全细节和独立故障外，优先通过真实用户入口验证完整通路（端到端）；验证错误输入和失败行为不属于为了红绿循环制造失败。

选择能暴露当前改动问题的验证方式。检查功能结果和失败行为，不以提交数、测试数或测试全绿代替产品判断。说明实际执行的验证和结果，区分模拟与真实外部服务，以及未验证的范围；不将开发报告或远端 CI 当成实际运行的证据。旧测试只保护旧实现时，随旧实现一起删除；仍有价值的使用情境迁到新入口。不要为单次测试给生产代码增加专用导出口、平行实现或搭建额外测试框架。

开发脚手架、临时探针、实验性测试和脚本放在 `temp/`，不得进入 Git。已有正式测试位于 `tests/`。

## 开发环境与命令

使用 Python 3.14 和 uv。先在仓库根目录设置开发环境，使虚拟环境、下载及缓存位于 `temp/`。

Linux / macOS：

```bash
source scripts/dev-env.sh
```

Windows PowerShell：

```powershell
. ./scripts/dev-env.ps1
```

安装开发依赖与查看入口：

```sh
uv sync --locked
uv run --locked nous --help
uv run --locked nous tools --help
```

已有检查命令如下，按改动选择使用，不要求每次小修改机械执行全部命令：

```sh
uv run --locked pytest tests/python
uv run --locked ruff check src tests/python
uv run --locked ruff format --check src tests/python
uv run --locked basedpyright
```

音视频处理需要 FFmpeg 和 ffprobe。通过 PATH 或 `media.ffmpeg_path`、`media.ffprobe_path` 配置；具体产品命令见 [工具 CLI](docs/design/interfaces/tools-cli.md)。涉及模型资源时阅读 [模型配置示例](docs/examples/models.toml) 和相关设计，不把某次开发机器的网关地址、模型名或凭据写成产品默认值。

本项目跨平台。除需求本身涉及平台适配外，不将开发机器的绝对路径或特有设置写入项目文档和 Git，不创建长期自启动或后台服务。

## 临时数据

生成产物、参考仓库、临时工作树、下载、安装和实验脚本统一放在仓库内 `temp/`，不要散落到用户目录或其它位置。已有外部开发工具可继续使用，新增项目临时内容仍遵循此约定。

使用 [temp 管理工具](scripts/temp.ps1) 建立目录、查看状态和清理。需要 PowerShell 7（`pwsh`），命令从仓库根目录运行：

```powershell
pwsh -File ./scripts/temp.ps1 init
pwsh -File ./scripts/temp.ps1 status
pwsh -File ./scripts/temp.ps1 clean
pwsh -File ./scripts/temp.ps1 clean -Apply
```

`init` 幂等补齐目录，不安装工具、不初始化实例；开发环境变量仍由 `scripts/dev-env.ps1` / `scripts/dev-env.sh` 设置。无参数默认 `status`；`status` 和不带 `-Apply` 的 `clean` 只预览。

| 目录 | 用途 |
| --- | --- |
| `temp/build/`、`temp/cache/` | 可重建的编译产物与依赖缓存 |
| `temp/runtime/` | 工具临时文件、pytest 临时目录 |
| `temp/work/<任务>/` | 一次性脚本、探针、实验输出和可丢弃实例；不散放在 temp 根目录 |
| `temp/reference-code/` | 可重新拉取的参考仓库与临时源码 |
| `temp/tools/`、`temp/venvs/` | 本地工具、下载的运行时和虚拟环境 |
| `temp/test-materials/` | 需要保留的输入材料、演示实例及可复查结果 |

测试材料按 `Audio/`、`Image/`、`PDF/`、`Text/`、`Video/` 分类；来源说明、授权和文稿跟随对应素材。配置、模板、约束和集合清单分别放 `Config/`、`Prompts/`、`Schemas/`、`Collections/`；运行结果、导出和诊断放 `Results/`、`Exports/`、`Diagnostics/`。保留的测试实例放 `test-materials/Instances/<名称>/`，实际创建使用 Nous 的 `init` 命令。清理时从旧散落目录救出的素材先放 `test-materials/Incoming/`，后续归类并修正本地引用。

`clean -Apply` 清理 `test-materials/` 之外的旧实例、参考源码、缓存、构建及临时工作，默认保留 `tools/`、`venvs/`，随后补齐空标准目录。`-IncludeToolchains` 同时清理工具和虚拟环境，使用后需重新安装。检测到的运行中进程、容器挂载、链接路径及 Git linked worktree 会跳过。执行清理前，把仍需保留的资料移入 `test-materials/`；不得用清理工具操作仓库外或当前任务未授权的数据。

完成工作后清理不再需要的临时脚本、测试产物和废弃构建。`temp/` 已被 Git 忽略；长期有用的产品示例、正式脚本和回归测试应迁入对应受版本管理的目录。

## 代码与参考项目检索

### Nous 源码

定位代码、理解符号和追踪调用关系时，先用 CodeGraph MCP 的 `codegraph_explore`，查询中注明文件、符号或具体问题。也可在项目目录执行：

```sh
codegraph explore "<符号或问题>"
```

CodeGraph 或索引不可用时，使用文件和符号检索继续定位； 也可选择安装或修复该工具，但需向开发者说明并请求许可。沿命中位置阅读负责模块和真实调用方，不凭搜索摘要决定修改。

### 参考实现

涉及功能实现和设计取舍时，针对当前问题检索相关参考项目的实际代码，阅读上下文，再结合 Nous 的领域语义、接口和依赖决定采用方式。引用或改编代码时保留所需的许可与来源。

参考仓库位于 `temp/reference-code/`。XERJ 是本地词法检索工具：使用 MCP `xerj_search`，按下表选择 `prefix`，使用源码中的英文术语或符号查询。未克隆仓库自行拉取。

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

## 维护文档

设计、公共契约或职责改变时，原地更新负责专题和源码索引，移除失效说明。同一规则只在一个负责位置定义，其它地方引用，不复制多套版本。

`CURRENT.md` 记录当前能力、有效决定和剩余工作，不写成操作日志。不要把每次纠错写成永久禁令，也不要将退役方案、实现过程或自我评审说明留在产品文档、代码注释和用户界面中。
