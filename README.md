# Nous Wave

> **INTERRUPTED — 开发已暂停（2026-10-20）。** 当前分支保存未完成施工；最新改动尚未整体验证。详见[中断交接](Session-Handoff-2026-10-20-15-28.md)。恢复须等待用户明确指令。

长期认知服务：有来源和时间的材料、实际接触、认识与修订，支持独立记忆产品及 Heptalogos 联合主体。

项目正在按照[完整施工方案快照](docs/construction-specs/README.md)从新代码结构和空数据重建。[当前施工](docs/construction.md)保留完整目标、已定边界和正在进行的功能。旧实现可在 Git 历史中查询。

正式目录按认知责任组织：`packages/nous` 是可嵌入业务核心，`packages/contracts` 定义公共 Zod schema，`apps/server`、`apps/cli` 和 `packages/client` 使用同一套操作；`integrations/openclaw` 接入原生模型调用。

运行入口和使用示例随本地链验证同步补充。开发使用 Node 24、TypeScript 7、pnpm、PostgreSQL 18。
