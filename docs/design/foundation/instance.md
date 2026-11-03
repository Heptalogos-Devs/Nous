# 实例与运行占有

## 目标和范围

一个 Nous 实例拥有自己的配置位置、持久数据、运行资源与生命周期。首轮实现本地实例的创建、定位、检查与单写入进程占有，不创建操作系统常驻服务。

依赖：[路径](paths.md)、[错误](errors.md)、标准文件能力和 `filelock`。实例定位在数据库和正式 Trace 之前可用。

## 正式数据

`InstanceLocator` 指定本地实例配置目录。CLI 的 `--instance PATH` 明确传目录，目录中使用 `nous.toml`。未给参数时读取 `NOUS_INSTANCE`，仍未给时使用 platformdirs 对应平台配置目录下的 `instances/default`。

`InstanceDescriptor` 包含 `instance_id`、`display_name`、`layout`。`instance_id` 在 `init` 时生成，内部使用 UUID，CLI 主要显示可读名称和路径。layout 为 `platform` 或 `portable`；此值决定未显式指定的路径默认值。

实例与 Namespace 分开。基础阶段不以创建一个 Namespace 为启动前提，不把操作系统用户或模型身份用作认知数据身份。

## 完整操作

`initialize_instance(locator, layout, name) -> InstanceDescriptor` 创建配置和必要目录，初始化基础数据库。已有有效实例时返回其信息；已有其他文件或残缺实例时说明实际情况，不覆盖用户文件。失败留下的已创建文件可在下一次检查中说明，不自建通用安装事务框架。

`inspect_instance(locator) -> InstanceInfo` 只读取本地元数据、路径和运行占有信息。数据库损坏也应能看到配置位置和诊断，不要求完整启动。

`acquire_instance(locator) -> InstanceLease` 使用 `filelock` 的本地原生文件锁，生命周期覆盖全部写入资源。进程结束或正常关闭释放锁。锁文件不是 pid 真实性权威；不得仅因文件存在而判定运行，也不通过删除锁文件强行夺锁。

## 并发使用

同一实例默认一个负责数据库写入和后台工作的应用进程。不同实例可以由不同进程运行。第一轮本地管理修改配置或执行数据库检查前取得同一实例锁；实例已运行时返回 `instance.in_use`，不另开第二个本地写入运行时。

第一轮没有跨主机共享实例目录的保证。网络数据库和多节点运行不进入该实例锁合同。

## 实施与验证

组合根在连接数据库前取得占有，在最后关闭存储后释放。只读文件检查不需要独占写入。能在含中文和空格的路径初始化、关闭、重新打开；第二个写入进程收到清楚错误；异常启动后资源与锁正常释放。

采用 [filelock 原生平台锁](https://py-filelock.readthedocs.io/en/stable/concepts.html)，不实现 pid 心跳、租约 fencing 或自动抢占。
