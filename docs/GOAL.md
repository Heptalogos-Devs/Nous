# Nous Wave Goal

## Objective
持续实现完整 Nous Wave：Python 主应用、同进程 Rust 原生计算、持久认知和完整公开消费路径。授权包括环境准备、新分支实现和真实模型调用。

## Decisions
- 开发分支 codex/python-native-rewrite-cloud，源分支保持不动。
- 执行 docs/python-native-plan 的架构与现行项目宪纲；CPython3.14、uv、PyO3、SQLAlchemy/SQLite、DBOS3。
- 最多三个 GPT-6.1 Sol high 子代理：native、persistence、cognition；主代理负责平台、入口与集成。
- 所有凭据只从受保护的本地文件或环境引用，temp 为实例、缓存和构建目录。

## Current Work
已安装CPython3.14.8/Rust1.97.1、uv/LSP与完整依赖锁；已构建真实native，事务/DBOS、领域链已独立运行。正在集成HTTP/CLI/MCP与真实模型形成、2048维向量、hybrid查询和重启；继续完整认知、上下文、管理、外部消费者与发行。

## Blockers
OpenAI兼容连接 https://ark.cn-beijing.volces.com/api/plan/v3 已实际完成生成与2048维embedding。订阅接口可大量用于真实功能。笔记本实例与其付费历史未提供；使用仓库授权资料建立云端新实例。Windows/macOS需相应runner。

Superdesign CLI尚未认证，需要交互登录；不阻断本地管理界面实现。
