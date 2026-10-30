# Nous Wave Python SDK

异步客户端使用正式 Python 领域合同，供 Agent、MaiBot 和普通 Python 程序调用。安装：

```sh
uv add --editable /绝对路径/Nous-Wave/sdk/python
```

```python
from nous_wave import ClientOptions, NousClient
from nous_wave.contracts import AdmitFiles, ImportDocument, QueryRequest

async with NousClient(
    service_url,
    credential_file=protected_credential_file,
    options=ClientOptions(language="zh-CN"),
) as client:
    namespace = await client.create_namespace("研究")
    subject = await client.create_subject(namespace.id, name="助手")
    imported = await client.admit_documents(namespace.id, subject.id, AdmitFiles(
        inputs=(ImportDocument(text=actual_text, filename="notes.md"),),
        llm_enabled=False, vectors="disabled",
    ))
    result = await client.query_cognition(namespace.id, subject.id,
        QueryRequest(text="当前任务", mode="lexical"))
```

返回值为 Pydantic 合同或明确的 JSON 对象。`nous_wave.contracts` 可以独立导入，不加载 HTTP 客户端。`NousError` 保留服务端错误码、消息与上下文。写入重试复用同一 `operation_id`；网络错误保留该身份供继续。客户端连接、凭据读取、Trace、语言、超时、代理和下载校验共用一个 transport。

`ClientOptions.cognitive_clock=TimePoint(...)` 显式提供认知形成时钟；未设置时服务使用物理 UTC。Context 的 `usage.world_time` 仅描述用途中的世界坐标。作业调度、调用预算与实际用量使用物理时间。

## 来源与地址

```python
from nous_wave import SourceReadScope
from nous_wave.source_scope import CognitionSource, MediaSource, OriginalMedia

whole = client.with_source_read(SourceReadScope(
    subject_id=subject_id,
    source_refs=(MediaSource(reference=OriginalMedia(material_id=material_id)),),
))
single = whole.with_source_read(SourceReadScope(
    subject_id=subject_id,
    source_refs=(CognitionSource(reference=exact_cognitive_ref),),
))
```

范围可选择完整原件、精确区域、单个认知修订、Observation、工作上下文锚点或实际 conversation origin。认知与媒体 selector 均接受对应类型的 Proquint 地址，由服务端统一 ReferenceDirectory 解析。每个范围内选择器按 OR，多个范围与凭据授权按 AND 收窄；空范围返回空读取权限。单认知范围保持精确引用，读取容器原件仍需原件授权。

`resolve_reference(subject_id,address,as_of_commit=...)` 返回 `ReferenceResolution`。对象短地址在指定知识截点选择修订，固定地址保留精确修订。`source(subject_id,reference)` 读取认知或媒体来源；`memory_source(memory_id,revision=...)` 读取记忆修订的真实来源。`cognitive_revision(namespace_id,subject_id,reference)` 返回准确修订。

## 文件与导入

`upload_file`、`upload_stream` 流式接纳原件；上传必须指定 `owner_subject_id`。`expected_content_hash` 可核验实际上传字节。`download_material`、`download_region` 和 `download_derived` 使用相同 scope 元数据、长度与散列验证，失败会清理未完成文件。

```python
from nous_wave.contracts import FreezeCatalog, ProcessCatalog

material = (await client.upload_file(namespace_id, local_file,
    owner_subject_id=subject_id)).result
catalog = await client.freeze_catalog(namespace_id, subject_id, FreezeCatalog(
    inputs=(ImportDocument(material_id=material.id, format="json", filename="records.json"),),
    strategy_override="factual",
))
submitted = await client.submit_catalog(namespace_id, subject_id,
    ProcessCatalog(catalog_id=catalog.id, llm_enabled=True, vectors="metadata_allowed"))
job = (await client.wait_success((submitted.job_id,),
    namespace_id=namespace_id, subject_id=subject_id))[0]
```

JSON 接受 `records`、`paragraphs`、`docs` 包络，JSONL 保留记录顺序。每条记录可带 `external_id,text,source,source_type,event_time,observed_at,origin,tags,metadata,structured`，服务端保留原件与精确区域。拆分支持 auto、factual、narrative、quote、chat_log；quote 逐字保存并跳过形成模型。时间语境通过显式 timezone、reference_time 与 chat_reference_time 传给导入 owner。

