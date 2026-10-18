# 仓库工具测试

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

执行 `corepack pnpm test scripts/tests`。测试与工具实现分开。

Core/Kernel 公共操作场景在 [smoke](../smoke/README.md)；真实模型质量观察由 [Research](../../docs/research/README.md) 保存。

[Client consumer fixture](release/client-consumer.ts) 由 `corepack pnpm client:verify` 在仓库外 npm 安装中编译和执行，检查公开入口、generated 类型与下游声明生成。真实隔离实例的 Client 操作由 `smoke:longitudinal` 执行。

[Native custody](openclaw/custody.test.ts) 由 Vitest 自动发现，`check:fast` 也通过 `openclaw:custody:verify` 执行。它只拷贝 workspace 固定 OpenClaw 的运行代码到临时根，经正式 prepare 应用补丁，再运行实际 SQLite/delivery fixture；不联网安装、不使用 operator runtime、不发送 QQ。
