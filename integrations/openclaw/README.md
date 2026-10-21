# OpenClaw 认知接入

`nous-cognition` 在每次真实模型输入前准备认知，在原生保存终端输出后接纳接触。OpenClaw 保留模型与工具循环、历史、压缩、子代理和沙箱。每次普通调用、工具后续调用与辅助摘要持有独立 callId；其 UUIDv7 后缀直接作为正式 ModelInvocation 身份。

## 构建与安装

固定源码与许可证见 [source.json](upstream/source.json) 和 [LICENSE](upstream/LICENSE)。[model-context.patch](upstream/model-context.patch) 改编实际调用宿主、原生会话与归档读取、发布模型目录和插件合同。在已安装工作区依赖的 Nous 仓库运行：

```sh
node integrations/openclaw/build-upstream.mjs
node integrations/openclaw/build.mjs
```

第一条默认把固定源码、缓存与 npm 制品写入 `../temp/spec-research/native-release/`，调用选定上游的 `build-all package` 和正式 pack 入口。可用 `--source PATH --output PATH --cache PATH --profile package` 指定位置与官方阶段；`--build-only` 只构建，`--skip-build` 从已有正式产物打包。声明编译要求依赖物理位于该 checkout 内，不能使用外部 node_modules 链接。完整运行图使用上游实测的 4352 MiB Node heap；Linux 本机加 `--systemd-scope` 限制 6 GiB 内存、512 MiB swap、2 核，CI 由容器施加同一硬限。第二条默认生成 `../temp/spec-research/native-release/plugins/nous-cognition/`，用 `--output DIRECTORY` 指定插件目录。先通过正常 npm 安装入口安装制品及运行依赖，再安装插件。插件要求 `modelContext.version=1` 与发布模型元数据 API。

## 空 Gateway 检验

在独立安装目录安装正式制品后，使用 [smoke-upstream.mjs](smoke-upstream.mjs)：

```sh
node integrations/openclaw/smoke-upstream.mjs --root /absolute/path/to/node_modules/openclaw --nous-plugin /absolute/path/to/nous-cognition --hept-plugin /absolute/path/to/hept-worker --nous-installation-id '<actual-nous-bootstrap-id>' --output /absolute/path/to/temporary-smoke
```

命令在新的临时状态目录启用两个插件，使用实际 Nous 引导身份、空绑定和空 Subject 表；认证令牌只在子进程环境中。它检验正式 `/healthz` 和两个已认证插件路由，随后关闭进程组。该步骤不派发模型；实际 Task、工具、历史与认知另由纵切面场景验证。日志与结果留在选定输出目录，日志移除本次 Gateway 令牌。

## 独立 Nous 配置

OpenClaw `plugins.entries.nous-cognition`：

```json
{
  "enabled": true,
  "hooks": { "allowConversationAccess": true, "allowPromptInjection": true, "timeoutMs": 120000 },
  "config": {
    "runtimeId": "openclaw-local",
    "baseURL": "http://127.0.0.1:4333",
    "tokenEnv": "NOUS_SERVICE_TOKEN",
    "installationId": "<actual-nous-installation-id>",
    "modelResources": { "<provider>/<model>": "<registered-nous-model-resource-id>" },
    "bindings": {
      "main": {
        "authorityRef": "delegation:<registered-leaf-id>",
        "subjectId": "<subject-id>",
        "consumer": { "kind": "worker", "host": "openclaw-local", "id": "main" },
        "purpose": "worker",
        "audience": [],
        "billing": "nous"
      }
    }
  }
}
```

`installationId` 是目标 Nous 安装身份。binding 指向已登记的叶委派；Consumer、Subject、用途和听众与该委派一致。父 Host 令牌由 SecretRoot 或宿主环境供给，认知请求通过 `x-delegation-authority` 选择叶委派。材料范围和当前授权由 Nous 真实 owner 重验。SDK 和令牌只在本次请求内使用。

`nous.native` 安装配置登记同一 runtime 的 Gateway endpoint、SecretRef、网络 profile 和 agent→authorityRef。模型资源登记真实 native binding；`modelResources` 映射实际 provider/model，辅助摘要使用其他模型时也需显式登记。原生角色使用 `native_agent`，不替换 OpenClaw 自身 Prompt。

## 联合 Hept 配置

专业 Task 安装 [hept-worker](https://github.com/Heptalogos-Devs/Heptalogos/tree/feat/spec-rebuild/integrations/openclaw)。对应 agent 的认知 binding 只写：

```json
{ "billing": "external" }
```

四类认知操作通过现有 Gateway 方法 `hept.cognition.capture/prepare/accept/uploadBlob` 交给 Hept 插件。Hept 依据真实 session/run 和 Task 读取同一个 Worker，再用保存的 Nous 子委派访问正式认知 API。首次材料回执提供实际 Consumer 和 Subject。该模式不读取 Nous Host 令牌，逐调用费用只记在 Hept 的正式模型账本。

## 来源、回执与恢复

prepare 位于原生投影和压缩之后。插件保存当前新增输入的有界文本及原图字节，已有历史来源通过 `requiredSources` 复验。稳定 session 文档是来源根，callId 与 transcript cursor 表示修订。原生 messages、tools 和系统说明继续由原生宿主保存；选定认知进入一次性 slot。

原生先保存 PREPARED，再由正式模型 owner 预留并记派发，随后交回原生 provider。终端输出持久后，高优先级会计先保存实际 usage/费用；认知成功返回的 outputRef 写回同一原生终端 entry，低优先级会计再采用该精确引用。辅助摘要使用同一原生 custom entry。重放已持久的接纳回执时沿用原 outputRef，避免重复认知。认知失败时保留已经发生的调用事实。

恢复读取原 callId 的完整终端响应，完成同一接纳后才允许新调用。未派发调用记 not_sent；已派发无完整响应或结果 unknown 保持待核实。`sessions.modelCall` 经原生会话 owner 读取精确 UUID，并沿 retained window、压缩引用和原生归档验证旧 UUID 关系；终态旧调用可恢复，归档调用不能重新派发。接口不返回 Prompt、工具正文、输出正文或凭据。

显式工具恢复由 `planRecovery/recover` 沿原生 command、会话锁和执行循环完成。计划要求原生失败记录具有 `openclaw_model_context_accept_failed` 及对应 callId 的结构化来源，实际 DISPATCHED，且同步工具尚未启动。缺少结构证明返回 `native.recovery_proof_unavailable`；分支已发布后的不确定执行窗口返回 `native.recovery_effect_unknown`，由原生工具 owner 核实。恢复计划采用后，重复旧计划返回 `native.recovery_plan_changed`。原始消息和失败分支保存在原生历史。

受影响源码回归：

```sh
node_modules/.bin/vitest run src/agents/published-model-binding.test.ts src/gateway/server-methods/sessions-model-call.worker.test.ts src/agents/embedded-agent-runner/run/attempt.model-context.test.ts src/gateway/server-methods/sessions-get.worker.test.ts src/gateway/server-methods-list.test.ts src/gateway/agent-turn/agent-wait-dedupe.test.ts --pool=forks --maxWorkers=1 --no-file-parallelism
```
