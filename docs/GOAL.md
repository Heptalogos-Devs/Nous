# Nous Wave Goal

## Objective
持续实现完整 Nous Wave：Python 主应用、同进程 Rust 原生计算、持久认知和完整公开消费路径。授权包括环境准备、新分支实现和真实模型调用。

## Decisions
- 开发分支 codex/python-native-rewrite-cloud，源分支保持不动。
- 执行 docs/python-native-plan 的架构与现行项目宪纲；CPython3.14、uv、PyO3、SQLAlchemy/SQLite、DBOS3。
- 最多三个 GPT-6.1 Sol high 子代理：native、persistence、cognition；主代理负责平台、入口与集成。
- 所有凭据只从受保护的本地文件或环境引用，temp 为实例、缓存和构建目录。

## Current Work
用户已停止云端开发，当前工作提交到 `codex/python-native-rewrite-cloud`，转回本机继续。以下已完成能力与未完成事项保持有效。

工具链、独立native与Python完整入口已接通；SDK、CLI、HTTP、MCP使用同一领域权威。真实生成、2048维文本/图像/图文联合向量已调用，历史数据已迁移可读地址。DBOS managed context与正式MaiBot实际模型输入、Steward、admission、完成反馈已贯通；Linux三wheel已在源码目录外安装运行，备份恢复与凭据撤销已实际验证。

中英管理界面已实际完成桌面/手机操作：资料与来源、精确引用、上下文、凭据撤销、逻辑包跨主体安装、Prompt预览及备份恢复。真实资料持久形成已在明确短键合同下成功完成；此前失败及付费结果继续保留。

派生媒体已接正式输入图、精确选区、历史、谱系、物理产物及桌面/手机工作台；真实三策略均形成正式表示，原件/描述/结构化与认知向量补齐后联合查询覆盖完整。原生逻辑包携带派生输入图、内容、可读绑定与实际向量空间，并已跨主体安装读取。

配置的 video_url 扩展已通过官方 SDK 对实际视频完成解释；WAV 也已真实理解并保存表示，通道记录分别反映原视频/原音频和未输入模态。

Self/Social 正式操作已接 SDK。成长服务已用纯 LocalRef 提案生成 Schema/Concept、原子采纳并查询；工作台采纳、拒绝、定时计划及手机中英操作均已实际完成。模型调用按持久 invocation 固定付费请求，重启复用成功结果，未知结果不自动重付。

下一项：完整查询的机器表达式、过滤、候选预算、能力策略、来源展开与重排；外部 A-Memorix 包的图片语义关系、向量与资产选取；迁移完成后的旧实现清理及最终发行安装。

先前提交 `93eb347` 的发行构建40756320214已在 Linux manylinux2_28、Windows x64、macOS ARM 与 macOS Intel 全部通过：分别构建 native wheel、安装完整三个wheel、从源码目录外初始化实例并启动HTTP。

## 本机接续
```bash
git fetch origin
git switch --track origin/codex/python-native-rewrite-cloud
source scripts/dev-env.sh
uv sync --locked
```
若本地已存在该分支，直接切换并执行 `git pull --ff-only`。开发依赖、启动与构建命令见 README 和 `docs/python-native-install.md`；使用独立 `native/` 工作区，保留旧 Rust 实现供剩余能力迁移参考。

本机需自行配置凭据文件；云端 Key、访问令牌、运行实例、付费调用历史和构建缓存未提交 Git。此前实际验证使用 OpenAI 兼容地址 `https://ark.cn-beijing.volces.com/api/plan/v3`，对应生成模型 `doubao-seed-2.0-mini`、向量模型 `doubao-embedding-vision`，每批向量上限为 10。连接仍通过通用模型资源或 README 中的环境变量配置。

## Blockers
云端开发使用仓库授权资料建立了独立实例，本机已有实例与其付费历史未迁入云端。

Superdesign CLI尚未认证，需要交互登录；不阻断本地管理界面实现。
