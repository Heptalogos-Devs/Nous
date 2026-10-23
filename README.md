# Nous Wave

长期认知基础设施与人格中心。当前支持材料接纳、忠实文本导入、持久模型形成、Memory 修订与生命周期维护、中文词项查询、来源追溯及快照恢复。CLI、HTTP 和 MCP 共用 Rust 应用操作，SQLite 统一保存权威状态、回执和后续工作。

工程方向见 [基建实施规划](docs/基建实施规划.md)，当前进度与 [合同覆盖](docs/首批开发任务.md#当前-a-memorix-合同承接) 见首批开发任务。[能力续接清单](docs/能力续接清单.md) 保留结构化认知、多模态、managed context/steward 和完整 A-Memorix 产品的后续目标。源码复用入口见 [源码研究](docs/源码研究.md)、[实际复用记录](research/reuse.md) 和 [研究仓库锁定清单](research/sources.lock.json)。

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
```

Linux 使用同一 PowerShell 环境脚本，程序路径为 `debug/nous`。正式部署用 `--instance <path>` 指定持久位置。开发实例、下载、克隆、缓存和编译均位于 `temp`；脚本只修改当前进程环境。已安装工具链继续使用。

HTTP 入口为 `/v1`，Streamable HTTP MCP 为 `/mcp`。`nous mcp stdio` 连接运行中的服务。凭据默认读取实例内受保护文件；远程调用用 `--url` 与 `--token-file`。停服后可通过 `token create --namespace <id> [--subject <id>] --name <name>` 创建范围凭据。Subject 可读取已获授权的材料并记录再次接触；首次材料暴露由 Namespace 或管理员接纳。

材料保存、实际接触和记忆形成各有独立身份。`observation record` 创建 `kind=contact` 的实际接触记录。模型形成请求先持久入队，模型调用在写事务外进行；保存输出后，由同一权威事务采纳 Memory、来源关系与索引任务。Tantivy 索引可重建，查询合并未索引增量，并在返回前读取当前权威修订。

文本导入可直接生成可查询的忠实记忆，无需模型配置。准备 UTF-8 JSONL 文件并导入：

```powershell
@'
{"external_id":"leaf-001","text":"秋天的枫叶会变红。","source":"field-notes","tags":["植物"],"metadata":{"collection":"秋季观察"}}
{"external_id":"leaf-002","text":"银杏叶在秋天变黄。","source":"field-notes"}
'@ | Set-Content -LiteralPath ./temp/import.jsonl -Encoding utf8
$imported = & $nous --instance $instance import text --namespace $namespace --subject $subject --file ./temp/import.jsonl --format jsonl | ConvertFrom-Json
$imported.items | Select-Object index, status, error
$importedMemory = $imported.items[0].receipt.result.memory_id
& $nous --instance $instance query --subject $subject '秋天 枫叶'
& $nous --instance $instance memory source $importedMemory
```

导入保存 Material、`kind=import` 的来源事件、`origin=imported` 的忠实 Memory，以及标签、metadata 和 structured。实际接触保留独立 Contact 记录。`--form-memory` 可额外提交模型派生任务。同一调用方、Subject 和 external_id 的相同载荷重放旧回执，改变载荷返回 Conflict；每条记录独立提交，结果按原顺序给出 accepted、replayed 或 rejected。

格式支持 `text`、`jsonl`、`json`；JSON 接受记录数组、`records`、A-Memorix `paragraphs` 和 LPMM `docs`。原生记录含 external_id、text 及可选 source、source_type、event_time、observed_at、tags、metadata、structured。输入与规范化结果各限 4 MiB，最多 1000 条，单条限 1 MiB；HTTP 另受实例请求体上限约束。成熟格式中的实体、三元组和时间载荷保存在 structured，后续由相应认知领域承接。

记忆变更带当前修订号，每次形成新修订和审计。以下操作冻结记忆、查看非活跃结果，再恢复：

```powershell
$memory = & $nous --instance $instance memory get $importedMemory | ConvertFrom-Json
$frozen = & $nous --instance $instance memory change $memory.id --action freeze --expected-revision $memory.revision --operation-id freeze-leaf | ConvertFrom-Json
& $nous --instance $instance query --subject $subject --include-inactive '枫叶'
& $nous --instance $instance memory change $memory.id --action restore --expected-revision $frozen.result.revision
& $nous --instance $instance memory audit $memory.id
```

`memory change` 支持 reinforce、weaken、protect、unprotect、freeze、forget、restore、trash、purge、revise、decay；revise 通过 `--edit` 提供 JSON 编辑对象。protect 的 `--hours` 为正时设置期限，省略或非正值时永久保护。freeze、forget、weaken 保留正文和修订；普通查询排除非活跃记忆，`--include-inactive` 或精确引用可读取。trash（也可用 `memory delete`）进入 `memory recycle-bin --subject <id>`；restore 恢复。purge 需要 Namespace 或管理员权限，默认等待软删除 24 小时，`--force` 可显式跳过期限；清除正文修订及相关模型输出、回执副本，保留最小身份和审计，原始 Material 继续由材料领域管理。

按批评估自动衰减时，默认每批 50 条，范围为 1..1000。返回值包含 snapshot_seq、next_cursor 和 complete，续扫同一快照直到完成：

```powershell
$page = & $nous --instance $instance memory maintain --subject $subject --limit 50 | ConvertFrom-Json
while (-not $page.result.complete) {
    $page = & $nous --instance $instance memory maintain --subject $subject --limit 50 --after $page.result.next_cursor --snapshot-seq $page.result.snapshot_seq | ConvertFrom-Json
}
```

维护跳过受保护对象；自动衰减达到阈值后进入 Decayed，持续 24 小时仍低于阈值时归档到回收站。手动 Frozen、Forgotten 和 Weakened 保留各自状态。

本批原生接口与 MCP 工具如下，均进入相同应用用例：

| HTTP | MCP 工具 |
| --- | --- |
| `POST /v1/imports/text` | `import_text` |
| `POST /v1/memories/{id}/change` | `memory_change` |
| `GET /v1/memories/{id}/audit?limit=50` | `memory_audit` |
| `GET /v1/subjects/{id}/recycle-bin?limit=50` | `memory_recycle_bin` |
| `POST /v1/subjects/{id}/maintenance` | `memory_maintain` |

`.amembundle` 可在本地离线检查，无需启动实例：

```powershell
& $nous import bundle inspect --file ./temp/knowledge.amembundle
# 展开完整状态、来源引用与成员库存。
& $nous import bundle inspect --file ./temp/knowledge.amembundle --details
```

检查器承接上游 v1/v2 的知识、完整状态和图像包，核对成员校验和、语义摘要、记录引用与内容数量；默认显示 manifest、库存数量和所需领域能力。完整包的事务安装、导出与卸载，以及图谱、Episode、画像和 A-Memorix 兼容 API 按 [合同承接表](docs/首批开发任务.md#当前-a-memorix-合同承接) 继续实施。

[nous-vcp](crates/nous-vcp/README.md) 提供完整 VCP 计算族和旧版数值基线，包含 AGS 几何、传播资产、Pyramid、Sense、双场、RiverMemo、DTSC 与 Thinking。开发者可用 `cargo run -p nous-vcp --example native_pipeline` 重放完整计算和来源映射。retrieval 已提供真实 USearch 候选/Tag 双索引、成组发布和重启加载；真实模型向量、图资产与统一查询操作继续接入。

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
python ./scripts/m2-smoke.py (Join-Path $env:CARGO_TARGET_DIR 'debug/nous.exe')
python ./scripts/bundle-smoke.py (Join-Path $env:CARGO_TARGET_DIR 'debug/nous.exe')
# 使用已配置模型的实例作为模板，实际调用其模型服务。
python ./scripts/m1-smoke.py (Join-Path $env:CARGO_TARGET_DIR 'debug/nous.exe') --instance ./temp/instances/m1-dev
```

恢复试验要求使用新的数据库文件。`smoke.py` 验证基础协议与作用域；`m2-smoke.py` 在无模型配置的新实例中验证导入、生命周期、维护分页、作用域、清除和重启，覆盖 CLI、HTTP、MCP HTTP 与 stdio；`bundle-smoke.py` 用三类真实导出包验证离线检查、完整库存与错误包处理。当前 Windows 的 66 项工作区测试、fmt、clippy 和构建通过，M2 与 bundle 的真实使用检查分别通过 31 项和 10 项，模型调用数为 0。`m1-smoke.py` 从模板复制配置与必要模型凭据到独立开发实例，验证真实形成、两种 MCP、调用中强杀恢复、单次采纳与快照恢复。运行产物与报告集中于 `temp`。Windows 与 Ubuntu 工作流位于 [CI](.github/workflows/ci.yml)，日常 CI 使用本地受控模型协议响应。
