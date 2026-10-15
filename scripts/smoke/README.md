# 公共接口确定性场景

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

这些场景使用真实 Core/Kernel/PostgreSQL 和官方 Client，受控模型替身用于检查编排与 Authority。实际模型质量和付费调用在 [Research](../research/README.md) 中执行。

| 文件                                                     | 场景                                 |
| -------------------------------------------------------- | ------------------------------------ |
| [memory.ts](memory.ts)                                   | Memory、使用反馈、重启和生命周期     |
| [runtime-episode.ts](runtime-episode.ts)                 | WorkContext/Session 延续与 Episode   |
| [model-material-resource.ts](model-material-resource.ts) | 模型、材料、标准 Resource 接纳与恢复 |
| [media-routes.ts](media-routes.ts)                       | 相反媒体 route、来源通道和重放       |
| [longitudinal.ts](longitudinal.ts)                       | Observation 到纵向维护与重启轨迹     |
| [configuration.ts](configuration.ts)                     | 配置规范化、scope、CAS/replay 和重启 |
| [client.ts](client.ts) | 仓库外安装官方 Client，严格类型检查与 status/Subject 操作 |
| [self-selection.ts](self-selection.ts) | 超过 100 项的管理分页、完整依据、身份与有界反思；可选真实 native 验证 |
| [support.ts](support.ts)                                 | 场景共用的隔离实例和 Core 生命周期   |

## Public smoke

```text
just smoke
corepack pnpm smoke:memory
corepack pnpm smoke:runtime
corepack pnpm smoke:model
corepack pnpm smoke:media
corepack pnpm smoke:longitudinal
corepack pnpm smoke:configuration
corepack pnpm smoke:client
```

`just smoke` 先构建 Kernel，再顺序执行 `package.json` 中的 aggregate 场景；纵向场景由 `just check` 的 Rust integration test 执行一次；单独调用要求 debug Kernel 和 PostgreSQL runtime 已准备。

- memory：Memory 创建、检索、使用、重启与生命周期。
- runtime：WorkContext/Session 延续与 Episode exact revision。
- model：本地模型/资源 host 下的 Model、Material、External Resource 组合。
- media：两个不同音频能力的实际配置、当前 Core/Kernel/PostgreSQL 和受控 HTTP provider，验证 fallback 的通道约束、持久 input_access 与成功重放零新增请求。Opaque bytes 仅检查协议和持久语义，不作为视频质量证据。
- longitudinal：Session Observation → automatic Episode → Journal → Memory consolidation，随后重开服务、exact/lexical 查询、WorkContext continuation 和 meaningful UseEvent 重试。Schema 使用唯一 content evidence 输入；同一公共路径核对 Point/Range 边界与 Schema 公共 hard filters。模型 proposal 使用确定性 stub；该场景检查编排与 Authority 语义。
- configuration：Catalog/CLI、scope precedence、operation replay、规范化 active/desired 与重启生效。
- client：用系统临时目录中的独立 npm 安装检查六个 Client 入口、完整 generated 类型，以及真实隔离实例的 status 和 Subject create/get/list。

`smoke:longitudinal` 调用 Rust test harness 启动临时 PostgreSQL 和真实 Kernel gRPC；TypeScript 场景托管真实 Core HTTP 并使用官方 Client。ManualCognitiveClock 通过测试子进程的 stdin/stdout 控制，未增加产品 RPC。该测试由 `just check` 的 workspace tests 执行；aggregate smoke 不重复执行它。`just check` 随后运行 aggregate 公共场景。

场景创建临时实例，通过正常 Core 与官方 Client 操作。`NOUS_WAVE_KERNEL_EXECUTABLE` 可指定 Kernel；`NOUS_WAVE_POSTGRES_RUNTIME` 可指定 PostgreSQL 安装。`support.ts` 是共享启动 helper。


## Self 容量副本

```sh
pnpm smoke:self-selection
# 显式加载自己现有的 private env，使用已经配置的模型；仅创建隔离实例
NOUS_SELF_SELECTION_MODEL=<已配置的反思模型> pnpm smoke:self-selection /absolute/path/openclaw.json /absolute/path/runtime/node_modules/openclaw
```

默认使用本地确定性模型替身，检查真实 Core/Kernel/PostgreSQL 的容量、分页和读取边界，不证明模型判断。提供已有 OpenClaw provider 配置与受控 runtime 后，复用 native runner 的实际 HTTP 捕获，调用真实 self_reflection 并验证原生对话的实际输入和输出含第一页之后的身份。可用 `NOUS_SELF_SELECTION_MODEL` 选择已有配置中实际可用的反思模型，默认采用 provider 的最后一个模型；只消费现有 provider 模型。配置/凭据只用于隔离副本，证据及成果保留在打印的 `data/temp/smoke/self-selection-*` 路径。成功或失败均停止本次 Core/Gateway，付费结果一产生就保存在副本中。
