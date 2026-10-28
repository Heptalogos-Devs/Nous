# Python 与 Rust 原生边界

## 同进程、粗粒度计算调用

`nous_native` 是一个普通 Python 扩展包，由 maturin 构建。应用通过 PyO3 调用，部署时不再启动 Rust HTTP/CLI/Kernel 服务。Rust 不提供第二套认知 API，不连接 SQLite，不获得用户凭据。

一次 Rust 调用尽量完成一个完整计算：解析查询语法、构造索引代际、检索候选、运行 VCP pipeline、融合/排序或读出。不要把每个节点扩张、每个 Tag 选择或每次矩阵乘法都做成 Python↔Rust 往返。

现有 `nous-vcp` 和 `nous-retrieval` 作为算法起点，保留真正有效的公式、资产身份、中文 tokenizer、USearch 代际和数值研究。现有 `nous-domain/src/nousql` 提取出语法部分，保留 `manual.md` 与已有语法能力；名称解析、五轴时间求值和认知查询计划属于 Python 查询 owner。

## 边界数据

| 输入 | 表示与规则 |
|---|---|
| 查询语言 | UTF-8 Python str；Rust 使用现有 parser 返回小型语法 JSON、位置信息或 ParseError |
| 候选身份 | 构建资产时传 tuple[str, ...] 的不透明修订引用，之后使用整数行号 |
| Dense 向量 | 连续二维 float32 数组 `[objects, dimensions]`；查询为一维 float32 |
| VCP 数值资产 | 保留原内核所需精度，EPA/几何等浮点64不得为了统一接口改成float32 |
| CSR 图 | `indptr: uint64[n+1]`、`indices: uint64[m]`、权重按现有算法的float32/64合同 |
| 授权选择 | 当前允许的候选行与图节点位图/索引；应用已经确认 scope 与来源策略 |
| 数值参数 | 明确的结构化配置；采用已有参数名称与约束，不使用任意字符串map替代合同 |
| 结果 | 身份行号、分数、排序、必要路径和诊断；不从 Rust 返回完整 Memory/Subject 状态 |

词项索引可以在没有模型向量、CSR或EPA资产时建立并使用；对应参数保持缺省。Snapshot明确记录已具备的计算能力，请求尚无资产的算法时由查询owner处理准备或明确返回所缺能力，不填零向量冒充有效资产。

native 的 `.pyi` 对公开方法和异常完整标注。数组维数、布局、dtype、index bounds 在 native 入口检查一次；模型返回和数据库字段在各自入口处理。允许的诊断包括算法实际耗时、候选数量、迭代停止原因和资产代际，不要求每次保存完整矩阵。

语法树、少量参数等小数据可以用 JSON 在这条真实语言边界传递，Python 立即解析成确定类型。向量、矩阵、CSR 与大量候选使用 NumPy/buffer；不得经过 JSON 文本膨胀再反序列化。

## 数据所有权与 GIL

资产初始化时把 Python 数组复制到 Rust 拥有的不可变内存，建立 Arc 持有的 Snapshot。一个代际只做一次实质复制，多个查询复用该 handle。单次 query vector 体积很小，可复制后进入计算。这样 Python 的其他引用、数组视图或调用者修改不会与脱离 GIL 的 Rust 计算竞争。

仅设置 NumPy `writeable=False` 无法约束所有潜在别名。第一版明确采用 owned snapshot；有真实性能需求再在负责的资产模块实现可证明生命周期的 mmap/buffer。

`Python::detach` 包住纯 Rust 的计算和等待原生锁，闭包内不访问 Python 对象、不调用 Python回调。原有 VCP 需要 Tag 搜索适配时使用同一 native 包内的 USearch/索引 adapter，不能把热循环退回 Python。

Python 的 async 请求通过 `asyncio.to_thread` 或 AnyIO 的受管线程运行粗粒度 native 方法；detach 释放 GIL，而线程卸载避免阻塞事件循环。二者解决不同责任，均需要接通。

Rust rayon 池由 native owner 按实例配置创建并复用，搜索并发和线程总数有上限；不能每个 HTTP 请求再启动一套线程池。线程等待和索引关闭都由服务生命周期管理。

## 取消与资源释放

native 导出一个轻量 CancellationToken，内部使用线程安全原子状态。计算在已有迭代/批次边界查看它；不在每次浮点运算中检查。取消结果携带明确取消状态，已完成的查询不会被伪报为未执行。

Python 取消等待时先设置 token，并等 native 释放 index reader、文件映射与数组 snapshot 后结束 owned 工作。AnyIO 线程调用采用不遗弃实际工作的模式；不能放任服务退出后工作还在后台修改索引。

索引 writer 属于一个实例的原生索引服务，新代际写到临时路径、完成后发布。Python 的索引目录是显式受管路径，native 不自行发现当前工作目录。查询 reader 可继续持有旧 snapshot；引用释放后才清理旧文件。

## 查询的完整顺序

1. Python 查询入口验证 caller、Subject、requested kinds/历史与来源范围。
2. `parse_nousql` 使用现有 Rust grammar；Python 绑定稳定引用、人物、Tag 与 Directive，取得一次明确的 query clock/read view。
3. Python 准备合法候选、图与 embedding 空间，必要的模型向量由模型服务产生并正式采用。
4. 使用 native snapshot 执行词项、Dense、VCP、Hybrid 或精确计划中的计算部分。
5. Rust 返回候选修订引用、分数、真实路径和覆盖信息。
6. Python 按请求的历史截点和当前权限展开认知与来源，形成模型可读结果与严格 JSON。

五轴、`$asof`、`$history`、14类返回域、`@ref`、`$explore`、偏好与组合语义继续来自现行宪纲和已实现手册；迁语言不引入另一套 FIND 语法。

## 内核包的拆分方式

- `native/vcp`：从现有 `crates/nous-vcp` 直接采用算法；删除只因旧应用框架产生的依赖，保留数值合同和必要研究样本。
- `native/retrieval`：迁入 `crates/nous-retrieval`，保留 Tantivy 中文索引和 USearch。候选物化、账本和来源权限上移 Python。
- `native/nousql-syntax`：从现有 parser 及对应AST中提取纯语法。剥离对 QueryRequest、storage、运行配置和 clock 的依赖。
- `native/python`：一个 PyO3 模块负责参数转换、所有权和异常映射，提供 NumPy 批量入口、schema/stub。不得在这里实现认知规则。

不同时维护 Tantivy Python 现成包和另一套自制 Tantivy 查询语义。当前已有中文 tokenizer 接线，第一版采用现有 Rust 检索，减少语言迁移中的行为变化；以后若上游绑定覆盖实际需要，可在这个 owner 内替换。

## 核心目标接口

`templates/native-core.pyi` 是本次设计的实现合同草案，命名可与实际内核合理合并。需要完整实现 Snapshot、索引代际、候选/Tag 搜索、VCP读出和查询解析；不要只暴露若干低级矩阵函数再在 Python 重建整套已存在的原生算法。

stub 中的 `options_json`/`assets_json` 只容纳少量metadata，必须有在包内导出的对应JSON Schema。完整数组永远走数组参数。生成 schema 与 stub 时以Rust公开计算类型为来源，只维护这条实际边界，不发展全仓通用RPC代码生成框架。
