# OpenClaw 认知接入

`nous-cognition` 在实际模型边界准备认知，在原生输出持久保存后接纳接触。普通工具循环、压缩摘要、分支摘要及工具后续调用分别取得稳定 callId。原生历史、工具、子代理、执行与压缩继续由 OpenClaw 管理。consumer=worker 的接触留在工作者范围；主体查看成果时才形成主体经历。

## 构建与安装

基线与许可证见 [source.json](upstream/source.json) 和 [LICENSE](upstream/LICENSE)。[model-context.patch](upstream/model-context.patch) 直接修改 agent-core、原生会话、embedded 调用宿主、插件合同、HTTP trace 接纳与工具授权责任位置。

在已安装工作区依赖的 Nous 仓库运行：

```sh
node integrations/openclaw/build-upstream.mjs
node integrations/openclaw/build.mjs
```

第一条从固定源码应用补丁，以官方 `build-all package` 和自包含打包入口生成 `data/native/openclaw/` 正常 npm 包；包含官方包校验。使用 Linux systemd 用户 scope，将完整构建限制为 6 GiB 内存、512 MiB swap、2 核，Node 堆限制为 2 GiB。下载并发为 4。安装后使用普通 OpenClaw 启动入口。

第二条使用工作区固定的 esbuild 产出独立插件 `data/native/plugins/nous-cognition/`，运行依赖已捆绑。先安装改编后的 OpenClaw 包，再通过原生插件安装入口安装该产物目录。插件要求宿主公开 `modelContext.version=1`。

## 配置

OpenClaw `plugins.entries.nous-cognition`：

```json
{
  "enabled": true,
  "hooks": { "allowConversationAccess": true, "allowPromptInjection": true, "timeoutMs": 120000 },
  "config": {
    "baseURL": "http://127.0.0.1:4333",
    "tokenEnv": "NOUS_SERVICE_TOKEN",
    "installationId": "local-openclaw",
    "modelResources": { "<provider>/<model>": "<registered-nous-model-resource-id>" },
    "bindings": {
      "main": { "subjectId": "<subject-id>", "consumer": "worker", "consumerId": "main", "purpose": "worker", "audience": [], "billing": "nous" }
    }
  }
}
```

服务令牌从 SecretRoot 或宿主环境供给。服务身份授予对应主体的材料、上下文、接触与模型调用权限。`modelResources` 映射实际提供方和模型至正式资源，压缩使用不同模型时也须登记。联合运行由 Hept 承担专业任务费用时，该绑定改用 `billing:"external"`；Hept 插件仍在同一原生派发边界执行预算与计费。

原生宿主提供 agentId、父会话与调用身份；`consumerId` 与服务凭据 metadata 的 consumer 一致，子代理沿父会话使用同一工作范围。材料 scope 由服务可信入口确定。用途和听众来自受信绑定。调用使用正式 Nous Client，传送用途、听众、稳定幂等键与 traceparent。

## 接触、来源与恢复

prepare 发生在原生投影和压缩后。只将本次新增原生输入的有界片段作为当前材料，图像原始字节通过正式 blob 操作保存。整个历史仍由原生宿主持有。已有历史的来源引用通过 `requiredSources` 重新检查当前政策；原生压缩摘要保留这些依赖。外部 session 文档作为稳定来源根，调用与原生 transcript cursor 表示其修订。

选定认知仅占本次临时 slot。真实调用前，原生会话先保存 prepared/dispatch 身份，正式费用 owner 再预留并标记派发；随后交回原生 provider。成功输出完成原生保存后，accept 保存具有直接输入依赖的输出材料，结算实际用量并接纳 Encounter。辅助摘要的输出保存在同一个原生会话的 custom entry。

接受失败保留原生输出。恢复读取同 callId 的完整终端响应，先补接纳，再允许新调用。未派发的 prepared 调用可退役；预算或权限拒绝发生在 provider 调用前，持久记录 not-dispatched 并结算 not_sent；已派发而没有完整响应，或已经接纳为 unknown 的调用，保持阻断直到核实原结果。当前段落输出不视为终端结果，所有接纳重投使用同一原生响应。

上游补丁包含两个连续场景：实际 agent 工具循环与模型切换；原生 SQLite 会话恢复、辅助摘要、压缩来源继承和 unknown 阻断。运行：

```sh
node_modules/.bin/vitest run packages/agent-core/src/model-context.test.ts src/agents/embedded-agent-runner/run/attempt.model-context.test.ts --pool=forks --maxWorkers=1 --no-file-parallelism
```
