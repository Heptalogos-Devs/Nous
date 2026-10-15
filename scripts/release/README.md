# 发布产物构建与验证

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

Client 构建复用工作区 TypeScript 编译器和 npm 打包，不需要 Kernel 或 Runtime packs。Windows Portable 先准备 [Runtime packs](../runtime/README.md)，再构建 shipping Kernel、准备 notices、离线组装、验证实际 bundle。

| 文件 | 用途 |
| --- | --- |
| [client.ts](client.ts) | 编译并打包可独立安装的官方 Client 与 bundled Protocol |
| [verify-client.ts](verify-client.ts) | 仓库外 npm 安装、全部入口加载及完整类型检查 |
| [build-kernel.ps1](build-kernel.ps1) | shipping gnullvm Kernel、私有运行库和 strip |
| [prepare-notices.ts](prepare-notices.ts) | 网络阶段：准备当前应用与真实供应者许可证 |
| [assemble.ts](assemble.ts) | 离线阶段：组装 source-less Program/Runtime |
| [verify-portable.ts](verify-portable.ts) | 隔离实例的布局、重启、relocation 和缺 pack 拒绝 |
| [archive.ts](archive.ts) | 将现成 current.zip 保存为不可覆盖的归档 |
| [bundle.ts](bundle.ts) | assembler/notices 共用的应用 bundle |
| [manifest.ts](manifest.ts) | release inventory 与内容 SHA |
| [notices.ts](notices.ts) | 应用依赖及 runtime notices 读取/准备 |
| [zip.ps1](zip.ps1) | runtime pack 和 assembler 共用 ZIP helper |

## Client 发布

`corepack pnpm client:pack` 生成 `data/releases/client/nous-wave-client-0.1.0.tgz`。`corepack pnpm client:verify` 在系统临时目录安装该产物，用普通 Node 加载全部公开入口，并用严格 TypeScript 消费者生成声明。类型 fixture 位于 [client-consumer.ts](../tests/release/client-consumer.ts)。该检查已接入 `check:fast`，真实隔离实例操作已接入 [smoke:client](../smoke/client.ts)。安装方式和入口合同由 [Client README](../../packages/client/README.md) 维护。

## Portable 发布

先准备 Node、PostgreSQL、FFmpeg packs/catalog，并安装 shipping Kernel 的 LLVM-MinGW 工具链。版本与 payload 合同见 [Runtime Bundle Spec](../../docs/specs/active/deployment/runtime-bundle.md)。

```text
powershell -NoProfile -File scripts/release/build-kernel.ps1 -ToolchainRoot <llvm-mingw目录>
corepack pnpm release:notices
corepack pnpm assemble:portable
```

`build-kernel.ps1` 编译 `x86_64-pc-windows-gnullvm` release Kernel，复制私有 C++ runtime DLL 并 strip debug 信息。默认工具链目录为 `data/tools/llvm-mingw-windows/llvm-mingw-20260922-ucrt-x86_64`。

应用 bundle 每次清空并重建 `data/cache/release/application/program/` 的五个实际入口，不生成源码 digest 目录或 packages.json 检查点。`release:notices` 将实际依赖/运行时 license notices 与 native closure 准备到 `data/cache/release/notices/current/`，公开 URL 文本缓存可复用；该准备阶段需要网络。`assemble:portable` 消费这些缓存、release Kernel 和 packs，离线生成 `data/releases/windows-x64/current/` 与 `current.zip`，替换当前输出。离线组装校对当前 bundle 的 npm inventory 与 Kernel 的实际 Cargo closure，runtime 内容由 pack inventory 校验。包不包含开发配置或文档示例。`just release-prepare` 与 `just release` 分别封装准备和组装阶段。

`release:notices --runtime-root <packs/catalog 根目录>` 与 `assemble:portable --runtime-root <同一目录>` 可显式选择独立的 shipping packs；该目录包含 `manifest/runtimes.json` 与 `packs/`。默认仍为 `data/runtime/`。开发与 shipping runtime 内容不同时，应使用独立 catalog，避免更换正在运行实例所依赖的 manifest identity；组装只将选中的 catalog 与 runtime 放进发布包。

```text
corepack pnpm release:verify --bundle data/releases/windows-x64/current.zip
corepack pnpm release:verify --bundle data/releases/windows-x64/current.zip --layout colocated
corepack pnpm release:verify --bundle data/releases/windows-x64/current.zip --layout locator --relocate
```

验证会在 `data/temp/portable/` 下的隔离目录解包并启动 portable Core，通过分发的 Client 操作实例。layout 默认 `home`，也支持 `colocated`、`locator`；`--relocate` 移动独立实例后重新启动，不能和 colocated 合用。`just release-verify` 调用以上三种布局。通过后停止实例并删除本次临时目录；失败目录保留，供直接检查。

`corepack pnpm release:archive` 将现有 current.zip 保存到 `data/releases/archive/windows-x64/<source-head>-<digest>.zip`，不会重新组装，同名归档已存在时报错。`zip.ps1` 是 pack/assembler 共用 ZIP helper，无需单独调用。
