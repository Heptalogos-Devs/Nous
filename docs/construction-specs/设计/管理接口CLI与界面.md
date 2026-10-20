# 管理接口、CLI 与界面

## 面向用户任务的管理

Nous 与 Hept 都提供独立 API、Client、CLI 和 Web Console。联合界面围绕同一个主体展示生活、交往、工作与认知。页面、命令和维护助手调用相同业务操作，用户无需按内部表依次修改。

管理范围包括安装与启停、主体、模型资源、配置与密钥、渠道、世界和日历、注意与活动、追求和承诺、计算环境、认知、成本、日志、导出、备份与清除。完整能力通过任务入口呈现，内部派生对象只在需要时展开。

## 启动与身份

首次启动以本机 CLI 创建管理员并完成 bootstrap。浏览器登录由 Better Auth 提供；API key 按产品、主体与操作范围发放。远程访问在明确配置的 TLS 地址启用，默认仅回环监听。

软件管理员可以管理部署与资源；行为管理员可以调整主体参与范围和生活策略；创作者可以编辑种子与世界；普通交往者只参与社会交往。权限可以由一个实际用户同时拥有，但界面明确当前操作的身份与影响。

管理员与主体的私聊是正式交往入口。维护助手使用独立管理会话、凭据和工作空间，调用管理工具；人格主体不会通过普通对话获得安装权限。

## 公共接口形式

HTTP 路径以 `/v1` 开始，主体操作位于 `/v1/subjects/{subjectId}`。同步读与短修改直接返回对象或视图；长工作返回 OperationRef，支持 SSE 跟踪。批量任务接受一个完整输入，避免客户端串联内部准备、登记和完成步骤。

正式操作的名称、输入输出和路径见[公开操作目录](../公开操作目录.md)。公共 schema 生成 OpenAPI 和 Client；模型工具直接选用这些业务操作的授权子集，不另建一套语义不同的工具 API。

Client 提供标准分页、文件流和操作等待，保留原错误 code 与 operationId。服务端组件用内部应用入口，跨产品用 Client。MCP 的查询、阅读、材料提交和状态操作由官方 SDK 暴露，同一权限在 API 入口检查。

## CLI

采用 Commander 与 Clack 形成命令与交互式引导。全局参数包含 `--profile`、`--subject`、`--json`、`--quiet`；表格和进度用于终端，自动化使用稳定 JSON。凭据存储使用正式 SecretRef。

```text
nous init / serve / status / doctor
nous subject create / bind / capabilities
nous query "我们上次讨论的那家店"
nous read <ref> / source <ref>
nous seed preview / apply
nous growth run / inspect
nous settings get / set / reset / history
nous data export / import / fork / purge

hept init / serve / status / doctor
hept subject create / start / pause / stop
hept chat / inbox / conversation
hept world preview / create / inspect
hept calendar list / add / change / timeline
hept activity inspect / interrupt / resume
hept pursuit / task / commitment
hept why / operation / trace
hept settings / model / channel / workspace
hept data export / import / fork / purge
hept backup / restore / service / update
```

这些是命令组，内部子命令按公开操作目录补全。每个有写作用的命令说明目标与实际影响，未知效果处理要求明确选择。导入和 seed 支持先预览再采用，`--yes` 用于已经明确参数的自动化。

`status --watch` 通过 SSE 展示当前主体活动、前台、后台任务和最近告警，终端不需要持续刷出内部事件洪流。普通操作在模型不可用时仍能运行。

## 无服务时的有限管理

数据库或主体未运行时，CLI 仍能读取 bootstrap、查看系统服务、停止进程、调整连接和目录、调用 PostgreSQL/备份工具以及输出诊断信息。这些操作直接使用所属模块的离线入口，不绕过认知或世界事务写内部表。

正常数据操作要求相应产品数据层可用。恢复模式仅提供修复运行环境所需能力，使用显式的管理员执行与目标实例选择。

## Web Console 的结构

界面从主体和正在发生的事进入。首页展示主体状态、当前生活、待查看工作和管理员私聊；平时收起技术状态，出现需要处理的事件时显示通知。系统维护助手与主体聊天在界面上分开，维护操作显示其实际权限。

主要任务区是生活、交往、工作、认知、资源与设置。生活使用时间线与日历，交往保留完整消息链，工作按追求与成果组织，认知以可读内容和来源呈现。用户可以从一句话进入来源和形成过程，也可以从某次调用回到它服务的任务。

采用 React、Vite、TanStack Query 和成熟可访问组件库，日历使用 FullCalendar 的标准能力。图标与配色使用现成设计资源，交互减少无关字段和装饰。通用表单用 RJSF，模型/预算/日程等关键任务采用渐进式专用表单。

身份、读取和错误状态由真实接口返回。演示数据只在独立示例环境中出现。原始技术字段在 Developer 披露层，不占据普通操作者页面。

## 完整创建流程

用户先给主体命名，选择是否使用种子，再选择世界或仅交往。界面展示种子中 Memory、Self、Social、Motivation 的将要形成内容，世界包展示地点、人物与日历起点。未提供种子时创建空的初始认知，之后可以自然形成。

接着选择角色模型与费用上限。模型相同时可共用资源，角色输入范围仍不同。最后配置渠道和工作空间，展示已启用能力、实际发送范围及需要安装的可选依赖，完成后启动。

用户可以先创建并管理主体，再启动人格运行。解锁管理、启动服务和启动主体是独立状态。重新打开页面继续同一主体和未完成工作。

## 查看与编辑认知

Nous Console 提供自然语言搜索、NousQL 编辑、对象阅读、修订历史、来源展开、人格文本、关系、语言惯例、愿望和后台形成成果。默认显示正文与认识程度，技术诊断显示完整 Ref 和实际查询计划。

可读人格和 seed 编辑调用 preview/apply，相互依赖的跨域变化一起采用。Memory 的人工修改有自己的 actor 与来源，主体之后可看到正式修改后的认识。来源限制、抑制、退役与物理清除使用不同用户动作，按其实际效果命名。

Hept 通过 Nous 的公开 Client 与可嵌入 Console 组件复用这些页面。独立 Nous 无需安装 Hept。

## 维护助手与确定性入口

维护助手使用成熟 Agent/工具执行器，授权工具来自现有管理 API。诊断、查看日志和读取配置可直接调用；删除、发布、安装、恢复等具体高影响操作根据管理员策略确认。

助手返回实际执行结果和对应操作入口，管理页面仍能完成相同任务。主体或模型离线时，CLI、Web 状态及系统服务管理继续可用。

## 日常完成场景

普通操作者能够创建主体、配置模型与预算、接入一个真实渠道、观察生活和任务、修改日历、查看一次未回复原因、阅读形成的认识、处理一次失败、重启继续并导出主体。所有步骤使用本节的正式入口，示例说明随施工同步更新。
