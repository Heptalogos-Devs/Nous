# OpenClaw 认知接入

`nous-cognition` 在每次真实模型输入前准备认知，在原生保存终端输出后接纳接触。OpenClaw 保留模型与工具循环、历史、压缩、子代理和沙箱。每次普通调用、工具后续调用与辅助摘要持有独立 callId；其 UUIDv7 后缀直接作为正式 ModelInvocation 身份。

## 构建与安装

固定源码与许可证见 [source.json](upstream/source.json) 和 [LICENSE](upstream/LICENSE)。[model-context.patch](upstream/model-context.patch) 改编实际调用宿主、原生会话与归档读取、发布模型目录和插件合同。在已安装工作区依赖的 Nous 仓库运行：

```sh
node integrations/openclaw/build-upstream.mjs
node integrations/openclaw/build.mjs
```

第一条用官方 `build-all package` 和自包含打包入口生成 `data/native/openclaw/` npm 包，完整构建限制为 6 GiB 内存、512 MiB swap、2 核。第二条生成 `data/native/plugins/nous-cognition/`。先通过正常包安装入口安装改编后的 OpenClaw，再安装插件。插件要求 `modelContext.version=1`。当前补丁的受影响源码场景已经通过；完整包构建由发行步骤执行。

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

专业 Task 安装 [hept-worker](../../../Heptalogos/integrations/openclaw/README.md)。对应 agent 的认知 binding 只写：

```json
{ "billing": "external" }
```

四类认知操作通过现有 Gateway 方法 `hept.cognition.capture/prepare/accept/uploadBlob` 交给 Hept 插件。Hept 依据真实 session/run 和 Task 读取同一个 Worker，再用保存的 Nous 子委派访问正式认知 API。首次材料回执提供实际 Consumer 和 Subject。该模式不读取 Nous Host 令牌，逐调用费用只记在 Hept 的正式模型账本。

## 来源、回执与恢复

prepare 位于原生投影和压缩之后。插件保存当前新增输入的有界文本及原图字节，已有历史来源通过 `requiredSources` 复验。稳定 session 文档是来源根，callId 与 transcript cursor 表示修订。原生 messages、tools 和系统说明继续由原生宿主保存；选定认知进入一次性 slot。

原生先保存 PREPARED，再由正式模型 owner 预留并记派发，随后交回原生 provider。终端输出持久后，高优先级会计先保存实际 usage/费用；认知成功返回的 outputRef 写回同一原生终端 entry，低优先级会计再采用该精确引用。辅助摘要使用同一原生 custom entry。认知失败时保留已经发生的调用事实。

恢复读取原 callId 的完整终端响应，完成同一接纳后才允许新调用。未派发调用记 not_sent；已派发无完整响应或结果 unknown 保持待核实。`sessions.modelCall` 经原生会话 owner 读取精确 UUID，并沿 retained window、压缩引用和原生归档验证旧 UUID 关系；终态旧调用可恢复，归档调用不能重新派发。接口不返回 Prompt、工具正文、输出正文或凭据。

受影响源码回归：

```sh
node_modules/.bin/vitest run src/agents/published-model-binding.test.ts src/gateway/server-methods/sessions-model-call.worker.test.ts src/agents/embedded-agent-runner/run/attempt.model-context.test.ts src/gateway/server-methods/sessions-get.worker.test.ts src/gateway/server-methods-list.test.ts src/gateway/agent-turn/agent-wait-dedupe.test.ts --pool=forks --maxWorkers=1 --no-file-parallelism
```
