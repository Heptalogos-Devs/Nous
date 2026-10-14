# 仓库工具测试

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

执行 `corepack pnpm test scripts/tests`。测试与工具实现分开。

| 测试 | 独立风险 | 实现入口 |
| --- | --- | --- |
| [gateway](research/gateway.test.ts) | 实际代理预算、并发请求和 telemetry | [研究代理](../research/README.md) |
| [gateway-trace](research/gateway-trace.test.ts) | capture 上限、媒体描述符和凭据清除 | [研究 trace](../research/README.md) |
| [model-call-guard](research/model-call-guard.test.ts) | 持久预算恢复和耗尽 | [研究预算](../research/README.md) |

Core/Kernel 公共操作场景在 [smoke](../smoke/README.md)；真实模型质量观察由 [Research](../../docs/research/README.md) 保存。
