# 开发与源码检查

[脚本路由](../INDEX.md) · [脚本指南](../README.md)

从仓库根运行。常规开发先安装依赖和构建 Kernel，再显式准备 runtime。下面的 Portable 入口使用实际已组装程序。

## 文件职责

| 文件 | 用途 |
| --- | --- |
| [start.ts](start.ts) | 源码 Core/Kernel 开发实例与退出生命周期 |
| [prepare.ts](prepare.ts) | 显式准备开发 PostgreSQL 运行包 |
| [postgres-linux.ts](postgres-linux.ts) | Linux 开发 PostgreSQL 的依赖库准备，供 prepare 调用 |
| [nous.ts](nous.ts) | 对开发 locator 调用源码 CLI |
| [portable.ts](portable.ts) | 用实际 Portable 程序连接独立开发 locator |
| [check-code-length.ts](check-code-length.ts) | 对 Git 管理和未忽略的新手写文件检查物理行数 |

`pnpm check:length` 读取唯一配置 [.config/scripts/code-length.toml](../../.config/scripts/code-length.toml)，检查生产源码、测试文件、测试 helper、开发/研究/构建脚本；Rust 内联测试也计入所属源码。生成 bindings 只按配置中的两个路径排除，测试目录没有豁免。当前达到 TypeScript 600 / Rust 500 行警告，达到 800 / 700 行拒绝；warning 不升级成 error。该门禁位于 `check:fast` 的编译前，也由两平台 CI 使用。

## 开发实例

```text
cargo build -p nous-kernel
corepack pnpm dev:prepare
corepack pnpm dev
corepack pnpm nous status
```

`dev:prepare` 准备 PostgreSQL 18.6 runtime，不启动 Core。Windows 从 `data/runtime/packs/` 与 `data/runtime/manifest/runtimes.json` 安装已准备的 pack；其他平台使用 Kernel 的 dev-runtime 下载入口。`NOUS_WAVE_POSTGRES_RUNTIME` 可指定现有安装目录。

`dev` 启动源码 Core 和 debug Kernel，生成路径 locator 并使用 `data/instances/dev/bootstrap.toml` 与 `data/config/apps/nous.toml`。配置缺失时创建最小配置，已有文件保持原样。Kernel 未构建时直接报错；终端退出时关闭子进程。

`nous` 将参数传给源码 CLI，默认连接开发实例。用 `--home <实例目录>` 或 `--locator <bootstrap.toml>` 显式选择实例。操作示例见 [CLI README](../../apps/nous-cli/README.md)。

## 配置检查与真实 portable 开发

完整配置说明保存在 [examples/nous.toml](../../docs/reference/examples/nous.toml)，含当前版本与注释。修改 `data/config/apps/nous.toml` 后用当前源码离线检查：

```text
corepack pnpm nous config check
corepack pnpm dev:portable config check
corepack pnpm dev:portable serve
corepack pnpm dev:portable status
```

`dev:portable` 默认使用 `data/releases/windows-x64/current/` 中的 Node 和 launcher，进而使用包内 Core/Kernel/Runtime；可用 `--bundle <解包目录>` 指定另一实际包。配置和密钥直接引用 `data/config/apps/`、`data/config/secrets/`，不复制或覆盖。生成的 locator 位于 `data/instances/portable/bootstrap.toml`，Authority/对象/身份与源码开发实例独立。无参数等同 `serve`；关闭规则由正常 launcher 承担。

包尚未组装时该命令明确失败。配置字段与 `config_revision` 必须符合所选 Core package 的合同；版本不匹配时，按该 package 合同更新配置，或选择与配置匹配的 package。
