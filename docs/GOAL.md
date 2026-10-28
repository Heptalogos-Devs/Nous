# Nous Wave Goal

## Objective
持续实现完整 Nous Wave：Python 主应用、同进程 Rust 原生计算、持久认知和完整公开消费路径。授权包括环境准备、新分支实现和真实模型调用。

## Decisions
- 开发分支 codex/python-native-rewrite-cloud，源分支保持不动。
- 执行 docs/python-native-plan 的架构与现行项目宪纲；CPython3.14、uv、PyO3、SQLAlchemy/SQLite、DBOS3。
- 最多三个 GPT-6.1 Sol high 子代理：native、persistence、cognition；主代理负责平台、入口与集成。
- 所有凭据只从受保护的本地文件或环境引用，temp 为实例、缓存和构建目录。

## Current Work
工具链、独立native与Python完整入口已接通；SDK、CLI、HTTP、MCP使用同一领域权威。真实生成、2048维文本/图像/图文联合向量已调用，历史数据已迁移可读地址。DBOS managed context与正式MaiBot实际模型输入、Steward、admission、完成反馈已贯通；Linux三wheel已在源码目录外安装运行，备份恢复与凭据撤销已实际验证。

中英管理界面已实际完成桌面/手机操作：资料与来源、精确引用、上下文、凭据撤销、逻辑包跨主体安装、Prompt预览及备份恢复。真实资料持久形成已在明确短键合同下成功完成；此前失败及付费结果继续保留。

继续完成派生媒体谱系、Self/Social与长期形成、完整公开消费者及跨平台发行验证。不得将当前已通过的链路等同于完整产品目标完成。

## Blockers
笔记本实例与其付费历史未提供；使用仓库授权资料建立云端新实例。Windows/macOS已配置发行runner，尚需在对应系统实际执行。

Superdesign CLI尚未认证，需要交互登录；不阻断本地管理界面实现。
