# 配置采用与资源所有权

## 共同操作

Configuration 维护定义、来源、快照和 TOML；ConfigurationOperations 负责一次修改的路径/时间/向量/Wave 绑定及生效。CLI 和在线调用都使用这一操作。底层纯定义不打开数据库或加载 Native。

在 configuration 内按实际修改原因分离目录/来源解析、快照、文件保存；保留清楚公共能力。不要把一个巨大 self 分散到若干 mixin。应用用例构造时接收绑定能力，删除已被替换的 callback 回填或消费者判断。

## 保存与发布

继续使用候选 → 只读绑定核对 → 原子保存 → 明确可立即采用的状态 → 发布快照。含 restart 的整项修改等待重启；保存后采用失败保留旧有效值并返回具体阶段、期望/有效版本和恢复办法。离线普通字段无需打开不可用数据库。

时间域登记与 Embedding 空间绑定遵循各自 owner。新向量空间可配置，首次向量采用才绑定。Wave 构建参数改变只影响新快照；查询参数影响下次查询，不能原地改变已经发布快照的含义。

## 连接与凭据

Provider endpoint/protocol/credential_ref、代理、CA、连接池以及被该连接使用的 Credential.kind/name/path 采用 restart。实例打开时解析的环境和凭据引用具有明确生命周期。逐请求时限、模型选取、任务预设与查询参数可在下一操作生效。

给 credentials 容器及所有联合分支附正确元信息，避免只标引用字段却让实际凭据位置变更立即发布。新增 Provider/凭据一并修改时，默认值的差异也按完整资源合同计算生效模式。

## 缺失与显式 null

Provider 网络覆盖中，缺失表示继承实例设置；显式 null 表示清空允许为空的字段，例如取消该 Provider 的实例代理。不能用 `exclude_none=True` 合并两者。

由 source resolver 在构造有效 ProviderSettings 前保留“本层是否声明”的信息。对资源初始化的默认 null 不视为显式清空；本层 null_fields 仍是明确输入。将结果解析成完整、可独立消费的网络配置，Network 不再自行做第二轮来源合并。

## 快照与模板

一次操作固定 ConfigSnapshot；同时捕获本次可能需要的 Prompt 资源及其 literal 依赖版本。多阶段操作后半段使用已捕获模板；磁盘编辑影响下一根操作。Native build/query 分别固定其 build/query 配置。预览与执行不得另读配置形成双轨。

## 字段目录与维护

动态模型/媒体/Wave 字段也必须导出类型、描述、缺失、nullable、范围、单位、scope 和生效方式。已选成熟 SDK 的协议配置继续复用，名称只描述协议能力。媒体策略由 MediaAnalysis 的操作参数明确选择，不在模型 TaskAssignment 中配置。
