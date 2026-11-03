# 设计来源

## 当前项目材料

当前源码基点与交接共同决定迁移范围：

- [Nous暂停分支](https://github.com/Heptalogos-Devs/Nous-Wave/tree/2458932646d6b2968db38d1e0a97103f93cd2cad)
- [本次读取的交接文件](https://github.com/Heptalogos-Devs/Nous-Wave/blob/2458932646d6b2968db38d1e0a97103f93cd2cad/ses-nous-wave-imports-202610290007.md)
- [项目宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)
- [Architecture-Vault宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)
- [A-Memorix真实合同](https://github.com/Heptalogos-Devs/Nous-Wave/blob/2458932646d6b2968db38d1e0a97103f93cd2cad/research/a-memorix-contracts.json)
- [现有SDK](https://github.com/Heptalogos-Devs/Nous-Wave/tree/2458932646d6b2968db38d1e0a97103f93cd2cad/sdk/python)
- [现有NousQL](https://github.com/Heptalogos-Devs/Nous-Wave/tree/2458932646d6b2968db38d1e0a97103f93cd2cad/crates/nous-domain/src/nousql)
- [现有VCP](https://github.com/Heptalogos-Devs/Nous-Wave/tree/2458932646d6b2968db38d1e0a97103f93cd2cad/crates/nous-vcp)

Python 语言边界、SQLAlchemy/DBOS 同库方案和模块结构见本目录的设计文档；当前目标和工作见[开发目标与计划](../GOAL.md)。

## Python、静态工具与原生构建

- [Python维护状态](https://devguide.python.org/versions/)
- [uv workspace](https://docs.astral.sh/uv/concepts/projects/workspaces/)
- [uv依赖与环境](https://docs.astral.sh/uv/concepts/projects/config/)
- [Basedpyright CLI与Language Server安装](https://docs.basedpyright.com/latest/installation/command-line-and-language-server/)
- [Basedpyright配置](https://docs.basedpyright.com/latest/configuration/config-files/)
- [Ruff编辑器与原生Server](https://docs.astral.sh/ruff/editors/setup/)
- [PyO3并行与detach](https://pyo3.rs/main/parallelism)
- [PyO3性能与边界](https://pyo3.rs/main/performance.html)
- [maturin](https://www.maturin.rs/)
- [maturin本地开发](https://www.maturin.rs/local_development)

## 事务、任务与模型

- [DBOS数据库连接](https://docs.dbos.dev/python/tutorials/database-connection)
- [DBOS客户端事务内入队](https://docs.dbos.dev/python/reference/client#enqueue_in_transaction)
- [DBOS 3升级后的正式API](https://docs.dbos.dev/python/upgrading)
- [DBOS配置](https://docs.dbos.dev/python/reference/configuration)
- [DBOS 3.3.0客户端源码](https://github.com/dbos-inc/dbos-transact-py/blob/3.3.0/dbos/_client.py)
- [SQLAlchemy SQLite事务说明](https://docs.sqlalchemy.org/en/20/dialects/sqlite.html)
- [Pydantic AI Provider与client生命周期](https://pydantic.dev/docs/ai/models/overview/)
- [Pydantic AI精简安装与Provider extras](https://pydantic.dev/docs/ai/overview/install/)
- [Pydantic校验](https://docs.pydantic.dev/latest/concepts/strict_mode/)
- [A-Memorix上游](https://github.com/A-Dawn/A_memorix)
- [MaiBot上游](https://github.com/Mai-with-u/MaiBot)

DBOS同库事务方案使用现有公开接口。它要求业务连接与DBOS系统状态在同一数据库；SQLite适用于本轮单机实例部署。模型SDK有不同HTTP客户端类型，按解析到的版本使用其正式构造接口。

## 云端环境与治理

- [Codex云端环境](https://learn.chatgpt.com/docs/environments/cloud-environments)
- [Codex Cloud使用](https://help.openai.com/en/articles/20001545-using-codex-cloud)
- [云端安全与凭据](https://learn.chatgpt.com/docs/enterprise/security-cookbook)
- [AntiRLHF当前README](https://github.com/AntiRLHF-Research/AntiRLHF/blob/main/README.md)
- [AntiRLHF当前宪章](https://github.com/AntiRLHF-Research/AntiRLHF/blob/main/CONSTITUTION.zh-CN.md)
- [专项技能目录](https://github.com/AntiRLHF-Research/AntiRLHF/blob/main/CONTENTS.zh-CN.md)
- [当前安装器](https://github.com/AntiRLHF-Research/AntiRLHF/blob/main/tools/install.py)
- [研究正文当前位置](https://github.com/AntiRLHF-Research/AntiRLHF/blob/main/docs/charter/Revised.md)

云端环境的脚本字段和secret机制以实际界面为准；本包区分当前网络secret与旧setup-only secret。源码包、运行连接和真实资料分别取得，不假定Git提交包含本机temp或凭据。