`admit_documents(...,AdmitFiles(...))` 完成文件资格去重、冻结与处理，返回 `ImportResult`。`dedupe_policy` 为 none、content_hash 或 manifest；force 重新处理，clear_manifest 清除当前资格并保留历史事实。全部跳过时 `catalog=None`，跳过身份在 `skipped_success_ids` 中。异步管理创建即使全部跳过也返回真实 finalization 作业。

`import_files` / `import_directory` 先上传准确原件，再调用记录导入用例并返回真实 `TextImportSubmission`。文件可为路径，或 `{staged_path,filename,size}` / `{path,name,format,source,file_identity}`。SDK 不解析正文。本地扫描与文件路径限制在显式的 `transfer_root`；`ManagedFiles` 提供 raw/lpmm/maibot/converted aliases，拒绝路径或符号链接越界。原路径留在宿主。

`import_catalog`、`import_unit_output` 返回冻结 Unit 与不可变 Output。单位尝试保留实际 job_id、阶段、失败与可选 vector_error；未执行单位保持 pending。`vectors="required"` 要求向量完成，`metadata_allowed` 保留成功的正文 Output 及独立向量失败，`disabled` 不请求向量。

同一 Catalog 可以分批提交 `ProcessCatalog(unit_ids=...)`。默认继续会按实际来源、LLM 与有效形成选项复用已完成单元；已接纳正文但向量失败的单元只继续向量步骤。新请求会保留原 Output 和付费结果，不重新形成已经满足本次政策的单元。`import_attempts` 返回各次真实处理结果；旧 Output 的首次向量政策保持不变，后续完成情况由实际尝试和向量事实表达。

```python
# 继续剩余的冻结单元，不重新拆分或重做先前成功的单元。
continued = await client.submit_catalog(namespace_id, subject_id, ProcessCatalog(
    catalog_id=catalog.id, unit_ids=tuple(unit.id for unit in catalog.units[1:]),
    vectors="required",
))

# 明确重新形成所选单元。新 operation 产生新的模型调用意图；
# 同一 operation 重放保留该次实际付费结果。
redone = await client.submit_catalog(namespace_id, subject_id, ProcessCatalog(
    catalog_id=catalog.id, unit_ids=(catalog.units[0].id,), force=True,
    vectors="required",
))
```

每个文件的全部原始单元都达到本次处理政策后，系统从跨作业的真实成功尝试生成 `FileSuccess`。只修改部分单元的形成政策时，其余旧政策输出不会被算作新政策的整文件成功；需要整文件重做时选择全部单元并设置 `force=True`。`retry_parent_job_id` 必须指向同一 Catalog 的实际处理请求。HTTP/MCP 和 CLI `catalog process` 的 `ProcessCatalog` JSON 使用相同字段；不必改变全局 Prompt 来触发默认继续。

`job`、`jobs`、`wait_jobs`、`wait_success`、`cancel_job` 使用正式 DBOS 状态。等待采用有上限的退避和调用超时。失败作业保留已提交的业务事实。

## 查询与管理

`query` / `query_cognition` 支持 lexical、dense、hybrid、vcp，认知与媒体 domains、混合 parts、精确引用、事件时间、来源过滤及历史截点。`query_manual()` 返回当前原生 NousQL 手册。

`adopt_cognition`、`merge_concepts`、`split_concept`、`resegment_episodes` 使用共享合同；`update_memory` 接受正式 `MemoryChange` 生命周期命令。`adopt_seed` 保存显式配置文档及 SelfFacet 证据，`self_profile` 读取当前正式 Self 画像。

来源管理使用 `source_directory`、`preview_sources`、`trash_sources`、`restore_sources`、`purge_sources` 及共享 `SourceCommand`；恢复保留实际 Observation 身份。Bundle 使用 `export_bundle` 与 `install_bundle` 合同，A-Memorix 提供 ZIP 文件操作。导出产物保留实际依赖闭包，读取与来源 purge 由材料 owner 共同约束。

