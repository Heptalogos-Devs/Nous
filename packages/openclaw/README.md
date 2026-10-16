# Nous OpenClaw 认知集成

[Nous 仓库地图](https://github.com/Heptalogos-Devs/Nous-Wave/blob/master/INDEX.md)

同一份插件供独立 OpenClaw 和 Heptalogos 使用。Nous 服务保存认知；插件只保存当前调用的临时 cursor/slice，OpenClaw 保存原生对话及 Subject/Session 绑定。它使用现有 Memory、WorkContext、Projection、ContextCompiler、Observation 和 ReportUse。

## 安装

需要 Node.js 24.21+、Git、OpenClaw **2026.9.1** 和已运行的 Nous 公共服务。发行物内含插件、安装器、正式入口补丁及依赖许可，不依赖 Heptalogos 或其数据库。

从 Nous 仓库构建发行物：

```sh
corepack pnpm install --frozen-lockfile
corepack pnpm openclaw:pack
```

将生成的 `data/releases/openclaw/nous-wave-openclaw-0.2.1.tgz` 交给宿主。在独立安装目录中：

```sh
npm exec --yes --package=/absolute/path/nous-wave-openclaw-0.2.1.tgz -- nous-openclaw-install /absolute/path/subject-runtime
```

原生安装器创建自己的 npm 根与持久插件 tarball，固定 OpenClaw 和经过修复的传递依赖，生成可重装的 lockfile，然后运行入口准备。只接受新目录或先前由该安装器创建的目录，不覆盖任意应用的依赖配置；配置、密钥和主体数据应放在运行代码目录之外。原 OpenClaw 2026.9.1 将 Undici/MCP SDK 固定在受影响版本，因此不能以普通 `npm install openclaw` 代替此入口。当前实际固定 Undici 8.10.2、MCP SDK 1.31.0；`prepare` 同时检查 OpenClaw 和 proxyline 实际解析的版本，旧依赖拒绝准备。

需要重复核对或准备已有受控安装时：

```sh
node /absolute/path/subject-runtime/node_modules/@nous-wave/openclaw/bin/prepare.mjs /absolute/path/subject-runtime/node_modules/openclaw
```

MCP HTTP OAuth 若已配置，升级前保存的凭据必须有正确 issuer 或重新登录；自定义 provider 保存完整 issuer，bundled provider 另需 expectedIssuer。stdio 和 SDK server 不适用此 OAuth 客户端风险。详见 [维护者安全公告](https://github.com/advisories/GHSA-6qxp-vccf-f47h)。

入口准备器核对版本和全部原始/已修正文件哈希后一次应用补丁；版本不符、代码漂移或部分补丁会拒绝安装。重复执行安全。安装目录位于其他 Git checkout 内也使用包根目录执行补丁，避免 Git 静默跳过相对路径。它不改动 operator 配置、凭据或会话。开发仓库的 `corepack pnpm openclaw:prepare [openclaw-package-root]` 使用相同安装器；缺省选择 `data/runtime/openclaw/node_modules/openclaw`。

在该实例的 OpenClaw 配置中启用：

```json
{
  "plugins": {
    "allow": ["nous-cognition"],
    "load": {
      "paths": [
        "/absolute/path/subject-runtime/node_modules/@nous-wave/openclaw"
      ]
    },
    "slots": { "contextEngine": "nous-cognition", "memory": "none" },
    "entries": {
      "nous-cognition": {
        "enabled": true,
        "config": {
          "runRoot": "/absolute/path/nous-instance/run",
          "subjectId": "00000000-0000-0000-0000-000000000001",
          "consumerId": "consumer:subject:openclaw",
          "cognitionTokenCap": 4096
        }
      }
    }
  },
  "agents": {
    "defaults": { "compaction": { "memoryFlush": { "enabled": false } } }
  }
}
```

Subject 必须已通过官方 Client/CLI 创建，Seed 可省略。consumer 必须在 Nous operator 配置中启用所需 Memory/Runtime 贡献。`runRoot` 使用受保护的 Core discovery；不要将其中的凭据授予不受信任工具。可选 `workContextId` 绑定同 Subject 的实际工作。模型、渠道、工具和 OS/节点权限继续由 OpenClaw 与宿主配置；此插件不签发管理权限。生产实例应将服务凭据与主体工具的可读范围隔离。

## 运行合同

每次原生模型调用在消息转换后、provider 发送前准备、应用认知并完成 Runtime 输入检查，覆盖首轮、工具后续、重试及原生子 Agent。缺失必需认知终止调用；不会退回 legacy 或继续发送。当前只支持受控的原生 `openclaw` harness，其他 harness 会拒绝必需模式。

RESET 替换临时 slice，APPEND 仅接受相同 track/epoch 的连续前缀。slice 不写进 transcript，原生压缩只处理 Host 历史，下一调用重新应用。关闭绑定不销毁 Subject/Session。重启后经正式 session extension 找回 Nous Session，新的 worker 使用独立 cursor；原生 session reset 建立新 Nous Session。绑定保存于原生正式插件命名空间，按实际 reset entry 区分历史代际，兼容原生复用物理 sessionId 的 reset。进程卸载注册表保留持久扩展，显式禁用或删除仍按原生生命周期清理。

发送前使用 `ReportUse.model_input` 核验当前来源与安全历史，服务必须明确确认已执行检查；即使没有认知片段也不能跳过。模型完成、失败或取消按实际 outcome 另记 Observation。purge、suppression 或 withdrawal 将实际使用这些认知的 Session 历史标为失效；子会话保留父 Session 关系，沿父链执行同一检查。Core 或 Gateway 重启不会清除标记。旧输出或原生压缩摘要可能带有被清除的内容，因此旧 Session 终止后续模型发送，需先执行正式 native reset。该操作保留 Subject、当前认知及 WorkContext；不是把 RESET slice 当作历史清除。consumer 禁止此前进入历史的认知域时也须使用干净 Session。

原生工具、渠道、Computer use、skills 与子 Agent 保留。子 Agent 使用同一 Subject 和独立 Nous Session，以隔离任务交接，不能自行 fork 父历史。受众来自可信原生调用信息；没有解析成人物的 sender 保持 unresolved，不伪造 Entity。并行执行者各有 binding。

用户/模型内容按接触记录 Observation，实际工具结果保留结构与错误标志并进入经历。模型输出只记为 `model_output`，不冒充已外发。ReportUse 的 `presented` 在输入提交前记录已经纳入待发送输入的 exact refs；传输失败不被记为模型选择、动作或外发成功。普通“记住/忘记”不获得认知 mutation/purge 工具。

当前采用认知时不加载可写 SOUL/IDENTITY/USER/MEMORY 作为第二份状态。Self 使用同一正式贡献与输入路径，插件不会合成个人经历。Self 通过正式领域操作形成、反思与修订；普通对话不直接修改持久状态。框架无关 API 见 [`@nous-wave/client/managed`](https://github.com/Heptalogos-Devs/Nous-Wave/blob/master/packages/client/src/managed.ts)，产品合同见 [Managed OpenClaw](https://github.com/Heptalogos-Devs/Nous-Wave/blob/master/docs/specs/active/cognitive-runtime/openclaw.md)。

## Host Life 与结构化接触

同一正式插件可选配置 `hostBridge`，不需要 Host 复制插件。配置由操作者提供：

```json
{"hostBridge":{"origin":"http://127.0.0.1:19090","credentialFile":"/absolute/private/host-bridge.token"}}
```

origin 只接受配置的 IPv4/IPv6 loopback HTTP origin，拒绝 URL 凭据、路径与重定向。credentialFile 为绝对、owner-only 私密文件，内容是一行 Bearer token；只有插件 Node 端读取，不进入公开 schema 模块、prompt 或日志。每次请求实际等待 Host；拒绝、离线、取消及缺少可信上下文都会阻止 provider。未配置 Host 的独立安装使用同一插件和 v3 原生入口。

公开 wire 的唯一 owner 是官方 Client `@nous-wave/client/host-bridge`，导出 `HOST_BRIDGE_PATHS`、Zod schemas、相应 TS 类型和 `HOST_BRIDGE_JSON_SCHEMAS`，该入口不依赖 Node。Host 直接使用这些合同生成 OpenAPI/校验请求，不复制另一组认知 DTO。四个 POST 路径为 `/nous/v1/prepare`、`admitted`、`handoff`、`feedback`。

prepare 携带真实 native Session key/ID、run/agent/history generation、父 key、Nous Session/Subject、invocation、稳定 contact request ID 与原始 prompt 的 SHA256，不转储原 prompt。Host 先持久受限 NativeRun/Activity，再按已知 run/digest/session/scope 许可。返回当前 `workContextId`、canonical `CognitionUsage` Protobuf JSON、`contacts[{contactRef,observation}]` 与 Activity/actor scope。插件严格 `fromJson` 并核对 Subject/Session、原生 agentInstanceId、稳定 observation requestId 和实际 observedAt；Host audience/sourceRead 原样进入正式 prepare。每次调用都重新设置当前 WorkContext；静态配置不能覆盖 Host 的活动。

Hosted user 接触只使用 Host 的结构化 ObservationInput，不再将工程 prompt/QQ 上下文包另记成 `object:openclaw` 根。Host 保留实际作者、会话、MessageFact provenance、事件 occurred 与现实 observed；重放既有 contact 使用首次实际接触时间。admitted 回调使用同一 contactRequestId，回传实际 Projection 与 occurrence receipts。子 Agent 在正式 isolated handoff 中等待 Host 保存受限继承，拒绝时关闭已开的 Nous child Session；不借此授予维护或 QQ 发送权限。

工具/assistant 实际结果通过反馈合同回流：Host 先持久 `stage=pending`，返回第一次保存的 canonical ObservationInput，其 context 保存公开 `HOST_ACTIVITY_CONTEXT_KEY`（`activity_ref`）、`HOST_CONTACT_CONTEXT_KEY`（`contact_request_id`）与 `HOST_COGNITION_USAGE_CONTEXT_KEY`（`cognition_usage`，canonical encoded usage），关联每个来源的原 Activity/实际 scope；Host 用 `withHostObservationScope` 一次构造并持久，重放不换成新 scope；插件提交 Nous 后以 `stage=committed` 回传真实 receipt。Host 负责 pending 补交，复用公开 `/managed` 的 `outcomeObservation` 构造函数及稳定 ID。实际 model_output 不代表渠道发送。

v3 在原生 `message_end` 写入前同步附上非秘密 contact identity、首次现实 observedAt 与实际 outcome/projection 标记；原生先保存真实结果，再尝试回流。回写失败不把已发生动作变成重新执行/重新发送。下一全新 user turn 与 compact 前通过正式 SDK 读取完整 SQLite transcript，按原 contact/scope 补交带标记的结果，失败则拒绝后续模型/compact。原生 reset 为保留历史的 boundary；不清掉唯一副本。插件只有临时 cursor/已确认集合，不另建数据库、队列或重试模型循环。

准备安装会统一升级到 boundaryVersion **3**；旧 native 入口明确拒绝，需重新运行正式 prepare。自动测试使用真实 loopback Host 与 mock Nous 公共操作检查上述许可/来源/恢复边界；离线 native fixture 另验证真实 SQLite 保留 contact 标记及 reset 历史。真实 Host、Activity 与 QQ 全流程仍由产品集成验证。

## 宿主控制的渠道派发

受控渠道在 `ChannelPlugin.message.deliveryCustody` 实现必需的 `adopt` 与
`authorize`。类型从 `openclaw/plugin-sdk/channel-message` 或
`channel-outbound` 导出。未声明此接口的通用渠道保持原生行为。

`adopt` 在最终 policy/normalization 之后、durable queue 写入之前接收原生
run/execution identity/session、目标、回复事实与 prepared batch。宿主必须核验真实
Activity/Behavior 的当前采纳范围；缺少可信来源时拒绝，不能从模型正文或 metadata
补造权限。返回非秘密的 `{ reference }`，它位于原生 prepared batch 顶层，随现有
SQLite queue 保全。统一入口将 runtime-owned message 的 `skipQueue` 分支纳入同一
`queuePolicy=required` 队列，稳定 intent 来自原生既有 `deliveryIntentId` 或
`mirror.idempotencyKey`；缺失时拒绝，不从正文补造。默认使用原生 permanent
completion retention 防止已结清 intent 重复发送；不能在 queue 写入失败后直接发送。
旧队列迁移不得重新取得采纳，恢复只使用原 reference。

每个 text/media/payload 及原生分段的 adapter handoff 前，原生等待 `authorize`，
传入 reference、已保存 batch、实际本段正文/媒体、原始 `sourceIndex`、回复目标和原生 queue 身份；拒绝会
阻止协议调用。实际 Channel send context 暴露同一 `sourceIndex`，用于关联授权与回执；prepared batch 的原始索引经 SQLite 恢复保持不变，相同正文不被合并或按正文猜测。宿主每次核验当前取消/停止、受众、载荷、来源范围和会话发送权限；
observe-only 会话由宿主及第一方 Channel 拒绝外发，通用层不硬编码平台账号。
Channel 内的媒体子发送、编辑/撤回及自定义 actions 也必须复用同一宿主授权执行器，
不能注册原始协议工具或直发旁路。原生未知发送、completion 与回执语义保持不变，
Host 回写失败不成为重新发送的理由。

默认 Vitest 与 `check:fast` 自动执行离线 native custody 检查：

```sh
pnpm openclaw:custody:verify
```

它从 workspace 固定 OpenClaw 依赖拷贝运行代码到测试临时根，使用正式 prepare，
不联网安装或修改原生依赖/操作者实例。已有受控包也可手动运行同一 fixture：

```sh
pnpm exec tsx scripts/tests/openclaw/custody.ts /absolute/path/openclaw-package-root
```

此场景使用已安装生产函数和实际原生 SQLite queue，检查稳定 intent 去重、reference
保全、恢复不重新采纳、逐段授权、取消和 observe-only 拒绝及普通渠道行为。它不证明
QQ、真实 Host API 或模型交互；这些由产品集成验证。

## 验证

```sh
node --env-file=/absolute/path/operator.env node_modules/tsx/dist/cli.mjs scripts/smoke/openclaw.ts /absolute/path/openclaw.json /absolute/path/openclaw-package-root
```

该公共场景使用现有 loopback HTTP 模型 gateway 的真实 provider；创建隔离 Nous/Core/Kernel/PostgreSQL 与原生 Gateway，以 `/readyz` 判断就绪。检查实际 provider 输入、读文件工具结果、WorkContext RESET、原生压缩、子 Agent、认知不可用时阻止发送，Core 重启后恢复，以及实际 Self 输出进入原生压缩摘要后的 purge、Gateway 重启拒绝发送和正式 reset 后的干净输入。诊断与失败输入留在私有临时目录，不输出凭据。它不替换 operator 实例或证明未测渠道/平台能力。

Self 的真实行为场景单独运行，不进入默认离线 smoke：

```sh
node --env-file=/absolute/path/operator.env node_modules/tsx/dist/cli.mjs scripts/smoke/openclaw-self.ts /absolute/path/openclaw.json /absolute/path/openclaw-package-root
```

场景使用同一 provider 的首个模型执行原生交互，最后一个模型执行正式 Self 形成/反思并检验切换，配置中需要两个不同模型。它保留无 Seed 主体的实际自选名字和有情境偏好，通过实际公共资料任务检查使用，再对照 Seed 主体、来源修订、重启、新会话及普通聊天权限。Self 解释使用显式的 180 秒执行上限；模型选择需具备结构化输出及语义判断能力，结构合法不代表形成结果正确。需检查私有结果中的实际陈述、适用条件、工具步骤与最终输入，不能用“回答出现名字”代替认知应用证明。

场景失败后可在末尾传入其 evidence 目录继续，同一 Subject、原始来源、付费解释结果和原生会话都会保留。诊断目录不应加入版本控制。
