# 分支迁移与完整交付

## 保护暂停分支

源分支：`codex/context-infrastructure-sdk`。本次交接提交：`2458932646d6b2968db38d1e0a97103f93cd2cad`。

云端先读取源分支交接与当前HEAD，再从这个提交建立自己的工作分支。可使用 `codex/python-native-rewrite`；若Codex已为本任务建立隔离分支，沿用它并确认基点是这份交接。不要在源分支提交，不reset源分支，不force push，不自动合并。

在新分支起步时，全部原源码继续保留。建立独立native workspace与Python根工程，默认开发命令只进入它们。原Rust应用暂作为阅读材料，不继续添加新功能。后续每迁完一个实际owner，连同其消费者和文档一起更新，再删除该owner被替代的Rust实现与专属支撑。

为方便持续参考，可按需建立detached只读工作树，避免每次从已删除文件回溯：

```bash
git fetch origin codex/context-infrastructure-sdk
git switch -c codex/python-native-rewrite 2458932646d6b2968db38d1e0a97103f93cd2cad
# 需要独立参考目录时再执行，目录保持未跟踪。
git worktree add --detach temp/reference/nous-rust 2458932646d6b2968db38d1e0a97103f93cd2cad
```

上述命令供实际云端工作树使用；当前目录已经有未提交内容或已经处于Codex隔离分支时，直接保护并沿用，不重复建分支或覆盖工作。原源码不在同一个服务运行时与Python双写。

## 执行依赖与可用结果

### 建立Python入口并接入原生计算

先安装工具链、生成uv.lock、建typed应用包、复制现有SDK并补齐类型合同；提取VCP/检索和NousQL语法到nativeworkspace，构建第一份真实扩展。完成Python `nous --help`、实例初始化、启动、状态、HTTP/MCP会话；本步骤不要求旧Rust应用全工作区构建。

这时要尽早调用实际native算法与查询parser，防止把语言边界留到最后。采用现有真实研究输入和用户已授权资料，读取算法结果与解析结果，而非只检查module可import。

### 完成一个可持续使用的认知链

统一实例、数据库、配置、权限、网络、Prompt、模型与DBOS接线。完成：创建Namespace/Subject→上传实际资料→保全来源→接触或导入→真实模型形成→采纳→实际embedding→中文/语义/VCP查询→展开来源→重启后继续。

链路中的每项接口都由正式owner提供。模型不可达时继续完成独立接线、类型检查和已有真实资料的确定性处理；将真实调用标为具体阻塞，不制造模型输出或模拟成功交付。

### 迁移正在进行的导入、文件、来源和Bundle

承接暂停分支的InputSource/UnitCatalog/UnitOutput/FileSuccess及去重规则，把Source预览/执行/恢复、A-Memorix导入管理、文件传输与Native Bundle真正接到同一Python事务。按当前真实类型解释历史未完成部分，改清楚源头数据结构。

处理本地真实数据需要用户实际提供副本或导出包；云端缺少这些文件不妨碍新的实例开发。云端不能运行交接中的Windows一次性schema刷新脚本，也不能假定temp目录已上传。

### 完成认知、查询和长期工作

将当前结构化认知、完整NousQL、14类返回域、VCP生产图资产、认知使用反馈、Active/Resident/WorkContext、Steward、高级Self/Social/Motivation与持续形成完整接入。读取既有能力清单的真实消费者，按职责组合迁移，不逐行翻译Rust。

概念与关系图服务资产从正式权威关系导出，不能永久停留在原件标签共现图。原生图计算与Python权限/来源保持一次明确的交接。

### 完成外部消费者与产品管理

迁移现有PythonSDK和MaiBot补丁，真正使用A-Memorix工具/管理页面完成查询、来源、画像、图谱、Episode、维护、Bundle、调优与导入删除恢复。维护公开协议的实际行为，删除内部Rust服务的过渡调用。

完整CLI/MCP/HTTP/SDK、模型配置、Prompt编辑、诊断、i18n、管理视图、扩展、备份恢复、Subject分支、跨平台安装均保持产品目标。已存在的成熟页面和操作优先采用；新的前端仅消费正式HTTP。

### 退出旧运行结构并发行

当Python正式入口覆盖对应功能后，删除新分支里被替代的Rust app/foundation/storage/CLI及仅支撑它们的生成器、脚本和测试；保留native算法和必要科学样例。更新README、构建与启动命令。源分支永久保留当前交接状态，避免为参考目的在新产品中长期携带可运行的旧Kernel。

安装包依赖native wheel，无须本机Rust；CPU密集内容继续原生运行。完成当前云端Linux实际安装与dogfooding，其他系统按对应环境执行，明确尚未运行的平台。

## 实际验证与AI友好开发

Basedpyright和Ruff服务于日常编辑与跨模块合同更改，native只编译受影响workspace。重要结构合入时运行相关消费者；不要每改一个文件就重建整个Rust树、全量复核、逐步review。

保留有实际价值的少量验证：数据库写入和入队一起回滚/提交；原生查询在取消后释放资源；历史/权限与来源不串；真实媒体两段调用确实使用原图；真实Codex/MaiBot入口完成长期读写与重启继续。既有VCP公式区分案例迁入native测试，移除保护已废弃Rust内部形状的测试。

用真实任务、资料、决定与故障修正记录dogfood Nous，别为演示而编造语料。标准单元测试可以构造最小数学或事务输入，但不要拿这些输入宣称真实召回质量。工具输出以功能与具体阻塞为中心，不产出测试数量竞赛、覆盖率门禁和庞大证据档案。

## 多代理协作

仅在云端实际提供子代理能力时使用。按独立owner分工：native与计算边界、Python事务和平台、领域与外部消费者。共享的合同与数据库schema由明确负责人统筹；变更合同后通知实际消费者并完成接线。不会因为多个子代理分别报告完成就结束主任务。

沿用已存在的任务上下文与目标恢复机制，保存Objective、有效Decisions与Current Work。提交保存有用集成进展；可随时被用户暂停并从交接继续。不将每次调用记录扩张为永久台账。
