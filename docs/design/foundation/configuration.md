# 配置定义、解析与有效快照

## 负责范围

Configuration 统一负责字段发现、类型与范围校验、分层覆盖、来源解释、修改和有效快照。模型、网络、Wave 等模块分别维护自己设置的含义，Configuration 组合这些纯定义。

首轮实现完整机制和本包确定的字段。模型相关纯配置可以读取和校验；未实现的模型执行能力在状态中显示未接入。读取配置不产生 Provider 请求。

依赖：Pydantic 2、pydantic-settings、tomlkit、标准文件能力、[路径](paths.md)与[错误](errors.md)。解析不依赖数据库、Trace 或网络。持久绑定检查通过[变更规则](configuration-changes.md)在需要时由实际 owner 参与。

## 不同维度

每个字段分别定义：

| 维度 | 值或含义 |
|---|---|
| 类型 | 严格数值、字符串、枚举、数组、对象、可判别联合及必要的格式 |
| 缺失 | 必需、具有默认值、允许继承；与显式 null 分开 |
| 约束 | 单位、范围、长度及真正需要的跨字段约束 |
| 范围 | instance、namespace、task、call 中允许出现的位置 |
| 呈现 | standard、advanced、developer、internal |
| 修改效果 | live、next_operation、restart、new_binding |
| 修改影响 | ordinary、disruptive、data_bound |
| 保护 | secret_value、只读身份、需要能力或资源才能采用 |
| 描述 | 用途、默认原因、改变后果、配置例子 |

呈现层级不决定可编辑性，也不决定修改风险。普通用户会选择 Embedding 模型，因此一个 standard 字段也可以具有 data_bound 影响。

## 定义与来源

各 owner 提供纯 Pydantic 配置模型。类型、默认值和约束直接定义在模型中；补充元信息通过 Field metadata / `json_schema_extra` 的 `x-nous` 扩展附着。配置目录从这些模型导出，不手写第二份同名字段表作为运行权威。

模型默认值可以保存在代码定义中；可调整的行为在调用点从已解析设置读取，不散落魔法数字。依赖库的所有内部常量无须重新登记，只有 Nous 正式选择、覆盖和暴露的参数进入目录。

包内算法 profile 可以是 JSON，操作者配置使用 TOML。本轮实例和 Namespace 文件是期望设置的保存位置。数据库以后只保存明确需要数据库的领域设置和身份绑定，不镜像整份 TOML。

## 覆盖顺序

启动范围由低到高：包内默认值 → 实例 TOML → 显式进程环境覆盖 → 显式启动参数。

业务范围先取得上述实例有效设置，再叠加被允许的 Namespace 文件、任务设置、本次调用设置。字段只在其声明的 scopes 内可覆盖。实例的资源上限单独作为上限应用，Namespace 和调用不能通过普通覆盖扩大它。

`NOUS_INSTANCE` 只定位实例。配置环境键使用 `NOUS__SECTION__FIELD`；明确覆盖记录其来源。对于动态资源 key，配置标识采用小写 ASCII slug，禁止双下划线，避免环境路径歧义。实际 provider 模型名保留原字符串，不按 slug 改写。

pydantic-settings 负责成熟的来源读取和类型转入；Nous 在 source 边界保留来源定位并执行上述固定覆盖顺序。不得额外保留一条“环境直接读入网络模块”的旁路。

## 合并语义

普通结构对象按已定义字段递归合并。数组整体替换，fallback 列表和消息顺序不会拼接或去重。资源映射按明确资源 ID 覆盖，不按列表位置合并。未知字段是配置错误；provider-specific 参数只在对应受控资源的扩展区内保留，并按执行器实际能力校验。

缺失表示继承。显式 null 仅允许在 nullable 字段上出现。TOML 没有 null，因此保留 `[config] null_fields = ["network.proxy_url"]` 表达该层的显式清空；同一层同时给出字段值与 null_fields 是错误。

`unset` 删除当前层的值及该路径的 null 标记，从而恢复继承。`set --null` 写入 null 标记。空字符串、0、false、空列表都按其正常值处理，不用它们代替缺失。

删除资源项使用公开的删除操作，先检查引用它的任务和 fallback 链。它不等同于把某个字段写为 null。

## 读取与快照

共同操作：

```text
catalog(filter) -> FieldDefinitions
resolve(scope, call_overrides?) -> ConfigSnapshot
explain(path, scope) -> EffectiveValue
validate(input, scope, checks) -> ValidationResult
update(patch, scope, expected_version?) -> ConfigChangeResult
reload() -> ConfigChangeResult
```

`ConfigSnapshot` 包含已校验值、当前版本以及每个叶子的来源。运行时版本是进程内变化计数；不要求用户手写它。需要并发编辑保护的调用可以使用读取结果的 expected_version。

配置值对消费者不可变。`frozen=True` 只限制属性赋值，不能单独保证嵌套 dict/list 不变；实现采用不可变集合或封装后的只读结构。公开序列化返回新副本。一次完整操作保留同一快照，不在每个 helper 中重新解析环境与文件。

当前 Python 入口为 `nous.configuration.Configuration(config_file, ...)`。`snapshot` 是属性，`snapshot.values` 提供递归只读的属性与映射访问，数组为 tuple；`to_dict()` 返回独立可变副本，`typed_values()` 返回独立的 `NousConfig` 强类型副本。`update`、`unset`、`reload` 是异步操作；持久绑定检查和采用回调由持有资源的应用注入。`version` 表示已接受的期望配置代，`snapshot.version` 表示有效代；带 `expected_version` 的修改对照前者。`status()` 给出两者、等待重启状态及尚未恢复的采用失败。保存成功后的采用失败会保留旧有效快照并转为整项等待重启，具体结果见[修改与绑定](configuration-changes.md)。

`explain` 返回有效值、保存值、来源层及定位、生效状态和被覆盖信息。Secret 只返回引用和可用状态，不返回实际值。

## Namespace 与任务配置

首轮完成所有范围的纯合并、校验及文件读写接口。NamespaceLocator 由调用者提供；配置服务不据一个配置文件创建认知域。首轮 CLI 只提供 instance 范围管理，后续 Namespace 管理按同一接口接入。

进程内调用使用 `resolve(namespace_file=..., task_overrides=..., overrides=...)` 依次叠加 Namespace、任务和调用设置。Namespace 保存使用 `update(patch, scope="namespace", namespace_file=...)`；`unset` 和 `reload` 接受同样的目标。运行中的 Namespace 文件在首次取得时建立有效值，之后通过明确修改或 reload 发布，外部文件编辑不会自动进入下一次操作。实例存在待重启修改时，Namespace 新保存值也等待重启。

任务定义注册所需设置和允许的调用覆盖。Journal、Steward 等任务在自身实现时加入，无须先把全部未决认知参数写进当前目录。

## 失败与验证

语法、字段类型、未知键、缺失值、scope 不允许及跨字段冲突都给出具体路径和可理解原因。校验失败不写文件、不发布快照。文件外部编辑后需要显式 reload；不默认启动文件监控器。

覆盖、数组替换、缺失/null、来源解释、实例上限、旧操作快照与新操作快照分别以真实配置例子验证。来源：[Pydantic Settings](https://docs.pydantic.dev/latest/concepts/pydantic_settings/)、[TOML Kit](https://tomlkit.readthedocs.io/en/latest/)。
