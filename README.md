# Nous

Nous 是独立安装的长期认知服务，为智能体提供记忆、知识、来源、时间、检索与认知成长，也是 Heptalogos 的认知和人格中心。Python 主应用负责领域、模型、授权、事务与 CLI、HTTP、MCP、Python SDK；必要的 Native 计算承担 VCP、NousQL 等性能热点。

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

当前本地 CLI 提供实例、配置、运行诊断、多轴时间，以及模型、Prompt、内容、媒体和 Wave 检索工具。原件与派生结果保存在本地，可按短地址读取来源；配置兼容网关后可执行文字、结构化和媒体任务及文本 Embedding。Wave 可构建和重开检索快照，运行词项、Dense、TagMemo、RiverMemo、混合与 DTSC 查询，并读取原文、来源和 Thinking 路线。实例初始化、内容读取与纯词项检索不要求配置模型或遥测服务。实际能力和后续工作见[当前工作](docs/CURRENT.md)和[架构](docs/ARCHITECTURE.md)。

此前的服务、SDK 与未迁入的研究材料保存在 [legacy](legacy/README.md)。现行检索计算与索引位于 [native](native/README.md)。

从源码运行需要 Python 3.14、uv、稳定版 Rust 1.97 及系统 C/C++ 编译工具链。Windows 使用 Visual Studio Build Tools 的 C++ 桌面开发组件；Linux/macOS 使用对应平台的 C++ 编译器。`uv sync --locked` 会通过 maturin 构建包含 Python 主应用、Tantivy/Jieba、USearch 和 Wave 数值内核的 `nous-server` 包。在仓库目录先设置开发环境，使缓存和虚拟环境位于 `temp/`：

```powershell
. ./scripts/dev-env.ps1
```

Linux 和 macOS 使用 `source scripts/dev-env.sh`。随后安装依赖，初始化一个独立实例并检查运行资源：

```sh
uv sync --locked --no-dev
uv run --no-dev nous --instance ./temp/nous-local init --layout portable
uv run --no-dev nous --instance ./temp/nous-local config list --level advanced
uv run --no-dev nous --instance ./temp/nous-local runtime check
uv run --no-dev nous --instance ./temp/nous-local tools prompt list
uv run --no-dev nous --instance ./temp/nous-local tools content add ./notes.md
```

开发和测试时使用 `uv sync --locked` 安装开发依赖。

源码安装使用仓库中的 `uv sync --locked`。`uv build` 生成同一平台的完整 wheel 与源码分发包；wheel 可用 `pip install <wheel路径>` 安装，无需单独取得 Native 包。源码分发包安装时需要上述构建工具链。

按[模型配置示例](docs/examples/models.toml)在实例 `nous.toml` 中填写网关、凭据引用、模型与任务后，可以发现模型、处理资料并读取结果：

```sh
uv run --no-dev nous --instance ./temp/nous-local tools model discover gateway
uv run --no-dev nous --instance ./temp/nous-local tools model run --task summarize --input ./notes.md
uv run --no-dev nous --instance ./temp/nous-local tools media analyze ./photo.png --strategy describe_then_structure
uv run --no-dev nous --instance ./temp/nous-local tools result read 'res:替换为返回的地址' --sources
uv run --no-dev nous --instance ./temp/nous-local tools embedding probe text-embedding --input ./notes.md
uv run --no-dev nous --instance ./temp/nous-local tools wave build --manifest ./docs/examples/wave/collection.json --space chosen-text
uv run --no-dev nous --instance ./temp/nous-local tools wave query 'wav:替换为构建返回的地址' "取消后文件为何仍被占用" --mode hybrid --explain
uv run --no-dev nous --instance ./temp/nous-local tools wave read 'wav:替换为构建返回的地址' candidate-key --sources
```

将示例地址替换为实际返回的短地址，chosen-text 替换为已配置空间，candidate-key 使用实际命中键；Embedding 空间按 probe 结果配置后使用 `tools embedding run --space <空间名> --purpose document --input ./notes.md`。音视频处理需要可从 PATH 找到的 FFmpeg 和 ffprobe，也可以通过 `media.ffmpeg_path`、`media.ffprobe_path` 指定可执行文件。详细命令和选项见[工具命令](docs/design/interfaces/tools-cli.md)、[Wave 命令](docs/design/interfaces/wave-cli.md)、[本地网关](docs/development/local-gateway.md)与[实际使用通路](docs/development/model-media-walkthrough.md)。

## 产品范围

Nous 支持材料、来源及五轴时间，Memory、Episode、Journal、Ego、Social、LanguageConvention、Motivation 和知识检索。A-Memorix 已有的检索、画像、Episode、导入、图谱、维护、纠错、Bundle、图片及管理接口是需要完整覆盖的功能基线；CLI、HTTP、MCP 和 Python SDK 是正式调用入口。
