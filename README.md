# Nous Wave

长期认知基础设施与人格中心。当前支持流式材料接纳、媒体描述与结构化转述、忠实文本导入、持久模型形成、结构化认知、Memory 修订与生命周期维护、中文词项和 Dense/VCP/Hybrid 查询、来源追溯及快照恢复。CLI、HTTP 和 MCP 共用 Rust 应用操作，SQLite 统一保存权威状态、回执、已采纳向量和后续工作。

工程方向见 [基建实施规划](docs/基建实施规划.md)，当前进度与 [合同覆盖](docs/首批开发任务.md#当前-a-memorix-合同承接) 见首批开发任务。[能力续接清单](docs/能力续接清单.md) 保留多模态、managed context/steward 和完整 A-Memorix 产品的后续目标。源码复用入口见 [源码研究](docs/源码研究.md)、[实际复用记录](research/reuse.md) 和 [研究仓库锁定清单](research/sources.lock.json)。

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
$namespace = (& $nous --json --instance $instance namespace create research --operation-id first-namespace | ConvertFrom-Json).result.id
$subject = (& $nous --json --instance $instance subject create --namespace $namespace --name reader | ConvertFrom-Json).result.id
$material = (& $nous --json --instance $instance material put --namespace $namespace --file ./docs/requirements/项目宪纲.md | ConvertFrom-Json).result.id
$observation = (& $nous --json --instance $instance observation record --subject $subject --material $material | ConvertFrom-Json).result.id
& $nous --instance $instance source read "material:$material" --subject $subject
```

Linux 使用同一 PowerShell 环境脚本，程序路径为 `debug/nous`。正式部署用 `--instance <path>` 指定持久位置。开发实例、下载、克隆、缓存和编译均位于 `temp`；脚本只修改当前进程环境。已安装工具链继续使用。

HTTP 入口为 `/api`，Streamable HTTP MCP 为 `/mcp`。`nous mcp stdio` 连接运行中的服务。凭据默认读取实例内当前受保护文件；远程调用用 `--url` 与 `--token-file`。运行中通过 `identity issue --namespace <id> [--subject <id>] --name <name>` 创建范围凭据，`identity rotate/revoke/history` 管理轮换、撤销和历史；本地停服恢复使用 `identity reset-bootstrap`。Subject 可上传归自己的材料、读取已获授权的来源，并记录实际接触。

材料保存、实际接触和记忆形成各有独立身份。`observation record` 创建 `kind=contact` 的实际接触记录。模型形成请求先持久入队，模型调用在写事务外进行；保存输出后，由同一权威事务采纳 Memory 与来源关系。检索服务按合法快照维护 Tantivy 索引，并在返回前读取当前权威修订。

文本导入可直接生成可查询的忠实记忆，无需模型配置。将实际资料导出为 UTF-8 JSONL，每行包含 `external_id`、`text`，可带 `source`、`tags`、`metadata` 和 `structured`：

```powershell
$imported = & $nous --json --instance $instance import text --namespace $namespace --subject $subject --file ./temp/import.jsonl --format jsonl | ConvertFrom-Json
$importJob = (& $nous --json --instance $instance job wait $imported.result.job_id --timeout-ms 45000 | ConvertFrom-Json)[0]
$importJob.result.items | Select-Object index, input_index, record_index, status, error
$importedMemory = ($importJob.result.items | Where-Object admission | Select-Object -First 1).admission.memory_id
& $nous --instance $instance query --subject $subject '<实际资料中的主题>'
& $nous --instance $instance memory source $importedMemory
```

导入先保存精确输入原件和有序记录目录，再返回持久 Job。每条记录接纳独立文本 Material、`kind=import` 的来源事件、`origin=imported` 的忠实 Memory，以及标签、metadata 和 structured。`--form-memory` 可额外提交模型派生任务。结果按输入顺序给出 accepted、replayed 或 rejected，并标明从零开始的 input_index 和 record_index；整份文档解析失败时 record_index 为空，其他合法输入继续。`job get/cancel` 查看或取消导入，重启继续使用已冻结的目录。

`completion` 默认为 `accepted`，Job 在记录接纳处理完毕后完成。选择 `formed` 并设置 `form_memory=true`，Job 会在已接纳记录的实际形成任务全部成功后完成；等待期间可查看持久依赖。形成任务失败时父 Job 标明失败及实际依赖，已接纳资料保留。CLI 的 `--wait-formation` 同时启用形成与此完成模式，再用 `job wait` 等待该导入 Job。

相同调用方、Subject、external_id 和规范记录载荷复用原接纳，改变记录载荷返回 Conflict；form_memory 也参与该身份。另一份真实输入包含相同记录时，保留新的容器来源关联并重放原接纳。记录接纳、此次容器关联和任务进度同事务提交。来源结果分别展示记录文本与原文件信息，读取整份原件仍需该材料的来源权限。

HTTP 与 MCP 接受多文档 inputs，每项指定自己的格式。Material 输入使用已上传且获授权的材料 ID；inline 输入由服务保存原始 UTF-8 字节。CLI 可用 `--material <id>` 替代 `--file <path>`：

```json
{
  "operation_id": "import-notes",
  "namespace_id": "<NamespaceId>",
  "subject_id": "<SubjectId>",
  "inputs": [
    {
      "name": "notes.jsonl",
      "format": "jsonl",
      "input": { "kind": "material", "material_id": "<MaterialId>" }
    },
    { "format": "text", "input": { "kind": "inline", "content": "需要导入的实际记录" } }
  ],
  "form_memory": false,
  "completion": "accepted"
}
```

格式支持 `text`、`jsonl`、`json`；JSON 接受记录数组、`records`、A-Memorix `paragraphs` 和 LPMM `docs`。原生记录含 external_id、text 及可选 source、source_type、event_time、observed_at、tags、metadata、structured。整个任务的原始输入与规范化目录各限 4 MiB，最多 1000 条，单条限 1 MiB；所有输入严格按 UTF-8 解码，HTTP 另受实例请求体上限约束。成熟格式中的实体、三元组和时间载荷保存在 structured，后续由相应认知领域承接。

导入前可预览纯文本的实际拆分：

```powershell
& $nous --json --instance $instance import preview --namespace $namespace --subject $subject --file ./temp/notes.md --strategy narrative --narrative-window-size 1600 --narrative-overlap 400
& $nous --json --instance $instance import preview --namespace $namespace --subject $subject --material $materialId --strategy chat-log
```

`import preview` 返回原文 SHA256、字节数、生效配置、选中策略和有序 units；每个 unit 带全文件 ordinal、精确 UTF-8 `start_byte/end_byte`、原文文本、实际策略、场景或消息 context 及 `requires_llm`。支持 auto、narrative、factual、quote、chat-log；quote stanza 和超长单条消息保持完整。`imports.text` 配置提供默认窗口、overlap 与事实段落目标，命令参数覆盖本次选择。源文与返回 unit 文本目录各限 4 MiB，最多 1000 units。

预览不接纳材料、不创建任务，也不调用模型。`--file` 在调用端读取 UTF-8 原文；`--material` 沿原件读取权限访问已保存材料。HTTP 对应 `POST /api/imports/preview`，请求为 `{namespace_id,subject_id,document:{format:"text",input:{kind:"inline",content:"..."}},overrides:{strategy:"narrative"}}`；也可使用 `input:{kind:"material",material_id:"..."}`。MCP 对应 `import_preview`。时间参考仅采用明确传入的 TimePoint，预览不推断事件时间。

记忆变更带当前修订号，每次形成新修订和审计。以下操作冻结记忆、查看非活跃结果，再恢复：

```powershell
$memory = & $nous --json --instance $instance memory get $importedMemory | ConvertFrom-Json
$frozen = & $nous --json --instance $instance memory change $memory.id --action freeze --expected-revision $memory.revision --operation-id freeze-leaf | ConvertFrom-Json
& $nous --instance $instance query --subject $subject --include-inactive '枫叶'
& $nous --instance $instance memory change $memory.id --action restore --expected-revision $frozen.result.revision
& $nous --instance $instance memory audit $memory.id
```

`memory change` 支持 reinforce、weaken、protect、unprotect、freeze、forget、restore、trash、purge、revise、decay；revise 通过 `--edit` 提供 JSON 编辑对象。protect 的 `--hours` 为正时设置期限，省略或非正值时永久保护。freeze、forget、weaken 保留正文和修订；普通查询排除非活跃记忆，`--include-inactive` 或精确引用可读取。trash（也可用 `memory delete`）进入 `memory recycle-bin --subject <id>`；restore 恢复。purge 需要 Namespace 或管理员权限，默认等待软删除 24 小时，`--force` 可显式跳过期限；清除正文修订及相关模型输出、回执副本，保留最小身份和审计，原始 Material 继续由材料领域管理。

按批评估自动衰减时，默认每批 50 条，范围为 1..1000。返回值包含 snapshot_seq、next_cursor 和 complete，续扫同一快照直到完成：

```powershell
$page = & $nous --json --instance $instance memory maintain --subject $subject --limit 50 | ConvertFrom-Json
while (-not $page.result.complete) {
    $page = & $nous --json --instance $instance memory maintain --subject $subject --limit 50 --after $page.result.next_cursor --snapshot-seq $page.result.snapshot_seq | ConvertFrom-Json
}
```

维护跳过受保护对象；自动衰减达到阈值后进入 Decayed，持续 24 小时仍低于阈值时归档到回收站。手动 Frozen、Forgotten 和 Weakened 保留各自状态。

HTTP 接口与 MCP 工具进入相同应用用例：

| HTTP                                          | MCP 工具              |
| --------------------------------------------- | --------------------- |
| `POST /api/imports/text`                      | `import_text`         |
| `POST /api/memories/{id}/change`              | `memory_change`       |
| `GET /api/memories/{id}/audit?limit=50`       | `memory_audit`        |
| `GET /api/subjects/{id}/recycle-bin?limit=50` | `memory_recycle_bin`  |
| `POST /api/subjects/{id}/maintenance`         | `memory_maintain`     |
| `POST /api/embeddings/sync`                   | `embedding_sync`      |
| `GET /api/embeddings/runs/{id}`               | `embedding_run`       |
| `POST /api/query`                             | `query`               |
| `POST /api/references`                        | `references`          |
| `GET /api/memories/{id}/source`               | `memory_source`       |
| `GET /api/models/calls/{id}`                  | `model_call`          |
| `POST /api/models/contract`                   | `model_contract`      |
| `POST /api/models/preview`                    | `model_preview`       |
| `POST /api/prompts`                           | `prompts`             |
| `GET /api/diagnostics`                        | `diagnostics`         |
| `GET /api/subjects/{id}/diagnostics`          | `subject_diagnostics` |
| `POST /api/traces/query`                      | `trace_records`       |
| `GET /api/metrics`                            | Prometheus 文本       |
| `GET /api/config`、`POST /api/config`         | `config`              |
| `GET /api/jobs/{id}`                          | `job_get`             |
| `POST /api/jobs/{id}/cancel`                  | `job_cancel`          |

`.amembundle` 可在本地离线检查，无需启动实例：

```powershell
& $nous import bundle inspect --file ./temp/knowledge.amembundle
# 展开完整状态、来源引用与成员库存。
& $nous import bundle inspect --file ./temp/knowledge.amembundle --details
```

检查器承接上游 v1/v2 的知识、完整状态和图像包，核对成员校验和、语义摘要、记录引用与内容数量；默认显示 manifest、库存数量和所需领域能力。知识包管理通过 `bundle`、HTTP `/api/bundles`、MCP `bundles` 和官方 SDK `bundles` 使用相同领域操作。`export --preview` 预览选择内容，`export` 返回 `download_material_id`，用 `material get --output` 下载。`bundle inspect --material` 检查已上传的包，`install --material` 支持 `merge` 和空主体的 `restore`，`--chat` 指定安装会话。Subject 上传自己的包时，使用 `material put --owner-subject`。

默认导出原生 `.nousbundle` 完整主体快照。`--format amemorix --version <发布版本>` 导出标准 `.amembundle`，`--content-level knowledge|full` 选择知识或完整语义状态。可携带真实图片和向量；预览 manifest 中列出实际保留内容与空间采用状态。包描述中的版本用于标识所分享资料的发布。

导出的归档由导出主体持有，可使用该主体的凭据直接下载。原生包保存认知修订、来源、实际模型调用和已接受的向量；恢复后可复用原向量及其生产记录。卸载撤回该包拥有的活跃认知，保留历史与仍被其他对象使用的来源，随后可重新安装。

查询默认显示简短词序引用。`--json` 中的 `addresses.exact` 指定固定修订，`addresses.object` 指定稳定对象；后续可用 `@ref(...)`、`source read <词序引用> --subject <id>` 或 `reference resolve <词序引用> --subject <id>` 读取。新对象在写入事务内取得地址；已有 Namespace 可由操作者执行 `reference reconcile --namespace <id>` 补齐目录。知识包携带原地址映射，主体恢复后仍可解析资料中原有的引用。

[nous-vcp](crates/nous-vcp/README.md) 提供完整 VCP 计算族和旧版数值基线，包含 AGS 几何、传播资产、Pyramid、Sense、双场、RiverMemo、DTSC 与 Thinking。开发者可用 `cargo run -p nous-vcp --example native_pipeline` 重放完整计算和来源映射。实际文本查询已接通模型向量、USearch 候选/Tag 双索引、VCP 图资产和来源映射。

可选模型重排通过原生 `rerank` 协议和 `query_rerank` 模型角色配置，沿同一网络、预算、账本和持久任务执行。查询可设置 `rerank: "optional"` 或 `"required"`，CLI 使用 `--rerank optional|required`；默认关闭。结果保留基础得分，并提供独立的 `rerank_score`、实际任务与模型调用身份。精确读取、单候选及显式排序直接返回。

## 配置与模型资源

`instance.toml` 提供部署基线。启动时按注册的 typed 默认值、部署值、Instance、Namespace、Subject 覆盖顺序解析配置；每个 key 整体替换。运行中的配置管理返回 active、desired、值来源和修订号：

```powershell
& $nous --instance $instance config schema
& $nous --instance $instance config validate
& $nous --instance $instance config catalog
& $nous --instance $instance config show --namespace $namespace --subject $subject
& $nous --instance $instance config history --key models.catalog
& $nous --instance $instance config set --file ./temp/config-change.json
& $nous --instance $instance config clear --file ./temp/config-clear.json
```

`schema`、`validate` 离线读取部署文件；其余命令连接服务。省略范围参数时使用当前凭据范围。变更文件包含 `operation_id`、`scope` 和 `edits`，同一批可设置或清除多个 key：

```json
{
  "operation_id": "set-formation-route",
  "scope": { "kind": "instance" },
  "edits": [
    {
      "op": "set",
      "key": "models.roles.memory_formation",
      "expected_revision": 0,
      "value": { "routes": ["chat"], "max_calls_per_day": 20 }
    }
  ]
}
```

`expected_revision` 取 `config show` 的目标 scope/key 覆盖记录修订；首次覆盖使用 0。清除项为 `{"op":"clear","key":"...","expected_revision":123}`，恢复继承。服务对完整候选快照及受影响下级范围一起校验，然后原子提交。模型和媒体策略为 Live；server、network、tasks、platform、observability 为 Instance/Restart，待重启并由实际资源 owner 成功应用后更新 active。auth 的引导凭据由本地身份恢复入口管理。

模型配置使用资源图，职责通过有序 route 列表选择资源：

| 配置 key                        | 内容                                                                                                                                              |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `models.catalog`                | `providers` 保存 endpoint 和 SecretRef；`models` 保存 provider、协议、实际模型名、模态与能力；`routes` 绑定模型和调用参数。只允许 Instance 覆盖。 |
| `models.roles.memory_formation` | Memory 形成职责的 routes、Prompt、每日调用预算与重试策略。                                                                                        |
| `models.roles.<formation_role>` | Concept、MemorySemantics、Episode、Schema、Journal、Association 各自注册形成职责。                                                                |
| `models.roles.embedding`        | 文字、媒体与联合输入共用的向量职责；模型定义包含实际模型、维度、归一化和语义参数。                                                                |

职责绑定可在 Instance、Namespace、Subject 继承或覆盖。以下部署片段将两个形成职责绑定到同一条 route；`request_model` 替换为服务商实际模型名：

```toml
[models.catalog.providers.gateway]
endpoint = "http://127.0.0.1:3000/v1/"
credential = { file = "credentials/model-gateway.token" }

[models.catalog.models.chat]
provider = "gateway"
protocol = "openai_chat_completions"
request_model = "<chat-model>"
input_modalities = ["text"]
capabilities = ["text_generation", "structured_output"]

[models.catalog.routes.chat]
model = "chat"

[models.catalog.routes.chat.parameters]
max_input_bytes = 65536
max_output_tokens = 8192

[models.roles.memory_formation]
routes = ["chat"]
max_calls_per_day = 20

[models.roles.concept_formation]
routes = ["chat"]
max_calls_per_day = 20
```

Provider、Model 和角色绑定各有 `max_calls_per_day`。Provider/Model 拥有共享硬额度，角色额度按 Instance、Namespace、Subject 累计检查；降低或提高某个角色额度不会改变公共额度。一次外呼在同一事务预约所有适用额度，失败和不确定尝试保留计次。额度上调可唤醒等待中的任务。

模型的可选 `pricing` 使用明确的 `currency`，`input_per_million`、`output_per_million` 使用十进制字符串。调用记录分别保存规范 `usage`、冻结单价、估算金额和供应商实报金额；计数、币种或计价缺漏通过 `missing`、`invalid_fields` 表达。材料正文、模型生成和计费各自保留其实际结果。

配置只保存 SecretRef，密钥内容放入实例受保护的 `credentials` 目录。向量模型使用 `embedding` 能力与独立 route。`openai_embeddings` 接受文本批次；`ark_multimodal_embeddings` 接受一项由文本、图像等部分组成的联合输入，每次返回一个向量，因此 route 的 `max_batch_size` 设为 1。Prompt 选择为 `{"kind":"default"}`、`{"kind":"file","file":"prompts/custom.md","locale":"zh-CN"}` 或 `{"kind":"managed","reference":{"scope":{"kind":"instance"},"id":"custom","revision":123}}`。调用记录冻结实际模板、变量来源、消息、参数和 usage。

先查看职责输入合同及当前 Prompt，再用领域 typed input 文件构建动态输出 Schema：

```powershell
& $nous --instance $instance model contract --role concept_formation
& $nous --instance $instance model contract --role concept_formation --file ./temp/structured-input.json --namespace $namespace --subject $subject
& $nous --instance $instance model preview --role concept_formation --file ./temp/structured-input.json --namespace $namespace --subject $subject
& $nous --instance $instance model preview --role concept_formation --file ./temp/structured-input.json --prompt-file ./temp/formation-prompt.md --namespace $namespace --subject $subject
```

`contract --file` 通过正式领域构建器返回 messages、变量和动态 output Schema，未配置模型绑定时也可使用。`preview` 使用当前生效资源图与同一调用构建器，返回实际消息、媒体通道、Prompt、变量、输出合同和目标 JSON Schema；`--prompt-file` 仅替换本次预览。两者均为只读本地构建，不调用 provider。HTTP 对应 `/api/models/contract` 的 `{role,scope,input?}` 与 `/api/models/preview` 的 `{role,scope,input,edits:{prompt?}}`；MCP 工具接收相同请求。

`prompt catalog/read/history/edit/activate/preview` 管理提示词资源。`prompt edit --id <id> --file <正文文件> --expected-revision <修订>` 保存不可变正文；增加 `--activate-role <职责> --binding-revision <职责覆盖修订>` 可在同一事务激活。职责必须已有模型路线。独立激活使用精确资源选择；预览与真实调用使用相同 MiniJinja 渲染器。模板、变量来源、渲染正文和目标输出合同均可审查。

`nous diagnostics` 查看实际日志位置和丢弃记录数。JSON 日志按日保存在实例 `logs` 目录，策略通过 `observability` 配置管理。`nous trace <trace-id>` 按页读取提交、任务和调用关系；继续分页时将 `next_cursor` 传入 `--cursor`。HTTP 接受并回显 `x-nous-trace-id`，任务和调用持久继承接受事务的 trace。实例诊断、trace 和 Prometheus 指标入口需要管理员权限。

`nous stats --subject <Subject UUID>`、MCP `subject_diagnostics` 与 Python SDK `subject_diagnostics(subject_id)` 读取该主体及当前来源范围内的统计。认知数量按实际类别和生命周期汇总；任务、队列、维护完成时间、向量输入状态及模型用量取自同一 Authority 快照。本机消耗与去重后的 Imported 生产历史分栏，费用按实际货币分别汇总。尚无同义资源或队列 owner 的指标明确列入 `unknown`；本地资源就绪检查单独标注观察时间。

回执的 `time` 是取得写锁后捕获的本次事务时间，同一提交中的记录共用该时刻；幂等重放保留原时间。原始事件时间、主体接触时间和领域语义时间由对应输入与领域维护。

资源配置完成后，提交正式形成任务：

```powershell
& $nous --instance $instance model status
$formation = & $nous --json --instance $instance memory form --subject $subject --observation $observation --operation-id first-formation | ConvertFrom-Json
& $nous --instance $instance job get $formation.job_ids[0]
```

文本向量职责通过 `models.roles.embedding` 绑定资源图中的 route。配置就绪后，可显式补齐主体认知和来源标签的向量，再查询：

```powershell
$embedding = & $nous --json --instance $instance embedding sync --subject $subject --operation-id first-embedding | ConvertFrom-Json
$embedding.result.jobs | ForEach-Object { & $nous --instance $instance job get $_ }
& $nous --instance $instance query --subject $subject --mode dense '秋天有哪些植物变化？'
& $nous --instance $instance query --subject $subject --mode vcp '秋天有哪些植物变化？'
& $nous --instance $instance query --subject $subject --mode hybrid '秋天有哪些植物变化？'
```

`embedding sync` 返回持久任务 ID；`result.jobs` 包含本次新建和已在处理相同输入的任务。HTTP 提交 `POST /api/embeddings/sync`，JSON 为 `{"subject_id":"<id>","operation_id":"first-embedding","limit":5000}`；按 ID 读取 `/api/embeddings/runs/<job_id>`。MCP 使用 `embedding_sync` 和 `embedding_run {job_id}`。查询默认 `lexical`；HTTP/MCP 的 `mode` 同样接受 `dense`、`vcp`、`hybrid`。语义查询会补齐缺少的当前认知、标签和查询向量并等待结果；当前主体快照上限为 5000 条认知对象，超出时返回 ResourceExhausted。

标签取自来源 MaterialDetails.tags 的原文，去除首尾空白后生成稳定身份；同一 Material 派生的多个认知对象 在图证据中共用一个来源根。范围、来源、时间和活跃状态先约束候选与构图，命中返回前再读取最新权威修订。候选索引、Tag 索引和完整 VCP 资产按不可变代际一起发布；重启加载已发布资产，恢复与重建复用 SQLite 保存的向量原值。同一模型空间的相同查询复用持久查询向量，重启后再次查询无需远程嵌入。正文未变的生命周期修订也复用原向量和调用来源。

任务达到 `succeeded` 后，可查询记忆并读取其原文、实际接触、Prompt、模型参数及 usage：

```powershell
$result = & $nous --json --instance $instance query --subject $subject --kind memory '枫叶 秋天' | ConvertFrom-Json
$memory = $result.hits[0].object
$reference = $result.hits[0].addresses.exact
& $nous --instance $instance query --subject $subject "@ref($reference)"
& $nous --instance $instance source read $reference --subject $subject
& $nous --instance $instance memory source $memory.id
& $nous --instance $instance memory history $memory.id
# source 返回 embedding_provenance，可用其 model_call_id 展开实际请求与 usage。
$source = & $nous --json --instance $instance memory source $memory.id | ConvertFrom-Json
if ($source.embedding_provenance.Count -gt 0) {
    & $nous --instance $instance model call $source.embedding_provenance[0].model_call_id
}
$backup = & $nous --json --instance $instance backup create | ConvertFrom-Json
& $nous --instance $instance stop
& $nous --instance ./temp/instances/restored backup restore $backup.backup
```

所有任务使用统一 JobRecord；形成与嵌入调用同一 ModelCall 服务和按资源每日调用预约。`job get` 返回 `job`、`queue` 和 `model_calls`；等待时 `job.state` 为 `waiting`，`job.wait.kind` 区分 `configuration`、`budget` 等条件。用 `job cancel <id> --operation-id <id>` 取消已接受的工作。重启会恢复已接受任务，已保存的成功输出可继续采纳；中断的远程调用保留不确定状态及重试记录。相同范围、模型空间和源输入的活跃任务共用处理权，重复同步返回已有任务。备份包含数据库、已采纳向量、引用对象、运行时配置快照、绑定的 Prompt 文件和校验清单；恢复到新实例时生成新的管理员凭据，按 SecretRef 重新提供模型凭据。

结构化认知通过同一应用入口创建、修订、查询、展开来源和形成。支持 MemorySemantics、Concept、Episode、Journal、Schema、Association；Concept 可原子合并/拆分，Episode 可按完整来源序列重分段。精确修订证据区分支持、反驳与上下文，同一材料的派生路径共享来源根；Journal 保存写作时的知识截面。

`nous cognition --schema` 输出完整请求 JSON Schema，`nous cognition --file <request.json>` 提交请求。HTTP 使用 `POST /api/cognition`，MCP 使用 `cognition` 工具，三者接收相同的 `action` 判别请求：`create`、`revise`、`get`、`history`、`set_deleted`、`source`、`concept_resolve`、`merge_concepts`、`split_concept`、`resegment_episodes`、`form`、`formation_run`。所有认知使用唯一 `query` 用例：CLI 的 `--kind`、`--reference kind:UUID@revision`、`--as-of-commit`、`--include-inactive` 与 `--include-deleted` 对应 HTTP/MCP 的 QueryRequest。HTTP/MCP 的 `reference` 使用包含 `kind`、`id`、`revision` 的对象。命中包含 `reference`、`object`、`score` 和 `source_roots`；Memory、Concept、Episode、Journal、Schema、Association 共用词项及 Dense/VCP/Hybrid 查询。

形成请求的 `input` 包含 `subject_id`、精确 `sources` 和 `targets[{key,kind,existing}]`。服务读取已授权来源并分配稳定目标 ID，模型只从来源键与目标槽位选择。Concept、MemorySemantics、Episode、Schema、Journal、Association 分别通过 `concept_formation`、`memory_semantics`、`episode_formation`、`schema_formation`、`journal_formation`、`association_formation` 职责选择 route，按顺序创建有依赖的任务；后续任务读取前序已采纳对象。输入输出预算取 route 参数，重试取职责绑定。形成复用共享 ModelCall 与持久队列；每个提案组独立事务采纳，返回 applied/conflict/rejected/skipped，缺配置等待、重启续接、来源清除退役均保存状态。

形成作业的 `succeeded` 表示全部组已处理，具体成果和冲突读取 `formation_run` 的 `groups`。`source` 返回对象精确修订、证据闭包、原材料和实际模型调用。CLI 交互优化、managed context/steward 与完整 A-Memorix 合同继续按 [能力续接清单](docs/能力续接清单.md) 推进。

## 媒体与精确来源

`material put --file` 流式接纳文本、图像、音频、视频和文档；媒体服务统一执行大小策略、类型识别与元数据探测。`media inspect <material-id>` 读取元数据；`material get <material-id> --output <path>` 下载原件。HTTP 对应 `POST /api/media/materials` 和 `GET /api/materials/{id}/content`。

媒体工具在 `platform.executables` 注册为 `managed` 的实例 `bin` 相对路径或 `external` 的绝对路径。`media` 配置指定 ffprobe、ffmpeg、可选文档工具及采样、音轨和大小策略。进程工作目录、临时文件、取消与进程树收尾由 Platform 管理。

三种派生策略分别是 `description-only`（自由描述）、`direct-structured`（媒体直接形成结构解释）、`describe-then-structure`（先保存自由描述，再同时输入媒体与该描述形成结构解释）。对应模型职责是 `media_description`、`media_direct_structuring`、`media_structuring`；每个职责独立绑定模型和提示词。`--delivery sampled-frames` 使用有时间定位的采样帧和配置的音轨，实际覆盖随结果保存。

```powershell
& $nous media schema
$derived = & $nous --json --instance $instance media derive --subject $subject --observation $observation --material $material --strategy describe-then-structure | ConvertFrom-Json
& $nous --instance $instance media run $derived.job_ids[0]
```

任务完成后，用它的精确结果形成记忆或展开来源：

```powershell
$run = & $nous --json --instance $instance media run $derived.job_ids[0] | ConvertFrom-Json
$reference = @{ kind = 'representation'; reference = $run.result } | ConvertTo-Json -Compress
& $nous --instance $instance memory form --subject $subject --observation $observation --source $reference
& $nous --instance $instance media lineage --subject $subject $reference
```

`media region` 管理原件和派生区域，`media representation/history` 读取精确修订及历史；`media source` 返回所选内容与实际通道。HTTP `POST /api/media` 与 MCP `media` 接收同一操作合同。每个 paid step 使用共享调用账本；第二段可以复用已持久的第一段描述。Memory、结构化认知和查询展开完整来源图，备份包含全部保留修订引用的派生文件。

提供模型密钥时可使用 `secret import --file <已有密钥文件> --reference credentials/new-api.token`，返回可用于模型配置的 SecretRef。文件发布与权限由密钥服务管理。

## 原生多模态检索

检索目标统一为认知对象与精确媒体引用。`query --domain cognition`、`--domain media` 限定范围，省略时查询两类。`--reference` 接受认知引用及 `material:UUID`、`source-region:UUID`、`representation:UUID@revision`、`derived-region:UUID`。文字、原图和区域可组成同一查询输入：

```powershell
& $nous --instance $instance embedding sync --subject $subject
& $nous --instance $instance query --subject $subject --domain media --media "source-region:$region" '请求趋势折线图'
& $nous --instance $instance query --subject $subject --mode hybrid 'MaiBot WebUI 主题'
```

CLI 的文字查询默认使用 Lexical，提供 `--media` 时默认使用 Dense，可显式选择 Dense/VCP/Hybrid。HTTP 和 MCP 使用 `input: {text, media}`；`media` 是精确 MediaReference 数组。区域经共用材料服务实际裁切、持久化后送模型；调用记录保留裁切文件引用与完整派生关系。

Ark 原生向量资源的部署配置示例：

```toml
[models.catalog.models.vision_embedding]
provider = "gateway"
protocol = "ark_multimodal_embeddings"
request_model = "doubao-embedding-vision"
input_modalities = ["text", "image"]
capabilities = ["embedding"]

[models.catalog.models.vision_embedding.embedding]
expected_provider_model = "doubao-embedding-vision-251215"
dimension = 2048
normalization = "none"
parameters = {}

[models.catalog.routes.vision_embedding]
model = "vision_embedding"

[models.catalog.routes.vision_embedding.parameters]
max_batch_size = 1

[models.roles.embedding]
routes = ["vision_embedding"]
```

Provider 的 endpoint 为支持原生协议的基础地址，适配器追加 `/embeddings/multimodal`。`embedding.parameters` 保存 `instructions` 等实际语义参数；输出模型、维度、归一化与这些参数共同定义 Space，真实调用路线单独记录为 Producer。`normalization = "l2"` 会在公共模型输出层实际执行归一化。相同主体和空间中的相同内容可复用已采纳向量及原 ModelCall。

`retrieval` 配置管理输入、结果、快照、候选集、缓存、等待和融合参数，支持 Instance/Namespace/Subject 继承。本次查询先固定授权候选；材料准备和向量生成在读事务外执行，之后仅刷新该集合的向量。不可处理的投影通过 `coverage.projection_issues` 返回。向量代际、精确来源和实际用量随调用与备份持久保存。

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
