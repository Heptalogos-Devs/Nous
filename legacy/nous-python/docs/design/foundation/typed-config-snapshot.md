# 一次解析的类型化配置快照

## 目标

`ConfigSnapshot.values` 持有已经校验的 `NousConfig`。正文缓存、读取回调和模型准备直接读取该视图，不在普通字段读取时复制或重新校验完整配置。

## 合同

```python
class ConfigSnapshot:
    values: NousConfig  # 深度不可变、已校验的类型化视图
    version: int
    sources: FrozenMapping
    state: str

    def to_dict(self) -> dict: ...  # 显式产生独立可修改副本
```

创建/resolve/合法覆盖时一次校验、一次冻结；只读 getter 返回同一个视图，不重新构造。版本只标明本 Runtime 的配置接受次序，不能单独作为跨重启冻结任务身份。

Configuration 仍负责 Schema、来源合并、nullable/unset、生效方式及保存状态。配置定义仍归各 owner；Rust 参数 Schema/defaults 继续由 Native 定义供给。

## 深度不可变的实现

统一 metadata 基类使用 Pydantic frozen 模型，`ConfigSequence` 将输入数组校验为 tuple，`ConfigMap` 将映射校验后冻结为只读 Mapping，`ConfigJson` 递归表达不可变参数树。共同适配保留 Schema 和 JSON 序列化；`model_dump(mode="json")` 与 `ConfigSnapshot.to_dict()` 返回独立 dict/list。来源描述使用 `FrozenValues`，与类型化配置分别保存。

仅设置 `ConfigDict(frozen=True)` 不足以保护嵌套 dict/list。models/presets/extensions、native 配置、provider overrides 以及 sources 都应在实际可达对象上不可改写。序列化向用户或 SDK 返回独立 dict/list，不能把视图内部容器交出去。

配置更改建立新候选并校验，资源采用成功后发布新 Snapshot。运行中的操作保留其旧快照，新根操作取得新的有效视图。需要修改参数的执行器以 model_copy/明确构造新值完成，不能就地改共享 preset。

## 活动配置与调用范围

一次完整操作固定有效快照；后台维护使用启动时捕获的配置并由当前 Runtime 持有。保存的成果记录实际请求与生产者信息，读取不依赖旧运行环境。

Namespace/task/call 合法覆盖在操作入口解析一次。业务层读取已解析的模块视图；ArtifactStore 的上限和 cache settings 回调可以继续存在，但返回已有类型化子视图，不能重复调用重校验 getter。

## 配置变更执行边界

ConfigurationOperations.update/unset/remove_resource/reload 是公开用例，应进入 Runtime 的 ExecutionManager 后再取得配置更改锁。等待配置锁、数据库绑定与文件采用具有同一根期限、诊断与关闭责任。

保留现有 file_saved、write_performed、effective/pending_restart 状态。文件已保存后后续资源应用失败，返回真实 partial/apply_failed 信息，不能声称文件未写。取消与超时也附带实际已完成状态，不由入口重新猜测。

离线配置操作仍不依赖完整模型/媒体/Wave 启动；由现有 open_configuration_operations 持有所需执行与数据库资源，调用同一个修改核心。

## 迁移及完成条件

迁移所有 typed_values() 调用，不通过保留昂贵旧方法和新增便宜方法形成长期双轨。修改配置基类、NousConfig 与资源模型、Configuration.resolve/save/publish、Runtime 装配回调、模型 freeze、Wave profile、media 和 cognition 消费者。

一次 501 文稿构建不会按文稿数执行整配置验证；对视图嵌套集合尝试修改失败；to_dict 修改不影响快照。并发操作看到各自固定版本，配置更新等待与期限可观察，失败后的保存/生效状态准确。
