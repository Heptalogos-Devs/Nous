# Nous OpenClaw 认知集成

同一份插件供独立 OpenClaw 和 Heptalogos 使用。Nous 服务保存认知；插件只保存当前调用的临时 cursor/slice，OpenClaw 保存原生对话及 Subject/Session 绑定。它使用现有 Memory、WorkContext、Projection、ContextCompiler、Observation 和 ReportUse。

## 安装

需要 Node.js 24.21+、Git、OpenClaw **2026.9.1** 和已运行的 Nous 公共服务。发行物内含插件、安装器、正式入口补丁及依赖许可，不依赖 Heptalogos 或其数据库。

从 Nous 仓库构建发行物：

```sh
corepack pnpm install --frozen-lockfile
corepack pnpm openclaw:pack
```

将生成的 `data/releases/openclaw/nous-wave-openclaw-0.2.0.tgz` 交给宿主。在独立安装目录中：

```sh
npm install --prefix ./subject-runtime openclaw@2026.9.1 /absolute/path/nous-wave-openclaw-0.2.0.tgz
node ./subject-runtime/node_modules/@nous-wave/openclaw/bin/prepare.mjs ./subject-runtime/node_modules/openclaw
```

安装器核对版本和全部原始/已修正文件哈希后一次应用补丁；版本不符、代码漂移或部分补丁会拒绝安装。重复执行安全。安装目录位于其他 Git checkout 内也使用包根目录执行补丁，避免 Git 静默跳过相对路径。它不改动 operator 配置、凭据或会话。开发仓库的 `corepack pnpm openclaw:prepare [openclaw-package-root]` 使用相同安装器；缺省选择 `data/runtime/openclaw/node_modules/openclaw`。

在该实例的 OpenClaw 配置中启用：

```json
{
  "plugins": {
    "allow": ["nous-cognition"],
    "load": {"paths": ["/absolute/path/subject-runtime/node_modules/@nous-wave/openclaw"]},
    "slots": {"contextEngine": "nous-cognition", "memory": "none"},
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
    "defaults": {"compaction": {"memoryFlush": {"enabled": false}}}
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

当前采用认知时不加载可写 SOUL/IDENTITY/USER/MEMORY 作为第二份状态。Self 使用同一正式贡献与输入路径，插件不会合成个人经历；完整 Self 行为场景仍在该功能分支验证。框架无关 API 见 [`@nous-wave/client/managed`](https://github.com/Heptalogos-Devs/Nous-Wave/blob/master/packages/client/src/managed.ts)，产品合同见 [Managed OpenClaw](https://github.com/Heptalogos-Devs/Nous-Wave/blob/master/docs/specs/active/cognitive-runtime/openclaw.md)。

## 验证

```sh
node --env-file=/absolute/path/operator.env node_modules/tsx/dist/cli.mjs scripts/smoke/openclaw.ts /absolute/path/openclaw.json /absolute/path/openclaw-package-root
```

该公共场景使用现有 loopback HTTP 模型 gateway 的真实 provider；创建隔离 Nous/Core/Kernel/PostgreSQL 与原生 Gateway，以 `/readyz` 判断就绪。检查实际 provider 输入、读文件工具结果、WorkContext RESET、原生压缩、子 Agent、认知不可用时阻止发送，Core 重启后恢复，以及实际 Self 输出进入原生压缩摘要后的 purge、Gateway 重启拒绝发送和正式 reset 后的干净输入。诊断与失败输入留在私有临时目录，不输出凭据。它不替换 operator 实例或证明未测渠道/平台能力。
