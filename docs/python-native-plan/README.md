# Nous Wave：Python 主应用与 Rust 原生计算

采用 Python 作为 Nous 的应用语言，Rust 保留成熟的检索计算、原生索引和 NousQL 语法解析。两个语言在同一服务进程内通过 PyO3 扩展协作。Python 拥有认知规则、权威数据库、业务事务、模型调用和持久作业；Rust 接受明确的计算输入并返回结果。

## 当前工作起点

从 `Heptalogos-Devs/Nous-Wave` 的 `codex/context-infrastructure-sdk` 分支创建 `codex/python-native-rewrite`。本次读取的交接提交为 `2458932646d6b2968db38d1e0a97103f93cd2cad`；交接文件是仓库根目录的 `ses-nous-wave-imports-202610290007.md`。

原分支保持原样。新分支首先保留全部源码、现行需求、研究、SDK 和集成材料，按完整能力逐块迁移。旧实现作为阅读和行为参考，不进入 Python 服务的默认运行链。已经迁完的代码在新分支成组退出，原分支继续保存全部历史。

交接中最后一批文件接纳、文件去重、Source 生命周期与 Native Bundle 改动仍是 WIP，存在未编译和未实际运行部分；本地开发实例还保有真实材料、付费结果与历史。云端从新的实例开始工作，使用真实可访问资料，不连接或修改笔记本上的原数据库。

## 已定技术选择

| 范围 | 选择 |
|---|---|
| 应用语言 | CPython 3.14 标准构建；保留现有 SDK 的 Python 3.11+ 客户端范围 |
| 环境与依赖 | uv workspace、一个 `uv.lock`、Hatchling 构建 Python 包 |
| 类型与编辑支持 | Basedpyright strict、对应 Language Server、显式 `.pyi` 与 `py.typed` |
| 格式与静态规范 | Ruff formatter、lint、import sorting，使用同一份 pyproject 配置 |
| 原生扩展 | PyO3 + maturin + NumPy；Rust 工具链沿用已能构建现有算法的版本 |
| 权威存储 | SQLite + SQLAlchemy 2 + aiosqlite；完整用例控制唯一提交 |
| 持久执行 | DBOS Python 3 稳定版；SQLite 同文件事务内入队 |
| 模型 | Pydantic AI slim 的原生 Provider；特有媒体向量协议使用官方 SDK |
| 网络 | HTTPX 家族客户端按所选 Provider SDK 的实际类型注入，统一配置和生命周期 |
| API / CLI / MCP | FastAPI、Typer + Rich、官方 MCP Python SDK |
| Prompt / 配置 | Jinja2、Pydantic、pydantic-settings、TOML、现有角色与参数体系 |
| 观测 | 标准 logging + structlog、OpenTelemetry、现有调用账本与费用语义 |
| 发行 | Python 安装包 + 各目标平台 native wheel；终端入口仍为 `nous` |

只增加三个 Python 发行单元：主应用 `nous-wave-server`、已有 SDK `nous-wave`、原生模块 `nous-wave-native`。其余职责是同一应用内部的 Python 模块。Heptalogos 继续是 MaiBot 派生产品和 Nous 的正式消费者，本轮不重写 Hept。

## 阅读顺序

- [工具链与开发环境](design/toolchain.md)：版本、强类型、LSP、构建缓存、具体命令。
- [语言、模块与所有权](design/architecture.md)：目录结构、三种发行单元、内部契约与跨模块操作。
- [事务、作业与索引](design/transactions.md)：SQLite、DBOS、原子接纳、任务取消、模型结果与索引。
- [原生边界](design/native-boundary.md)：VCP、Tantivy、USearch、NousQL、数组、GIL、线程和类型存根。
- [公共基础能力](design/platform.md)：配置、Prompt、模型、权限、时间、媒体、观测和管理。
- [功能续接与源码复用](design/adoption.md)：当前 Rust、Python SDK、A-Memorix、MaiBot 的实际采用方式。
- [迁移与交付顺序](design/migration.md)：分支保护、WIP 处理、逐块替换、实际使用与数据保护。
- [云端 Codex 环境](design/cloud.md)：准备环境、网络与凭据、AntiRLHF、夜间任务和完成范围。
- [云端任务 Goal](GOAL.md)：直接交给执行代理的目标。
- [来源](SOURCES.md)：已读取的仓库路径与官方资料。

## 随包配置与脚本

`templates/` 提供目标 pyproject、编辑器配置、原生包配置、Cargo profile 与类型存根。它们供新分支合并，不能整目录覆盖现有仓库。`scripts/cloud-prepare.sh` 只准备环境和缺失的 AntiRLHF；`scripts/dev-env.sh` 只设置本次终端的缓存路径。初次解析依赖、生成 lock、构建真实原生扩展在云端实施。

本包交付的是架构与可执行施工指导；没有修改远端仓库、创建云端任务或安装依赖到用户电脑。配置文件已作本地语法检查，依赖安装、native 编译、模型调用与目标平台运行由实际云端环境完成。