`configuration`、`prompts`、`model_contract`、`model_preview`、`model_call`、`backup` 接正式管理入口。`configuration_catalog()` 列出注册配置、默认值、Schema 与写入权限，`configuration_schema(key)` 读取单项定义；`configuration` 的 `list`、`schema`、`get`、`history`、`set`、`reset` 与 CLI `config`、MCP 使用同一操作。管理界面的“配置”页可编辑作用域覆盖、恢复继承值及取回历史值。`subject_diagnostics` 返回授权范围内的认知、向量、任务与实际 token 用量。不同凭据的 read/write/manage 能力由服务核验。

模型角色的实际输入、输出模式、对应 Schema 和 Prompt 可在调用前读取。`model_contract(ModelContractRequest(role="formation"))` 返回登记的输入/输出 Schema、当前 Prompt 和可选 variant；形成的 `text` 与 `media` 使用各自真实输出类型。`description`、`steward`、`maintenance`、`query_rerank` 同样使用实际消费者的合同。description 的 output_mode=text、输出 Schema 为 string，实际模型直接返回正文；其它强类型角色使用 json_schema。自由描述及两段解释的第一步均采用文本输出，应用保存正文与真实 producer_call_id；第二步仍同时传入原媒体和已保存描述。调用账本保留实际字符串及 actual_output_mode。自定义 Prompt 保留原内容，可在管理入口按实际用途编辑。

```python
from nous_wave.contracts import ModelContractRequest, ModelPreviewRequest

contract = await api.model_contract(ModelContractRequest(role="formation"))
preview = await api.model_preview(ModelPreviewRequest(
    role="formation", variant="text",
    input={"source_text": text, "source": {"source_type": "document"}},
))
```

Preview 与实际调用共用输入校验、Prompt 渲染、参数及调用快照构造，返回真实目标 Schema、模型与媒体摘要。媒体使用 `media` 中已有的 `MediaReference`，按当前来源权限读取精确原件或选区。此操作不读取模型凭据、不预留额度、不请求提供方；未登记的角色合同返回明确错误。CLI 使用 `nous model-contract ROLE NAMESPACE_ID` 和 `nous model-preview REQUEST_JSON NAMESPACE_ID`，HTTP 为 `POST /api/models/contract`、`POST /api/models/preview`，MCP 同名工具使用同一合同。

`whoami`、`credentials`、`credential`、`credential_history`、`create_credential`、`rotate_credential` 和 `revoke_credential` 使用正式凭据 owner。轮换要求 expected_revision，旧 token 立即失效，明文只在创建或轮换当次返回。

`job_page(namespace_id=...,subject_id=...,kind=...,state=...,limit=...,cursor=...)` 返回真实作业及 next_cursor。过滤先于分页，state 使用 DBOS 大写状态；续页保持首个快照的提交边界。

`derive_media(...,DeriveMedia(...))` 对实际源字节执行 crop、clip、audio_track 或 sample_frames，返回精确派生身份。`media_lineage`、`representation`、`representation_history` 与区域方法读取同一派生 owner。`download_derived` 读取已授权的精确内容；派生授权不扩大到父原件。`source_operation` 支持实际 log ID 或 commit_seq，`source_operations` 支持 before_commit_seq 分页。

`identity`、`cognition`、`sources`、`contexts`、`bundles` 与 `media` 接受公开 operation envelope，消费同一正式 owner；来源读取通过绑定客户端或请求 source_read 继续收窄。

## 每次模型调用的 Context

```python
from nous_wave.contracts.context import ContextUsage

binding = client.managed(
    namespace_id=namespace_id, subject_id=subject_id,
    session_id=actual_session_id, consumer_id="reply", instance_id=actual_worker_id,
    source_read=trusted_source_scope,
)
await binding.resume()  # 重建同一串行 worker 后读取已接纳 track
prepared = await binding.prepare(
    usage=ContextUsage(purpose="reply", role=actual_role, audience=actual_audience, intent=actual_intent),
    max_items=64, max_text_bytes=65536,
    current_sources=actually_required_sources,
)
applied = binding.apply(clean_host_input, prepared, renderer, serialize_input)
await binding.admit(applied, invocation_id=actual_invocation_id,
    history_generation=actual_history_generation,
    measurement=host_measure_complete_input(applied.input))
# 在此调用真实模型。
await binding.report(applied, invocation_id=actual_invocation_id,
    outcome=actual_outcome, occurred_at=actual_completion_time,
    uses=actually_observed_uses, output=actual_output_metadata)
await binding.close()
```

