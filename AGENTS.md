# Nous Development Guide

## 当前唯一开发目标

新仓优先完成一个**可单独安装、使用和发布**的通用 TypeScript/Rust 基础平台。独立平台经过真实、与 Nous 无关的应用使用并确认其可独立发布后停止本阶段，未经新的产品授权不继续编写 Nous 的记忆、Wave、Steward 等业务。

旧 Python/PyO3 系统保存在 legacy/nous-python/；PR #60 仍为独立历史开发分支。历史类名、包结构、内部表、测试和流程没有新仓实施权威。正常开发与检索排除 legacy/，研究特定算法或用户操作时按需定向阅读。

产品目标参见 docs/design/FOUNDATION.md；真实进度见 docs/CURRENT.md。项目产品语义以项目宪纲和获接受的设计为先，旧讨论与文档用于研究，不能自动成为已批准决定。

## 工程治理

开发前完整阅读并应用 docs/agents/antirlhf/CONSTITUTION.zh-CN.md，按需阅读 docs/agents/llm-use.md 及相关 Skills。治理原则、原始设计讨论和开发者笔记保持原有权限：未获明确请求不得修改治理文件、AGENTS.md 或 docs/AGENTS.md 指定的受保护内容。需要变更时提交议案补丁，继续其余已授权工作。

从完整用户操作决定接口、所有权及数据形状。模块应具有可直接使用的深接口；新增字段、参数、状态、转换、检查或适配层必须服务真实消费者。发现旧结构不适合需求，修改定义者及所有实际消费者并删除旧结构，不保留内部双轨或历史兼容。

优先使用 Bun/标准库、成熟依赖和 Rust 既有计算成果；高性能计算由 Rust crate 负责，TS 负责业务及资源组织。跨包只能使用 exports 公共入口；包内相对导入合法，禁令按目标实际包归属判断。单次内部对象尽量直接传递值，正式引用仅在确有长期身份时建立。

禁用 TDD/红绿循环及机械测试清单。真实完整操作、独立失败和关键数值算法需要有价值的检查；单纯字段存在、成熟依赖内部及过时私有结构不必重复测试。审查应评估复杂性是否真正下降，不以通过率或代码行数代表架构质量。

## 工具与运行约定

Bun 1.4.4，TypeScript 7.0.2，Zod 4.6.7，Rust 1.100.0，napi-rs 3.x。Bun workspaces/catalog/isolated linker、bun.lock；Cargo workspace/Cargo.lock。TS 严格类型与项目引用；Rust 算子 crate 与薄 napi crate 分开，每个 worktree 自行构建其 Native 产物。

构建和检查由新仓统一命令 check:repo 提供，提交工具采用 Lefthook。此时新仓尚未创建该脚本，故不得假装旧 uv、PyO3 或 Python 脚本是有效构建命令。不要用外部 CI 或过时测试代替实际功能。

仅在项目确实存在真实 Native/CLI 实现时运行相应检查；文档描述当前可用功能与尚未实现的能力，不编造通过记录。

临时工作和样本放入仓库忽略的 temp/。跨平台代码不写入开发者机器的绝对路径，Portable 发行必须含程序、资源与对应平台 Native 模块。不要通过自动安装或自启动更改开发者环境。

## 文档与参考

docs/design/ 是当前接受的基础平台方案；docs/references/ 保存旧产品设计与使用场景；docs/research/ 是研究；docs/sources/ 是历史原始资料；legacy/ 保留旧实现和执行文档。活动文档随职责和合同变化原地更新，移除失效机制的描述。

根 AGENTS.md 保护和目录 AGENTS.md 的文档权限限制延续。
