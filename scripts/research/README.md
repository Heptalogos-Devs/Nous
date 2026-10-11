# 真实模型与资料研究

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

真实模型调用需明确运行实例、资料和预算。生成物属于 ignored `data/research/`；稳定研究事实由 [研究入口](../../docs/research/README.md) 路由。

| 文件 | 作用 |
| --- | --- |
| [serve-gateway.ts](serve-gateway.ts) | 显式启动有持久调用上限的研究代理 |
| [gateway.ts](gateway.ts) | run-owned 代理、并发调用计数和 telemetry |
| [gateway-trace.ts](gateway-trace.ts) | 有界 capture、媒体 digest 和凭据清除 |
| [model-call-guard.ts](model-call-guard.ts) | 可恢复的研究调用预算 |
| [media.ts](media.ts) | 继续媒体导入、派生、formation 和 retrieval 实验 |
| [longitudinal.ts](longitudinal.ts) | 单次纵向 proposal 的真实模型研究 |
| [model-contracts.ts](model-contracts.ts) | 离线导出生产合同、Prompt 与身份 |
| [model-trace.ts](model-trace.ts) | 读单次真实 wire attempt，按当前合同解释 |
| [dogfooding.ts](dogfooding.ts) | 用生产 Core bootstrap 提供可变 LocalDocuments 研究宿主 |

Dogfooding 宿主的执行任务和实际结果见 [Full-System Dogfooding](../../docs/research/full-system-dogfooding-2026-10-08.md)；它复用标准 Resource adapter。Gateway 的安全边界与预算测试位于 [scripts/tests](../tests/README.md)。

## Live research

手动媒体与模型研究使用真实模型，必须先准备语料、运行实例和分发的 Client 模块。自动功能验收使用 deterministic provider。方法与数据说明见 [Research](../../docs/research/README.md)。

```text
corepack pnpm research:gateway --ledger data/research/runs/run-ledger.json --max-calls 1000
```

Gateway 默认转发 `http://127.0.0.1:3000/v1`，监听端口 18000；用 `--upstream`、`--port` 覆盖。`--ledger` 与 `--max-calls` 必填，显式累计调用上限允许 1–100000，预算计入失败和重试，持久化在 ledger；同路径旁保存 telemetry。将实例模型 endpoint 配为该 gateway 后再运行实验。 可加 `--trace-root data/research/runs/<run>/traces` 保存每次 attempt 的 `meta.json` 和经过敏感信息清除的 request/response。request capture 上限为 96 MiB，response 为 1 MiB；超限只保存大小、digest 和 truncation 状态。大型 data/base64 媒体与 multipart file 保存 media type、byte count 与 SHA-256 描述符；multipart 保留 model/language 等普通字段，binary 不作为文本落盘。Authorization、cookie 和 credential header 不落盘，已知凭据回显也在写盘前清除。trace 文件使用 0600 权限。

```text
corepack pnpm research:media-live --run-root <实例run目录> --client-module <分发client模块> --unit nasa-menon-conversation --strategy direct_structured --derive-only
```

Media 默认读取 `docs/research/corpus/media.json` 与 `data/research/corpus/raw`，将处理状态写到 `data/research/runs/media-state.json`；用 `--manifest`、`--raw-root`、`--state` 覆盖。状态文件用于继续已有实验，不会从头重复已完成操作。

媒体 runner 的 `--unit`、`--strategy` 可重复指定，`--derive-only` 只验证派生与来源图，`--skip-retrieval` 执行派生和 formation、暂不准备 embedding/query。`--allow-degradation <code>` 可重复声明实验预期的降级（例如 frames-only 的 `video_audio_not_interpreted`），结果仍保存全部 degradation；其他降级继续使 pipeline 失败。Receipt 与完整 manifest digest 绑定；更换语料用新的 state。人工评阅应核对实际输出，pattern 命中只覆盖预先声明的事实。Manifest 的 `oracle_patterns_by_representation_kind` 可为 Transcript 等表示声明与其职责相符的事实 oracle，未声明时使用共同 `oracle_patterns`。

## 模型合同与 trace 检查