prepare 提交真实持久任务并等待。renderer 接收独立副本并返回完整输入；serialize_input 编码该实际输入。`HostInputMeasurement` 包含完整输入 tokens、真实输出预留、窗口与测量来源，超窗会拒绝。SDK 管理 RESET/APPEND 游标，应用后输入变动会使接纳失败。同一 session/consumer/instance 的已接纳完整投影持久保存；`resume()` 在相同 SourceRead 下恢复。`context_track` 返回投影与未接纳 preparation IDs，撤回来源时返回空投影和 reset_reason。已确认旧 worker 停止时，`resume(cancel_pending=True)` 通过原取消操作接管遗留准备。`close()` 释放本 binding，保留会话与已接纳 track。没有显式来源范围时 managed 采用空读取范围。

`open_context_session` / `context_session` / `close_context_session` 管理真实宿主会话。`change_context_residents` 使用 `ChangeResidents.expected_revision` 修改居民；`change_work_context` 支持创建、编辑、暂停、恢复、结束、pin/unpin 与清理。工作指令和显式 current_sources 是必需项，预算不足会报错。

`contexts` 配置限制 consumer 预算与来源类型。配置 `steward_role` 后使用统一模型资源、Prompt、预算与账本；Steward 只选择当前授权候选，综合保持独立归因。居民使用 DBOS schedule 维护，模型接触、实际使用和完成结果分别记录。

A-Memorix `AMemorixAdapter` 保留六项工具、来源、导入、包与运行时管理入口。完整 MaiBot 补丁、模型窗口配置与真实发送边界见 [MaiBot 集成](../../integrations/maibot/README.md)。

## 认知整理提案

`start_maintenance` 提交实际模型生成作业；用 `maintenance_run` 读取快照、提案与真实 producer_call_id。`review_maintenance(...,ReviewMaintenance(expected_revision=...,decision="adopt"或"reject"))` 明确审阅，采纳复用认知 owner 的正式变更规则。来源当前不可用时 `sources_available=False`，正文与提案不会返回。

`create_maintenance_schedule`、`maintenance_schedules` 与 `stop_maintenance_schedule` 管理正式 DBOS 重复运行策略。工作台提供同一提案审阅与计划操作；每次计划生成待审提案，审阅决定单独保存。

`freeze_import_catalog`、`process_import_units` 与 `admit_import_files` 也接受原有单 Mapping 请求：`{namespace_id,subject_id,inputs:[{name,format,input:{kind:"inline",content:...},source}],overrides:{strategy:...}}`。`input.kind="material"` 使用已上传的 material_id；文件文档的 `{document,file_identity}` 包络继续可用。Mapping 文件接纳返回 `FileImportSubmission`；`submit_admit_documents` 是对应 typed 异步入口。

`import_text` 接受同一文档形状及 form_memory/completion，返回真实父作业。等待 `wait_success((submission.job_id,),...)` 后，`text_import_result` 读取对应结果。`accepted` 完成完整逐字记录接纳，并仅返回已排入队列的 formation_jobs；`formed` 要求 form_memory=True，父作业等待这些实际子作业成功。语义形成复用逐字 Memory 的精确修订，保留独立模型调用身份。每解析记录保持一个完整 Unit；输入总字节上限 4 MiB。

## 查询结果与来源展开

`query_cognition`、HTTP 的 Subject `/query`、MCP `query_cognition` 和 CLI `query` 使用同一 QueryRequest/QueryResult。可用请求文件完整指定查询；CLI 也提供 `--materialize`、`--rerank off|optional|required` 和 `--rerank-invocation-id`。

```python
from nous_wave.contracts import QueryRequest

result = await client.query_cognition(namespace_id, subject_id, QueryRequest(
    text="项目操作者", materialize=True, rerank="optional",
    rerank_invocation_id="operator-search-1",
))
print(result.expression)
if result.sources:
    for node in result.sources.nodes:
        print(node.key, node.status, node.text)
print(result.rerank)
```

