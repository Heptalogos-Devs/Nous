# Nous

Nous 是独立安装的长期认知服务，为智能体提供记忆、知识、来源、时间、检索与认知成长，也是 Heptalogos 的认知和人格中心。Python 主应用负责领域、模型、事务、CLI 与程序内 SDK；HTTP、MCP 与远程 SDK 沿后续路线接入。必要的 Native 计算承担 VCP 与检索索引等性能热点。

## 领域

| 名称 | 职责 |
|---|---|
| Nous Memory | 记忆、Episode、Journal、来源与认知修订 |
| Nous Wave | 词项、向量、图关联、VCP、候选融合与重排序等检索算法 |
| Nous Ego | 自我认识、人格倾向、种子、自我叙事、知识与能力边界 |
| Nous Social | 人物、群体、社会关系、跨平台身份认识与社会交流惯例 |
| Nous Context | 当前认知、工作上下文、认知投影与模型实际消费 |
| Nous Motivation | 愿望、关切和期望状态等长期认知 |

NousQL 是整个 Nous 的查询语言，由 Wave 执行检索。Ego 持有主体对自己的认识；当前心情和实际行为选择由 Heptalogos 持有。Social 的设计对黑话、表情包、GIF、音频梗等社会符号共同组织载体、内容描述、表达含义、情绪或意图、对象、情境和社会使用经验。

## 当前状态

当前 CLI 与程序内 SDK 提供 Note、Journal、草稿采用、概念标注、当前知识检索和反馈，以及实例、配置、诊断、时间、内容、媒体与 Wave 工具。正文和来源固定到精确版本；修订与归档保留历史读取。保存可以在同一事务中登记持久维护，DBOS 在宿主运行时执行标注和刷新，关闭重开后继续。手工记录、修订和词项检索无需模型，配置模型后可整理所选材料并生成标注。实际能力和后续工作见[当前工作](docs/CURRENT.md)和[架构](docs/ARCHITECTURE.md)。

此前的服务、SDK 与未迁入的研究材料保存在 [legacy](legacy/README.md)。现行检索计算与索引位于 [native](native/README.md)。

从源码运行需要 Python 3.14、uv、稳定版 Rust 1.97 及系统 C/C++ 编译工具链。Windows 使用 Visual Studio Build Tools 的 C++ 桌面开发组件；Linux/macOS 使用对应平台的 C++ 编译器。`uv sync --locked` 会通过 maturin 构建包含 Python 主应用、Tantivy/Jieba、USearch 和 Wave 数值内核的 `nous-server` 包。在仓库目录先设置开发环境，使缓存和虚拟环境位于 `temp/`：

```powershell
. ./scripts/workspace.ps1
```

Linux 和 macOS 使用 `source scripts/workspace.sh`。该入口同时准备 CodeGraph、Zvec-grep、项目索引和 Codex MCP；首次运行需要 Node.js/npm、uv 和 Codex CLI，Codex 需重连 MCP 或重启后读取新配置。随后安装依赖，初始化一个独立实例并检查运行资源：

检查工作区索引时运行 `. ./scripts/workspace.ps1 status` 或 `source scripts/workspace.sh status`；再次初始化会复用已有工具并同步索引。

```sh
uv sync --locked --no-dev
nous --instance ./temp/nous-local init --layout portable
nous instance register local ./temp/nous-local
nous instance use local
nous config list --level advanced
nous runtime check
nous tools prompt list
nous tools content add ./notes.md
```

工作区初始化会将虚拟环境中的 `nous` 加入当前终端的 PATH。登记并选择实例后，后续命令使用该实例；显式 `--instance` 或环境变量 `NOUS_INSTANCE` 优先。实例切换见[实例选择](docs/design/interfaces/instance-selection.md)。开发和测试时使用 `uv sync --locked` 安装开发依赖。

记录一段经历或手写一篇 Journal 后，可以直接用自然语言查询当前认识：

```sh
nous note add --text "取消等待后，后台线程仍在读文件。" --remember
nous journal create --text "资源需要持有到实际工作结束。" --title "资源释放"
nous query "任务结束后文件为什么还被占用" --mode lexical
nous read "替换为返回的完整修订标识符" --sources
```

配置写作和标注任务后，可用 `nous journalize --source "<Note标识符>" --topic "<主题>"` 生成可读草稿，再用 `journal draft read` 查看、`journal draft adopt` 明确采用；`journal annotate` 可以提出并采用概念标注。完整命令、修订、来源用途和反馈见[认知命令](docs/design/interfaces/cognition-cli.md)。

请求失败时，可用返回的操作关联执行 `nous runtime logs --operation <操作引用>`；保留日志与能力状态见[诊断指南](docs/design/interfaces/diagnostics.md)。

需要让保存后的维护持续执行时，选择后台方式，并运行待办工作：

```sh
nous journal create --file ./journal.md --background --refresh-mode lexical
nous job list
nous job run --until-idle
nous job read JOB
nous job wait JOB
```

配置好标注任务后，保存时增加 `--annotate`。`runtime run` 持续运行同一实例宿主；程序内使用 `open_instance(..., workers=True)`。取消的工作通过 `job resume JOB` 继续，ERROR 工作按 `job read` 给出的失败步骤执行 `job retry JOB --from-step STEP`。详情见[宿主与 Job](docs/design/interfaces/jobs-and-host.md)及[程序示例](docs/examples/durable-cognition/host-use.py)。

源码安装使用仓库中的 `uv sync --locked`。`uv build` 生成同一平台的完整 wheel 与源码分发包；wheel 可用 `pip install <wheel路径>` 安装，无需单独取得 Native 包。源码分发包安装时需要上述构建工具链。

按[模型配置示例](docs/examples/models.toml)在实例 `nous.toml` 中填写网关、凭据引用、模型与任务后，可以发现模型、处理资料并读取结果：

```sh
nous tools model discover gateway
nous tools model run --task summarize --input ./notes.md
nous tools media analyze ./photo.png --strategy describe_then_structure
nous tools result read "替换为返回的完整结果标识符" --sources
nous tools embedding probe text-embedding --input ./notes.md
nous tools wave build --manifest ./docs/examples/wave/collection.json --space chosen-text
nous tools wave query "替换为构建返回的完整快照标识符" "取消后文件为何仍被占用" --mode hybrid --explain
nous tools wave read "替换为构建返回的完整快照标识符" "candidate-key" --sources
```

将示例标识符替换为实际返回的完整可读标识符并保留引号，chosen-text 替换为已配置空间，candidate-key 使用实际命中键并保留引号；Embedding 空间按 probe 结果配置后使用 `tools embedding run --space <空间名> --purpose document --input ./notes.md`。音视频处理需要可从 PATH 找到的 FFmpeg 和 ffprobe，也可以通过 `media.ffmpeg_path`、`media.ffprobe_path` 指定可执行文件。详细命令和选项见[工具命令](docs/design/interfaces/tools-cli.md)、[Wave 命令](docs/design/interfaces/wave-cli.md)、[本地网关](docs/development/local-gateway.md)与[实际使用通路](docs/development/model-media-walkthrough.md)。

## 产品范围

Nous 的长期范围包括材料、来源及五轴时间，以及 Memory、Episode、Journal、Ego、Social、LanguageConvention、Motivation 和知识检索。当前正式入口是 CLI 与程序内 SDK，已接通的领域和后续建设按[当前工作](docs/CURRENT.md)区分。
