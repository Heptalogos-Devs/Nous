# 仓库脚本

[脚本路由](INDEX.md) · [仓库地图](../INDEX.md)

从仓库根执行。TypeScript 入口先安装 `corepack pnpm install --frozen-lockfile`；命令由 [package.json](../package.json) 和 [justfile](../justfile) 选择。

开发、第三方 runtime、Portable 发布、公共 smoke、真实模型研究和维护各有独立目录指南。目录指南先给出任务入口与每个文件的用途，再说明前置条件、输出和恢复方法；内部 helper 跟随调用它的入口说明。

本地生成物统一放在 ignored `data/`。普通服务启动使用已安装运行包；runtime 获取、发布准备和真实模型研究均是显式动作。Operator 配置与 Secrets 属于实例，不进入 Portable payload。

[workspace.ts](workspace.ts) 只拥有仓库内本地生成路径和开发 locator；产品路径规则由 [Core locations](../apps/nous-core/src/locations.ts) 和 [Runtime Bundle Spec](../docs/specs/active/deployment/runtime-bundle.md) 拥有。
