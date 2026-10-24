# Nous Wave

长期认知基础设施与人格中心。当前支持材料接纳、忠实文本导入、持久模型形成、Memory 修订与生命周期维护、中文词项和 Dense/VCP/Hybrid 查询、来源追溯及快照恢复。CLI、HTTP 和 MCP 共用 Rust 应用操作，SQLite 统一保存权威状态、回执、已采纳向量和后续工作。

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

HTTP 入口为 `/api`，Streamable HTTP MCP 为 `/mcp`。`nous mcp stdio` 连接运行中的服务。凭据默认读取实例内受保护文件；远程调用用 `--url` 与 `--token-file`。停服后可通过 `token create --namespace <id> [--subject <id>] --name <name>` 创建范围凭据。Subject 可读取已获授权的材料并记录再次接触；首次材料暴露由 Namespace 或管理员接纳。

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

| HTTP                                          | MCP 工具             |
| --------------------------------------------- | -------------------- |
| `POST /api/imports/text`                      | `import_text`        |
| `POST /api/memories/{id}/change`              | `memory_change`      |
| `GET /api/memories/{id}/audit?limit=50`       | `memory_audit`       |
| `GET /api/subjects/{id}/recycle-bin?limit=50` | `memory_recycle_bin` |
| `POST /api/subjects/{id}/maintenance`         | `memory_maintain`    |
| `POST /api/embeddings/sync`                   | `embedding_sync`     |
| `GET /api/embeddings/runs/{id}`               | `embedding_run`      |
| `POST /api/query`                             | `query`              |
| `GET /api/memories/{id}/source`               | `memory_source`      |
| `GET /api/models/calls/{id}`                  | `model_call`         |
| `GET /api/jobs/{id}`                          | `job_get`            |
| `POST /api/jobs/{id}/cancel`                  | `job_cancel`         |

`.amembundle` 可在本地离线检查，无需启动实例：

```powershell
& $nous import bundle inspect --file ./temp/knowledge.amembundle
# 展开完整状态、来源引用与成员库存。
& $nous import bundle inspect --file ./temp/knowledge.amembundle --details
```

