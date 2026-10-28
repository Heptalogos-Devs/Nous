# Nous Wave 云端重写目标

从 `codex/context-infrastructure-sdk` 的暂停交接继续。本次起点为 `2458932646d6b2968db38d1e0a97103f93cd2cad`，先完整阅读根目录 `ses-nous-wave-imports-202610290007.md` 与本包README、对应设计。只在新的 `codex/python-native-rewrite` 或Codex为本任务建立的隔离分支工作，保留原分支及真实实例数据，不自动合并。

将Nous重组为 **Python主应用 + 同进程Rust原生计算**。Python拥有认知规则、SQLite权威事务、配置、Prompt、模型与网络、后台工作、CLI/HTTP/MCP和管理；Rust保留VCP、Tantivy中文检索、USearch及现有NousQL纯语法。使用PyO3/maturin，批量数组和owned snapshot，计算释放GIL并在线程执行。原生模块不访问认知数据库、模型或用户凭据。

使用CPython3.14标准构建、uv workspace、Basedpyright strict、Ruff及对应LSP。保留现有Python SDK的3.11+范围，提供完整`.pyi`和`py.typed`。采用SQLAlchemy2/SQLite与DBOS Python3，在同一SQLite文件、同一业务事务内用公开`enqueue_in_transaction`完成领域结果与后续任务接纳。Provider及形成优先采用Pydantic AI/官方SDK，模板用Jinja2，协议用FastAPI/Typer/官方MCP。所有具体库按包内决定及实际安装API接线，提交lock。

先保留全部旧源码供阅读，新建独立native workspace与Python入口；完整迁移一个owner及其消费者之后，删除新分支里已被替代的Rust实现和专属支撑。不要一次清空仓库，不长期运行两套认知权威，不逐行翻译Rust、不让Python代理完整旧Kernel。A-Memorix、MaiBot及旧Nous已有能力优先复制改编到真正负责的模块，统一状态、事务与公开行为。

完整承接宪纲、当前功能与续接清单：A-Memorix全部实际工具/管理/协议，完整NousQL与14类返回域、VCP全体系与Nous扩展、材料和来源、五轴与历史、Memory及结构化认知、Self/Social/Language/Motivation、Active/Resident/Work/Managed Context与Steward、持续形成维护、三种媒体解释路径、原生视觉/描述/联合检索、Prompt配置、权限、诊断、导入与Bundle、备份恢复和跨平台安装。保留文件Catalog/Unit/Output/成功事实与任务的区别；交接WIP按真实行为完善，不当作已验证实现。

按迁移文贯通真实入口：初始化→接纳资料→真实形成→embedding→查询→来源→重启继续；再完成导入/来源/Bundle、完整认知与外部消费者、产品管理和发行。用已提供且云端可达的模型与授权真实资料进行Codex/MaiBot dogfooding。笔记本网关和temp数据不在云端，缺连接时明确具体阻塞，继续独立可完成工作，不伪造模型结果或真实测试资料。

云端已有新AntiRLHF时读取本地技能；缺失时按本包脚本下载官方技能包并运行其安装器，保留既有指令。执行当前Constitution及相关system-architecture、dependency-selection、structural-refactoring、persistence-transactions等专项方法。修正定义者与全部消费者，采用成熟机制，保护实际语义与用户路径。验证集中于独立故障、重要数值与真实使用，不扩张浅层测试、流程脚手架和证明性报告。LSP与类型检查服务编辑，Rust只构建计算workspace。

子代理按独立owner划分且共享明确合同；主代理完成真实集成。提交保存有用进展，持续保留Objective、有效决定和未完成能力。交付可安装Python产品、native构建与wheel、完整功能及当前文档；准确说明实际运行环境与真实阻塞，不以局部例程或示例可运行作为全产品完成。
