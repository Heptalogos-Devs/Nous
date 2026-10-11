# 脚本路由

[脚本指南](README.md) · [仓库地图](../INDEX.md)

| 要完成的任务 | 目录指南 | 主要入口 |
| --- | --- | --- |
| 源码开发、离线配置检查、源码长度检查 | [dev](dev/README.md) | `dev`、`nous`、`dev:prepare`、`dev:portable`、`check:length` |
| 构建/打包第三方运行依赖 | [runtime](runtime/README.md) | `runtime:pack`、显式构建 shell 脚本 |
| 构建并验证 Windows Portable | [release](release/README.md) | `release:notices`、`assemble:portable`、`release:verify` |
| 检查真实公共操作的确定性轨迹 | [smoke](smoke/README.md) | `just smoke` 或单个 `smoke:*` |
| 研究真实模型、资料、合同和 wire trace | [research](research/README.md) | `research:*`、`inspect:model-*` |
| 检查文档路由、处理孤立测试 PostgreSQL | [maintenance](maintenance/README.md) | Python 导航检查、PowerShell cluster 清理 |
| 修改和验证仓库工具本身 | [tests](tests/README.md) | `pnpm test scripts/tests` |

共享的仓库生成路径/开发 locator 见 [workspace.ts](workspace.ts)。各文件职责和参数在其目录指南中，不必从实现猜测入口。
