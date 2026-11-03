# 首轮本地管理入口

## 范围

首轮提供本地 CLI 和相同的进程内应用操作，服务于实例、配置、诊断与时间。完整 HTTP、MCP、远程 SDK 和后台服务部署在后续专题接入。

CLI 采用 Typer / Rich。命令只解析输入和格式化结果，业务进入相应 owner。配置、数据库和模型模块不反向依赖 CLI。

## 规范命令

```sh
nous --instance ./temp/nous-dev init --layout portable
nous --instance ./temp/nous-dev instance show
nous --instance ./temp/nous-dev config list --level advanced
nous --instance ./temp/nous-dev config show --effective
nous --instance ./temp/nous-dev config explain logging.level
nous --instance ./temp/nous-dev config set logging.level DEBUG
nous --instance ./temp/nous-dev config unset logging.level
nous --instance ./temp/nous-dev config validate
nous --instance ./temp/nous-dev config schema --output ./temp/nous-config-schema.json
nous --instance ./temp/nous-dev runtime check
nous --instance ./temp/nous-dev time parse "2026-11-03" --timezone Asia/Shanghai
```

`--instance` 指配置目录；不写时按实例专题解析。`--json` 交付完整类型化结果；正文默认可读，日志写 stderr。数组和对象可以通过合法 JSON 值文件输入，普通字段不要求手写完整请求对象。

## 离线与运行中的实例

首轮本地修改取得实例锁。已经被另一个运行时持有时返回 instance.in_use，提示使用相应在线管理入口（若该版本已经提供）或停止后管理；不能偷偷打开第二个写入服务。

离线 `config show --effective` 表示当前文件和本命令来源计算的“下次启动有效值”，在结果中明确 `state=prospective`。它不冒充另一个进程内当前生效的快照。`config show --saved` 可以只读已保存值。

离线保存返回 saved；当前进程内 Application 的 Configuration.update 可以真正发布新快照，按变更专题返回 applied 或 saved_pending_restart。后续 HTTP/MCP 使用这个同一操作。

## 用户可见内容

instance show 展示名称、布局、各逻辑路径与实际位置、是否被占用及必要配置问题。runtime check 真正创建并关闭本次运行资源，返回数据库可读性、配置绑定检查、观测模式和能力可用性。

config list 默认展示 standard；advanced/developer/internal 由显式展开。字段说明含类型、默认值、允许 null、范围、作用域、生效方式和危险影响。风险字段的后果不因隐藏高级级别而消失。

time parse 展示原输入、域、精度、点或范围以及不确定性。程序不以当前时间补齐未知事件时间。

## 不做的入口

首轮不提供几十个内部生命周期工具，也不为每个配置字段创建独立命令。模型 probe 与 Prompt 管理在模型计划中增加，不用空实现提前出现在帮助里。

## 完成条件

初始化 → 配置解释与修改 → 实际打开数据库 → 查看运行诊断 → 关闭 → 再次打开的操作链可用。当前范围的中文/英文帮助一致，JSON 与 stderr 分离；所有值来自真实操作结果。
