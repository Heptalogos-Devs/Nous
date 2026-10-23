# Nous Wave

长期认知基础设施与人格中心。当前实现为 Rust 原生基础设施及材料接纳：实例、配置、身份、作用域凭据、SQLite 事务、持久队列，以及共用应用操作的 CLI、HTTP 和 MCP。

工程方向见 [基建实施规划](docs/基建实施规划.md)，当前进度与下一步见 [首批开发任务](docs/首批开发任务.md)。源码复用入口见 [源码研究](docs/源码研究.md)、[实际复用记录](research/reuse.md) 和 [研究仓库锁定清单](research/sources.lock.json)。

需求以用户提供的《项目宪纲》附件 v3 为准，正文快照位于 [项目宪纲](docs/requirements/项目宪纲.md)。该文件供本次开发定位需求；Library 原件仍由原 Library 条目维护。

使用 Rust 1.97 或后续兼容版本。在项目根目录执行：

```powershell
. ./scripts/dev-env.ps1
cargo build -p nous --locked
$nous = Join-Path $env:CARGO_TARGET_DIR 'debug/nous.exe'
$instance = Join-Path (Get-Location) 'temp/instances/dev'
& $nous --instance $instance init
& $nous --instance $instance serve
```

另开 PowerShell 会话操作运行中的实例：

```powershell
. ./scripts/dev-env.ps1
$nous = Join-Path $env:CARGO_TARGET_DIR 'debug/nous.exe'
$instance = Join-Path (Get-Location) 'temp/instances/dev'
& $nous --instance $instance status
$namespace = (& $nous --instance $instance namespace create research --operation-id first-namespace | ConvertFrom-Json).result.id
$subject = (& $nous --instance $instance subject create --namespace $namespace --name reader | ConvertFrom-Json).result.id
Set-Content -LiteralPath ./temp/example.txt -Value '秋天的枫叶会变红。' -Encoding utf8
$material = (& $nous --instance $instance material put --namespace $namespace --file ./temp/example.txt | ConvertFrom-Json).result.id
& $nous --instance $instance observation record --subject $subject --material $material
& $nous --instance $instance source read $material
& $nous --instance $instance stop
```

Linux 使用同一 PowerShell 环境脚本，程序路径为 `debug/nous`。正式部署用 `--instance <path>` 指定持久位置。开发实例、下载、克隆、缓存和编译均位于 `temp`；脚本只修改当前进程环境。已安装工具链继续使用。

HTTP 入口为 `/v1`，Streamable HTTP MCP 为 `/mcp`。`nous mcp stdio` 连接运行中的服务。凭据默认读取实例内受保护文件；远程调用用 `--url` 与 `--token-file`。停服后可通过 `token create --namespace <id> [--subject <id>] --name <name>` 创建范围凭据。Subject 可读取已获授权的材料并记录再次接触；首次材料暴露由 Namespace 或管理员接纳。

材料保存、实际接触和记忆形成各有独立身份。当前提供前两项；模型形成、记忆修订、检索及 A-Memorix 完整能力按 M1 后续批次建设。持久队列已提供同事务入队与恢复机制，领域 Worker 随对应能力接入。

验证入口：

```powershell
. ./scripts/dev-env.ps1
cargo test --workspace --locked
cargo run -p nous-storage --example transaction_recovery --locked -- (Join-Path (Get-Location) 'temp/instances/recovery.sqlite')
python ./scripts/smoke.py (Join-Path $env:CARGO_TARGET_DIR 'debug/nous.exe')
```

恢复试验要求使用新的数据库文件。`smoke.py` 自动创建并清理隔离实例，验证真实 CLI、HTTP、两种 MCP、范围授权、幂等和优雅停止后重启。Windows 与 Ubuntu 工作流位于 [CI](.github/workflows/ci.yml)。
