# Nous 开发指引

本文件适用于整个仓库。修改局部内容时，应同时遵循对应目录的 AGENTS.md，如 `src/nous/` 的 [AGENTS.md](src/nous/AGENTS.md)； 任何 Agent 不得在不经过开发者明确许可、要求的情况下修改、移动或删除 AGENTS.md 等此类用于治理 Agent 的文件； AGENTS 文件变动规范：在根目录下创建 Git Patch `AGENTS.proposal.patch` 文件，并及时提醒开发者审阅。不必为提议停下其他不相关工作，Patch 应用后自行删除。

## AntiRLHF

开发前必读 [AntiRLHF Charter](docs/agents/antirlhf/CHARTER.md)、[CONSTITUTION](docs/agents/antirlhf/CONSTITUTION.zh-CN.md)，尤其是 GPT 模型。AntiRLHF 是项目所有者自研的模型治理方案，针对局部优化、过度工程、过度测试、无意义兼容，以及用流程代替工程判断的行为。

将这些要求落实到实际选择：找到问题的责任归属，完成用户需要的使用通路，修改不合适的结构，并清除被替代的实现。不要用额外报告、检查清单或反复声明遵循规范来代替工作。

## 项目与当前工作

Nous 是独立安装的长期认知服务，面向记忆、知识、来源、时间、检索与认知成长。Python 主应用统一领域操作和正式入口，必要的 Rust PyO3 Native 承担性能热点。

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

- 项目仍处原型阶段，暂保持 `v0.1.0`，没有外部使用者，也没有内部兼容或内部版本递增需求。结构、数据形态或框架不合适时，直接替换负责实现，迁移实际生产者、消费者、配置、测试和文档，删除被替代的代码及专属支撑。内部实现不得新增 `v1`／`v2` 等递增标识、版本链、兼容分支或双轨路径，也不为淘汰的内部结构建立旧格式保留或备份迁移机制。用户文稿的业务修订与精确历史引用继续按产品合同保存。
- 禁止为保住错误结构做局部绕过、包装和重复实现。先判断一个行为、数据或状态应由谁负责，再选择实现位置。现有模块可复用，也可因真实需求重新划分职责、重构。
- 优先采用已选成熟依赖的正式能力。不要自行重写协议客户端、媒体解析、数据库机制等已有能力，也不要为每个概念机械建立 service、repository、manager 或转发层。
- 保持供应商中立。LLM 服务经统一网关访问，New API 是可用网关之一；生产代码不为某个外部平台增加特殊业务。按明确协议、能力和任务配置工作，不根据模型名猜测能力。
- 完成一项能力时，同时处理实际生产者、消费者、配置和用户入口。不要留下只在内部方法或演示代码中成立的功能。

## 验证

禁止 TDD 和红绿循环。不得添加浅层测试、字段存在性测试，或重复验证依赖库已经负责的行为。除确有保护价值的关键契约、安全细节和独立故障外，优先通过真实用户入口验证完整通路（端到端）；验证错误输入和失败行为不属于为了红绿循环制造失败。

选择能暴露当前改动问题的验证方式。检查功能结果和失败行为，不以提交数、测试数或测试全绿代替产品判断。说明实际执行的验证和结果，区分模拟与真实外部服务，以及未验证的范围；不将开发报告或远端 CI 当成实际运行的证据。旧测试只保护旧实现时，随旧实现一起删除；仍有价值的使用情境迁到新入口。不要为单次测试给生产代码增加专用导出口、平行实现或搭建额外测试框架。

开发脚手架、临时探针、实验性测试和脚本放在 `temp/`，不得进入 Git。已有正式测试位于 `tests/`。

## 维护文档

设计、公共契约或职责改变时，原地更新负责专题和源码索引，移除失效说明。同一规则只在一个负责位置定义，其它地方引用，不复制多套版本。

重要类和方法用注释说明职责、状态边界或不明显的不变量，不逐行复述实现；详见 [docstring-std](docs/agents/docstring-std.md)。

`CURRENT.md` 记录当前能力、有效决定和剩余工作，不写成操作日志。不要把每次纠错写成永久禁令，也不要将退役方案、实现过程或自我评审说明留在产品文档、代码注释和用户界面中。

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

使用 [temp 管理工具](scripts/temp/README.md) 建立目录、查看状态和清理。

## 代码与参考项目检索

### Nous 源码

定位代码、理解符号和追踪调用关系时，先用 CodeGraph MCP 的 `codegraph_explore`，查询中注明文件、符号或具体问题。也可在项目目录执行：

```sh
codegraph explore "<符号或问题>"
```

CodeGraph 或索引不可用时，使用文件和符号检索继续定位； 也可选择安装或修复该工具，但需向开发者说明并请求许可。沿命中位置阅读负责模块和真实调用方，不凭搜索摘要决定修改。

### 参考实现

检索参考实现时阅读 [参考项目检索](docs/agents/reference-code.md)。