expression 按实际顺序给出准确地址与正文摘录；source graph 按真实 evidence、观察和精确媒体区域展开，保留边的极性与角色。budget.source_depth/source_nodes/source_text_bytes 限制展开；complete=false 表示预算截断或部分来源当前不可读。`source(subject_id, cognitive_reference)` 的 graph 使用同一来源规则，媒体节点继续沿其精确 reference 读取。

rerank 默认为 off。启用时，模型角色 `query_rerank` 使用 text-generation 路线、purpose=rerank 和 Prompt `query-rerank`，提交有界正文候选并保存实际调用。optional 缺少可用模型角色、凭据或 Prompt 时保留基础顺序，结果附 skipped 原因；实际提供方失败返回带 call_id、invocation_id 和 role 的错误。required 传播缺能力与调用失败。显式 `$sort` 优先，required 重排与其冲突；未评分候选保留基础相对顺序。hits.score 保留检索分，rerank_score 为实际模型分数。重用 invocation 身份与相同候选请求会复用已保存结果；未知付费结果需新身份才能再次请求。

检索算法配置使用共享 `configuration` 的 `retrieval` 键，按 instance、namespace、subject 继承。`RetrievalPolicy` 包含融合权重、RRF 常量，以及实际 VCP pipeline、spike routing、readout 和诊断参数；初始值来自 native 的 Serde 默认定义。先读取有效对象再修改，保留完整强类型结构：

```python
from nous_wave.contracts import RetrievalPolicy

effective = await api.configuration({"key": "retrieval"})
policy = RetrievalPolicy.model_validate(effective["value"])
await api.configuration({
    "action": "set", "key": "retrieval", "scope": "subject",
    "value": policy.model_copy(update={"dense_weight": 0.8}).model_dump(mode="json"),
})
changes = await api.configuration({"action": "history", "key": "retrieval"})
```

查询在开始时冻结同一份有效策略，组合分支共用它。诊断中的 `plan.policy` 给出 namespace、subject、配置来源和实际策略；`plan.native` 保留原生计算诊断，组合查询在 `plan.branches` 中呈现各分支。`$diagnostics(none|summary|full)` 控制详细程度，`budget.diagnostics_bytes` 限制每次查询的原生诊断输出，超限会明确标记 truncated。VCP 的 BM25 lane 使用同一授权快照的真实词法命中，`bm25K` 参与原生 readout 选择；Ghost tags 尚无向量输入接纳 owner，未作为配置项开放。

机器查询使用 `expression` 的 atom/any/all 树，每个 atom.source 是同一 NousQL 语法。`filters.sources` 按正式命名来源解析器选择完整标签；也可设置 source_ids、source_types、origin（field/value 对）、tags、cognitive_roles、formation_modes 和 epistemic_classes。`budget.candidates`、branches、depth 约束候选与组合树；来源和重排分别使用 source_* 与 rerank_* 预算。

```python
from nous_wave.contracts.query_conditions import QueryAny, QueryAtom, QueryFilters, QueryBudget, QueryCapabilities

request = QueryRequest(
    expression=QueryAny(children=(QueryAtom(source="项目"), QueryAtom(source="操作者"))),
    filters=QueryFilters(tags=("project",)),
    budget=QueryBudget(candidates=256, branches=16, depth=6),
    capabilities=QueryCapabilities(semantic="optional", exploration="optional", require_complete_vectors=False),
    mode="hybrid",
)
result = await client.query_cognition(namespace_id, subject_id, request)
print(result.execution)
```

semantic/exploration 的 required 要求实际计算通道可用；require_complete_vectors 默认 False，独立控制是否要求完整向量覆盖。Hybrid 向量覆盖不完整时真实降为词项检索，execution 返回 effective_mode、missing_vectors、candidate_limited、degraded 和 reasons；组合不同通道时 effective_mode 为 mixed。`$diagnostics(none)` 只关闭详细计划，execution 仍保留。authority 支持 subject_cognition、evidence、interpretation 的实际分类；external_current_authority 与 resource_descriptor 缺少资源 owner 时返回 capability_unavailable。current=none 为默认；prefer 保留本地结果并报告 external_freshness_unchecked，required 要求外部时效 owner。modalities 支持 text/image/audio/video/document/binary，按真实原件模态及同一来源根过滤；图片的派生描述仍保留图片来源模态。

