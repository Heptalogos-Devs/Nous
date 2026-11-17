# 逻辑路径与物理位置

## 目标和依赖

业务根据文件用途使用逻辑位置，物理目录由实例统一解析。首轮覆盖 Windows、Linux 和 macOS 的规则；验证报告只声明实际运行过的平台。

采用 `pathlib`、`platformdirs` 和 `importlib.resources`。路径解析只需实例描述与启动配置，无数据库依赖。

## 逻辑位置

| 根 | 内容与生命周期 |
|---|---|
| config | nous.toml、Namespace 配置、操作者编辑的 Prompt |
| data | SQLite 和正式数据 |
| content | 正式原件、修订正文及大字节 |
| index | 可重建索引 |
| cache | 可淘汰缓存、模板 bytecode |
| logs | 运行日志与受配置控制的诊断 |
| temp | 处理中间文件，按负责操作清理 |
| runtime | 本次运行所需短期文件；写入锁固定在配置目录及实际 data 根 |
| exports | 用户请求的导出产物 |

`ResolvedPaths` 为以上逻辑位置提供已解析的绝对 Path。各模块通过 `paths.for_kind(kind)` 或类型化属性取目录，不独立读取平台环境变量。

## 默认位置与覆盖

platform 布局按 platformdirs 的 config/data/cache/state/log 目录区分用途，并追加同一实例的稳定目录名；content/index/exports 可以位于实例 data 根下。runtime 使用平台状态根下的实例运行目录，不依赖短期系统临时目录来保存长期状态。

portable 布局把 data、content、index、cache、logs、temp、runtime、exports 放在指定实例目录下的相应子目录；配置文件位于该实例目录。代码安装目录和实例目录始终分开。

显式 init 为 data/content/index 建立资源根和持久身份。正常 open 只创建 cache/log/temp/runtime/exports 等运行所需目录；三个正式资源根或其标记缺失时报告实际错误。

所有相对配置路径以 `nous.toml` 所在目录解析，不以 shell 当前工作目录解析。用户显式指定的绝对路径按原意使用。路径字段不执行任意 shell 展开；环境值通过正式配置来源提供。

## 数据内的路径

Nous 自有文件的持久引用保存内容身份、逻辑用途和相对位置。物理绝对路径留在当前实例解析结果。外部原始文件路径可以作为来源描述保存，但再次访问属于显式外部输入操作。

`resolve_owned(kind, relative)` 只解析对应根下的相对位置，拒绝越过根的 `..` 或绝对路径输入。这里保护的是正式存储命名；`ingest` 将来仍可显式接纳任意经授权外部文件。

包内默认 Prompt 和资源通过 `importlib.resources` 读取，不假定安装包有固定可写文件夹。

## 路径变更

cache/log/temp 路径改变按配置的重启规则应用。data/content/index 路径变更涉及现有资源位置：本轮只能在停止实例后用显式重定位操作或初始化新实例完成，不允许普通配置重载悄悄连接一个空数据库。

重定位的完整复制与校验不在首轮施工范围；首轮对已经存在的数据根改变返回 `configuration.relocation_required`。未初始化实例可自由选择路径。

位置合同记录在配置目录的 `.nous-instance.json` 中，覆盖 TOML 手动编辑、环境覆盖和启动参数。配置目录内部资源记录相对位置，外部资源记录绝对位置；portable 整体移动时随配置目录重新解析，单独改变 data/content/index 的位置仍需显式重定位。资源 UUID 标记用于发现同一位置被替换为空目录或其他实例资源，数据库另有独立 UUID 用于核验所选文件。

## 完成条件

改变工作目录不改变相对路径解释；portable 实例整体移动后可重新解析相对路径；默认路径不含开发机器的个人目录。错误能指出具体逻辑根和物理位置。来源：[platformdirs API](https://platformdirs.readthedocs.io/en/stable/api.html)。
