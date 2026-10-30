# Nous Wave：Python 主应用与 Rust 原生计算

采用 Python 作为 Nous 的应用语言，现有 Rust Native 承担检索计算、原生索引和 NousQL 语法解析；后续性能热点可按实际收益采用合适的语言或成熟库。两个语言在同一服务进程内通过 PyO3 扩展协作。Python 拥有认知规则、权威数据库、业务事务、模型调用和持久作业；Rust 接受明确的计算输入并返回结果。

开发范围、实施依赖与当前工作统一见[开发目标与计划](../GOAL.md)，本目录维护相应技术设计。

## 已定技术选择

| 范围            | 选择                                                                   |
| --------------- | ---------------------------------------------------------------------- |
| 应用语言        | CPython 3.14 标准构建；保留现有 SDK 的 Python 3.11+ 客户端范围         |
| 环境与依赖      | uv workspace、一个 `uv.lock`、Hatchling 构建 Python 包                 |
| 类型与编辑支持  | Basedpyright strict、对应 Language Server、显式 `.pyi` 与 `py.typed`   |
| 格式与静态规范  | Ruff formatter、lint、import sorting，使用同一份 pyproject 配置        |
| 原生扩展        | PyO3 + maturin + NumPy；Rust 工具链沿用已能构建现有算法的版本          |
| 权威存储        | SQLite + SQLAlchemy 2 + aiosqlite；完整用例控制唯一提交                |
| 持久执行        | DBOS Python 3 稳定版；SQLite 同文件事务内入队                          |
| 模型            | Pydantic AI slim 的原生 Provider；特有媒体向量协议使用官方 SDK         |
| 网络            | HTTPX 家族客户端按所选 Provider SDK 的实际类型注入，统一配置和生命周期 |
| API / CLI / MCP | FastAPI、Typer + Rich、官方 MCP Python SDK                             |
| Prompt / 配置   | Jinja2、Pydantic、pydantic-settings、TOML、现有角色与参数体系          |
| 观测            | 标准 logging + structlog、OpenTelemetry、现有调用账本与费用语义        |
| 发行            | Python 安装包 + 各目标平台 native wheel；终端入口仍为 `nous`           |

只增加三个 Python 发行单元：主应用 `nous-wave-server`、已有 SDK `nous-wave`、原生模块 `nous-wave-native`。其余职责是同一应用内部的 Python 模块。Heptalogos 继续是 MaiBot 派生产品和 Nous 的正式消费者，其现行主线直接继承 MaiBot，主体内核及 Nous 连接仍待开发。

## 阅读顺序

- [工具链与开发环境](design/toolchain.md)：版本、强类型、LSP、构建缓存、具体命令。
- [语言、模块与所有权](design/architecture.md)：目录结构、三种发行单元、内部契约与跨模块操作。
- [事务、作业与索引](design/transactions.md)：SQLite、DBOS、原子接纳、任务取消、模型结果与索引。
- [原生边界](design/native-boundary.md)：VCP、Tantivy、USearch、NousQL、数组、GIL、线程和类型存根。
- [公共基础能力](design/platform.md)：配置、Prompt、模型、权限、时间、媒体、观测和管理。
- [能力与源码研究](design/adoption.md)：当前实现及 A-Memorix 消费者的后续方向。
- [数据边界](design/migration.md)：真实数据和业务身份。
- [云端开发环境](design/cloud.md)：准备环境、网络、凭据与数据。
- [开发目标与计划](../GOAL.md)：完整功能范围和当前工作。
- [重构进展](progress-report.md)：已运行的路径、当前施工与剩余能力。
- [来源](SOURCES.md)：已读取的仓库路径与官方资料。

## 开发与安装

实际依赖、锁文件、发行单元和构建配置位于仓库根目录及 `native/`、`sdk/python/`。开发环境和命令见[项目 README](../../README.md)与[安装说明](../python-native-install.md)；不要用计划模板覆盖现有配置。

当前 `ModelService` 尚需注入 `RoleRegistry`，源码组装完成后才能重新启动；状态见[重构进展](progress-report.md)。工作台源码暂时保留，管理以 API / CLI 为主要入口。