A-Memorix 图谱搜索使用 `adapter.memory_graph_admin(action="search", query="项目", limit=50)`；`invoke("memory_graph_admin", {"action":"search","query":"项目"})` 供原 MaiBot RPC 直接调用。结果为实际实体与关系，匹配优先级依次为精确、前缀、子串；每项 reference 经 `CognitiveRef.model_validate(item["reference"])` 读取为强类型后，交给 `client.source(subject_id, reference)` 展开来源。读取受当前 SourceReadScope 与认知可见性约束。

`client.graph_snapshot(namespace_id, subject_id)` 读取正式图谱目录。`client.manage_graph(namespace_id, subject_id, GraphChange(action="create_node", name="项目", reason="调用者提交的图谱登记"))` 创建有来源的 Entity；其余 action 支持改名、软删除、创建关系、修改权重及删除关系，返回实际报告 Material、Import 观察与认知修订。更改现有身份可传 exact `reference`，旧修订会返回 revision_conflict。`weight` 与 `epistemic.confidence` 分开保存，未知权重保留 None。

`client.configuration({"action":"get","key":"network","namespace_id":namespace_id,"subject_id":subject_id})` 读取应用出站网络有效策略与 origin；`action="history"` 读取同一 scope 继承范围的正式配置历史（limit 1..1000）。network 控制应用 Provider 的代理、环境信任、CA/TLS、分阶段超时和连接池；策略更新在下一次调用边界生效，已运行调用的租约结束后关闭旧池。外部 SDK 自身的 Transport 保持独立。`retrieval` 使用同一配置 get/set/reset/history 入口，默认值来自正式 Native schema。

人物目录、画像证据和别名可通过 `adapter.memory_profile_admin(action="list"|"query"|"evidence"|"get_aliases", ...)` 查看，原 `get_person_profile` 复用相同人物与证据读取。person_id 接受实际 MaiBot correspondence 或 Native Entity 身份；person_keyword/keyword 接受实际主名与别名，选择歧义会明确失败。普通读取不付费。显式 `force_refresh=True` 会提交并等待该 Person 的真实模型摘要任务，需配置 `person_profile` 模型角色；自动结果保留人工覆盖。画像证据的 evidence_key 是准确 Entity 修订及属性 JSON pointer，reference 和每条 evidence 保留真实来源，可继续使用公开来源读取。

人物管理使用同一 Entity 修订：`memory_profile_admin(action="set_aliases", person_id=..., aliases=[...])`、`delete_aliases`、`set_override`（`override_text`）与 `delete_override`。空别名列表是显式覆盖，删除覆盖后显示自动别名。`correct_evidence` 接受 `evidence()` 返回的确切 `hash`/`evidence_key` 与 `evidence_type="entity_attribute"`；`refresh=False` 只提交纠正，默认再刷新单人物。管理输入与实际调用者保留为来源。SDK 在传输前为写入及强制刷新建立 operation_id，重试应复用该值。

Native 客户端 `change_person_profile(ns, sub, PersonProfileChange(...))` 返回实际修订及输入来源；`refresh_person_profile(ns, sub, RefreshPersonProfile(...))` 返回真实 job_id，通过 `wait_success(job_id)` 等待。两者的 HTTP 路径是 subject 下 `/person-profiles/change` 与 `/person-profiles/refresh`。CLI `nous profile ns sub --request request.json` 和 MCP `memory_profile_admin(command=...)` 消费同一个 `ProfileAdminInput`。

Context 的 `ContextReport.output` 保存真实模型、provider、response_id、status、输出 item IDs 与 tool call IDs；未知字段保留 None。每次实际 wire 调用各自 admit/report，相同准备可用于真实 provider 参数回退，来源与预算每次重新校验。`contexts(action="track",...)`、CLI `context track` 与 MCP 通用 contexts 接口读取同一正式 track。