检查器承接上游 v1/v2 的知识、完整状态和图像包，核对成员校验和、语义摘要、记录引用与内容数量；默认显示 manifest、库存数量和所需领域能力。完整包的事务安装、导出与卸载，以及图谱、Episode、画像和 A-Memorix 兼容 API 按 [合同承接表](docs/首批开发任务.md#当前-a-memorix-合同承接) 继续实施。

[nous-vcp](crates/nous-vcp/README.md) 提供完整 VCP 计算族和旧版数值基线，包含 AGS 几何、传播资产、Pyramid、Sense、双场、RiverMemo、DTSC 与 Thinking。开发者可用 `cargo run -p nous-vcp --example native_pipeline` 重放完整计算和来源映射。实际文本查询已接通模型向量、USearch 候选/Tag 双索引、VCP 图资产和来源映射。

停服后按 [模型资源配置](docs/基建实施规划.md#配置模型资源与提示词) 通过 `config schema` 查看当前字段，再设置 `instance.toml` 与凭据引用，再启动服务。配置只保存 SecretRef，密钥内容放入实例的受保护 `credentials` 目录。另开终端执行：

```powershell
& $nous --instance $instance model status
$formation = & $nous --instance $instance memory form --subject $subject --observation $observation --operation-id first-formation | ConvertFrom-Json
& $nous --instance $instance job get $formation.job_ids[0]
```

文本向量职责使用配置中的 `models.text_embedding`，用 `config schema` 查看当前字段；职责与资源边界见 [模型资源与提示词](docs/基建实施规划.md#配置模型资源与提示词)。配置就绪后，可显式补齐主体认知和来源标签的向量，再查询：

```powershell
$embedding = & $nous --instance $instance memory embed --subject $subject --operation-id first-embedding | ConvertFrom-Json
$embedding.result.jobs | ForEach-Object { & $nous --instance $instance job get $_ }
& $nous --instance $instance query --subject $subject --mode dense '秋天有哪些植物变化？'
& $nous --instance $instance query --subject $subject --mode vcp '秋天有哪些植物变化？'
& $nous --instance $instance query --subject $subject --mode hybrid '秋天有哪些植物变化？'
```

`memory embed` 返回持久任务 ID；`result.jobs` 包含本次新建和已在处理相同输入的任务。HTTP 提交 `POST /api/embeddings/sync`，JSON 为 `{"subject_id":"<id>","operation_id":"first-embedding","limit":5000}`；按 ID 读取 `/api/embeddings/runs/<job_id>`。MCP 使用 `embedding_sync` 和 `embedding_run {job_id}`。查询默认 `lexical`；HTTP/MCP 的 `mode` 同样接受 `dense`、`vcp`、`hybrid`。语义查询会补齐缺少的当前认知、标签和查询向量并等待结果；当前主体快照上限为 5000 条认知对象，超出时返回 ResourceExhausted。

标签取自来源 MaterialDetails.tags 的原文，去除首尾空白后生成稳定身份；同一 Material 派生的多个认知对象 在图证据中共用一个来源根。范围、来源、时间和活跃状态先约束候选与构图，命中返回前再读取最新权威修订。候选索引、Tag 索引和完整 VCP 资产按不可变代际一起发布；重启加载已发布资产，恢复与重建复用 SQLite 保存的向量原值。同一模型空间的相同查询复用持久查询向量，重启后再次查询无需远程嵌入。正文未变的生命周期修订也复用原向量和调用来源。

任务达到 `succeeded` 后，可查询记忆并读取其原文、实际接触、Prompt、模型参数及 usage：

```powershell
$result = & $nous --instance $instance query --subject $subject --kind memory '枫叶 秋天' | ConvertFrom-Json
$memory = $result.hits[0].object
$reference = $result.hits[0].reference
& $nous --instance $instance query --subject $subject --reference "$($reference.kind):$($reference.id)@$($reference.revision)"
& $nous --instance $instance memory source $memory.id
& $nous --instance $instance memory history $memory.id
# source 返回 embedding_provenance，可用其 model_call_id 展开实际请求与 usage。
$source = & $nous --instance $instance memory source $memory.id | ConvertFrom-Json
if ($source.embedding_provenance.Count -gt 0) {
    & $nous --instance $instance model call $source.embedding_provenance[0].model_call_id
}
$backup = & $nous --instance $instance backup create | ConvertFrom-Json
& $nous --instance $instance stop
& $nous --instance ./temp/instances/restored backup restore $backup.backup
```

所有任务使用统一 JobRecord；形成与嵌入调用同一 ModelCall 服务和按资源每日调用预约。`job get` 返回 `job`、`queue` 和 `model_calls`；等待时 `job.state` 为 `waiting`，`job.wait.kind` 区分 `configuration`、`budget` 等条件。用 `job cancel <id> --operation-id <id>` 取消已接受的工作。重启会恢复已接受任务，已保存的成功输出可继续采纳；中断的远程调用保留不确定状态及重试记录。相同范围、模型空间和源输入的活跃任务共用处理权，重复同步返回已有任务。备份包含数据库、已采纳向量、引用对象、运行时配置快照、绑定的 Prompt 文件和校验清单；恢复到新实例时生成新的管理员凭据，按 SecretRef 重新提供模型凭据。

结构化认知通过同一应用入口创建、修订、查询、展开来源和形成。支持 MemorySemantics、Concept、Episode、Journal、Schema、Association；Concept 可原子合并/拆分，Episode 可按完整来源序列重分段。精确修订证据区分支持、反驳与上下文，同一材料的派生路径共享来源根；Journal 保存写作时的知识截面。

`nous cognition --schema` 输出完整请求 JSON Schema，`nous cognition --file <request.json>` 提交请求。HTTP 使用 `POST /api/cognition`，MCP 使用 `cognition` 工具，三者接收相同的 `action` 判别请求：`create`、`revise`、`get`、`history`、`set_deleted`、`source`、`concept_resolve`、`merge_concepts`、`split_concept`、`resegment_episodes`、`form`、`formation_run`。所有认知使用唯一 `query` 用例：CLI 的 `--kind`、`--reference kind:UUID@revision`、`--as-of-commit`、`--include-inactive` 与 `--include-deleted` 对应 HTTP/MCP 的 QueryRequest。HTTP/MCP 的 `reference` 使用包含 `kind`、`id`、`revision` 的对象。命中包含 `reference`、`object`、`score` 和 `source_roots`；Memory、Concept、Episode、Journal、Schema、Association 共用词项及 Dense/VCP/Hybrid 查询。

形成请求的 `input` 包含 `subject_id`、精确 `sources` 和 `targets[{key,kind,existing}]`。服务读取已授权来源并分配稳定目标 ID，模型只从来源键与目标槽位选择。`models.structured_formation` 使用与 `memory_formation` 相同的资源和 SecretRef 结构。Prompt 默认使用随包模板，也可通过 `prompt.file` 绑定实例 `prompts` 目录内的文件；调用记录保存模板、渲染文本、变量来源及实际摘要。默认输入预算 64 KiB（含输出 Schema）、输出 8192 tokens、每日 20 次调用。它复用共享 ModelCall 与持久队列；每个提案组独立事务采纳，返回 applied/conflict/rejected/skipped，缺配置等待、重启续接、来源清除退役均保存状态。

形成作业的 `succeeded` 表示全部组已处理，具体成果和冲突读取 `formation_run` 的 `groups`。`source` 返回对象精确修订、证据闭包、原材料和实际模型调用。CLI 交互优化、媒体三策略、原生多模态、managed context/steward 与完整 A-Memorix 合同继续按 [能力续接清单](docs/能力续接清单.md) 推进。

## 开发工具

`rust-toolchain.toml` 固定 Rust 与组件，本地和 CI 由 rustup 读取同一文件。Rust 使用 rustfmt 和 Clippy；编辑器配置提供 rust-analyzer。`.editorconfig` 统一缩进与文件结尾，`.gitattributes` 统一 Git 换行和二进制文件处理。

Markdown、JSON、YAML 使用固定版本的 Prettier。Node.js 24 可直接运行以下命令，npm 将工具下载到项目 `temp/cache/npm`，无需在仓库根目录安装 `node_modules`：

```powershell
. ./scripts/dev-env.ps1
npm run format
npm run format:check
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked
```

格式检查已接入 CI。Cargo 构建、编辑器检查和工具缓存使用项目内的受管临时目录。
