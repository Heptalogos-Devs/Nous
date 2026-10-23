# Nous Wave

长期认知基础设施与人格中心。当前支持材料接纳、持久模型形成、Memory 历史、中文词项查询、来源追溯与快照恢复。CLI、HTTP 和 MCP 共用 Rust 应用操作，SQLite 统一保存权威状态、回执和后续工作。

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
$observation = (& $nous --instance $instance observation record --subject $subject --material $material | ConvertFrom-Json).result.id
& $nous --instance $instance source read $material
& $nous --instance $instance stop
```

Linux 使用同一 PowerShell 环境脚本，程序路径为 `debug/nous`。正式部署用 `--instance <path>` 指定持久位置。开发实例、下载、克隆、缓存和编译均位于 `temp`；脚本只修改当前进程环境。已安装工具链继续使用。

HTTP 入口为 `/v1`，Streamable HTTP MCP 为 `/mcp`。`nous mcp stdio` 连接运行中的服务。凭据默认读取实例内受保护文件；远程调用用 `--url` 与 `--token-file`。停服后可通过 `token create --namespace <id> [--subject <id>] --name <name>` 创建范围凭据。Subject 可读取已获授权的材料并记录再次接触；首次材料暴露由 Namespace 或管理员接纳。

材料保存、实际接触和记忆形成各有独立身份。形成请求先持久入队，模型调用在写事务外进行；保存输出后，由同一权威事务采纳 Memory、来源关系与索引任务。Tantivy 索引可重建，查询合并未索引增量，并在返回前读取当前权威修订。

停服后按 [模型资源配置](docs/基建实施规划.md#配置授权与模型资源) 设置 `instance.toml` 与凭据引用，再启动服务。配置只保存 SecretRef，密钥内容放入实例的受保护 `credentials` 目录。另开终端执行：

```powershell
& $nous --instance $instance model status
$formation = & $nous --instance $instance memory form --subject $subject --observation $observation --operation-id first-formation | ConvertFrom-Json
& $nous --instance $instance job get $formation.job_ids[0]
```

任务达到 `succeeded` 后，可查询记忆并读取其原文、实际接触、Prompt、模型参数及 usage：

```powershell
$result = & $nous --instance $instance query --subject $subject '枫叶 秋天' | ConvertFrom-Json
$memory = $result.hits[0].memory
& $nous --instance $instance memory source $memory.id
& $nous --instance $instance memory history $memory.id
$backup = & $nous --instance $instance backup create | ConvertFrom-Json
& $nous --instance $instance stop
& $nous --instance ./temp/instances/restored backup restore $backup.backup
```

形成在缺少配置或达到每日调用预算时保留等待状态。重启会恢复已接受任务，已保存的成功输出可继续采纳；中断的远程调用保留不确定状态及重试记录。备份包含数据库、引用对象、配置和校验清单；恢复到新实例时生成新的管理员凭据，模型密钥通过正式凭据入口重新配置。

验证入口：

```powershell
. ./scripts/dev-env.ps1
cargo test --workspace --locked
cargo run -p nous-storage --example transaction_recovery --locked -- (Join-Path (Get-Location) 'temp/instances/recovery.sqlite')
python ./scripts/smoke.py (Join-Path $env:CARGO_TARGET_DIR 'debug/nous.exe')
# 使用已配置模型的实例作为模板，实际调用其模型服务。
python ./scripts/m1-smoke.py (Join-Path $env:CARGO_TARGET_DIR 'debug/nous.exe') --instance ./temp/instances/m1-dev
```

恢复试验要求使用新的数据库文件。`smoke.py` 验证基础协议与作用域；`m1-smoke.py` 从模板复制配置与必要模型凭据到独立开发实例，验证真实形成、两种 MCP、调用中强杀恢复、单次采纳与快照恢复。运行产物与报告集中于 `temp`。Windows 与 Ubuntu 工作流位于 [CI](.github/workflows/ci.yml)，日常 CI 使用本地受控模型协议响应。
