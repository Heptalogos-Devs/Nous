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

当前本地 CLI 提供实例、配置、运行诊断和多轴时间操作。运行基础不要求配置模型或遥测服务；内容、持久工作、模型执行、Wave、认知对象以及 HTTP/MCP/远程 SDK 将按后续计划接入当前运行时。实际施工范围见[当前工作](docs/CURRENT.md)和[架构](docs/ARCHITECTURE.md)。

此前的服务、SDK、Native 实现与测试保存在 [legacy](legacy/README.md)，供后续研究和迁移。

从源码运行需要 Python 3.14 和 uv。在仓库目录先设置开发环境，使缓存和虚拟环境位于 `temp/`：

```powershell
. ./scripts/dev-env.ps1
```

Linux 和 macOS 使用 `source scripts/dev-env.sh`。随后安装依赖，初始化一个独立实例并检查运行资源：

```sh
uv sync --locked --no-dev
uv run --no-dev nous --instance ./temp/nous-dev init --layout portable
uv run --no-dev nous --instance ./temp/nous-dev config list --level advanced
uv run --no-dev nous --instance ./temp/nous-dev config set logging.level DEBUG
uv run --no-dev nous --instance ./temp/nous-dev runtime check
```

开发和测试时使用 `uv sync --locked` 安装开发依赖。

## 产品范围

Nous 支持材料、来源及五轴时间，Memory、Episode、Journal、Ego、Social、LanguageConvention、Motivation 和知识检索。A-Memorix 已有的检索、画像、Episode、导入、图谱、维护、纠错、Bundle、图片及管理接口是需要完整覆盖的功能基线；CLI、HTTP、MCP 和 Python SDK 是正式调用入口。
