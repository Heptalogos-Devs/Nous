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

继续完成派生媒体谱系、Self/Social与长期形成、全部用户管理路径和双语界面。真实资料形成中发现未定义模型短键，已保留原付费结果与失败事实，加入明确短键合同和有预算的纠正尝试；正在验证新的持久任务与重启续接。

## Blockers
笔记本实例与其付费历史未提供；使用仓库授权资料建立云端新实例。Windows/macOS已配置发行runner，尚需在对应系统实际执行。

Superdesign CLI尚未认证，需要交互登录；不阻断本地管理界面实现。