```text
corepack pnpm inspect:model-contracts --all
corepack pnpm inspect:model-contracts --role memory_formation --config data/config/apps/nous.toml --output data/research/inspection/formation
corepack pnpm inspect:model-contracts --role material_description --prompt-path material/video-description.md --adapter gateway-chat-media-v1 --config data/config/apps/nous.toml --output data/research/inspection/video
corepack pnpm inspect:model-trace --root data/research/runs/<run>/traces --attempt 1
```

合同检查消费 production invocation 同一 Structured Contract Registry 和 PromptRegistry，导出 `contract.json`、实际 provider `schema.json` 与 `prompt.md`；未使用 Structured Output 的角色没有 schema 文件。配置可选；提供时导出无凭据的 model/protocol、profile 与 role configuration digest。`--prompt-path`（须配 `--role`）选择实际 invocation 的 Prompt override，`--adapter` 计入生产 invocation 的 adapter identity；两者沿用 production digest owner。该命令不调用模型，也不要求读取 credential 值。生成文件只能写入 ignored `data/research/`，不形成第二份 schema source。

Trace 检查显示 endpoint、model、状态、usage、wire digest、capture 状态及文件路径，并从捕获的 Prompt/schema 确定性匹配当前角色。`--prompt-path` 与 `--override-prompt-root` 可用于 custom Prompt；视频默认 Prompt 自动参与匹配。embedding/rerank/transcription 按实际 endpoint 识别角色。模型 JSON 输出可以对当前匹配的 Zod owner 复验；原 invocation 的 owner commit/degradation 仍由对应研究 runner 的实际结果确认。更换 Prompt/schema 后，旧 trace 不会伪称匹配当前合同。

## 纵向模型 proposal

```text
corepack pnpm research:longitudinal --config data/config/apps/nous.toml --input data/research/longitudinal/plan.json --role journal_synthesis --output data/research/longitudinal/journal-proposal.json
```

`--input` 使用 Kernel `PlanMaintenance` 返回的 ProtoJSON `MaintenancePlan`：包含当前 Subject、exact source revisions、ordered member keys、support/entity/candidate catalogs 和 owner snapshot。输入必须为 `ready` 且不超过 256 KiB；适用的调用方从私有 Kernel 请求取得该快照。`--role` 为 `episode_segmentation`、`journal_synthesis` 或 `memory_consolidation`。配置使用当前 `nous.toml`；gateway credential 来自已设置的配置指定环境变量。`--prompt-root` 默认 `prompts`，`--override-prompt-root` 可选择本地 prompt override。

Runner 通过真实配置角色和 canonical Structured Output 生成一次 proposal，校验 catalog keys，将来源计划、proposal、producer identity 与人工审阅项目写入新的本地文件；已有 output 文件会报错。研究产物放在 ignored `data/research/`。人工审阅使用真实 trace、来源事实和 boundary annotations，分别评估分段边界、Journal point 支持与省略、整合身份和 Schema 泛化。确定性 `smoke:longitudinal` 检查编排与 Authority 合同；质量研究使用这个手动入口。

## 研究结果与持续使用

一次性 Core Cognition qualification 和 cognitive-functional synthetic runner 已退役，专属 deterministic provider、场景/多 profile 回放和可选 smoke 分支一并删除。[方法](../../docs/research/core-cognition-semantic.md)、[v1 报告](../../docs/research/core-cognition-2026-10-07.md)、[v2 报告](../../docs/research/core-cognition-2026-10-08.md)、[功能语料](../../docs/research/corpus/functional/README.md)和[六项 text-only 选择](../../docs/research/corpus/text-compatibility-selection.json)继续保留原输入、oracle、参数和历史观察。

当前真实任务通过 [Agent 指南](../../docs/agent/README.md)中的 CLI/MCP 操作执行，实际 formation、trace、Query、use 与 Portable 重启结果见[重整观察](../../docs/research/deep-rebase-2026-10-09.md)。模型合同导出、单次 wire trace、媒体与纵向 proposal 研究入口仍供当前操作使用。确定性 longitudinal public smoke 保留有界维护、owner 提交、重放与重启检查；它不回放旧 synthetic corpus，也不代表真实模型质量。
