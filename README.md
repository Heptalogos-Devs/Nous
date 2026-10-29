# Nous Wave

Nous Wave 是拥有长期记忆、结构化认知、精确来源与可恢复工作的认知服务。Python 主应用负责领域规则、SQLite 权威事务、模型与管理入口；同进程 Rust 扩展负责 VCP、中文词项、向量检索与 NousQL 语法。

当前开发分支为 `codex/python-native-rewrite-cloud`，完整目标和仍在推进的能力见 [Goal](docs/GOAL.md)、[项目宪纲](docs/requirements/项目宪纲.md) 和 [能力续接清单](docs/能力续接清单.md)。迁移中的旧 Rust 应用保留作参考，日常命令进入 Python 与独立 `native/` 工作区。

## 开发与启动

应用使用 CPython 3.14，SDK 支持 Python 3.11+。需要 Rust 1.97.1、C/C++ 构建工具、pkg-config 与 FFmpeg。

```bash
source scripts/dev-env.sh
uv sync --locked
uv run nous init --instance temp/instances/default
uv run nous serve --instance temp/instances/default
```

打开 `http://127.0.0.1:7441/` 进入管理工作台。初始化生成的实例访问令牌位于 `credentials/admin.token`；令牌在浏览器中仅保留到断开连接或关闭页面。管理界面与 CLI、SDK、MCP 使用相同的公开操作。

[安装与发行](docs/python-native-install.md) 说明三包 wheel 的安装、原生平台要求、备份和恢复。发行包中的 native wheel 可直接安装，无需用户机器具备 Rust。

## 模型配置

生成与向量服务采用配置指定的 OpenAI 兼容连接。Provider、Model、Route、Role 分别维护连接、模型能力、参数与用途；没有固化的供应商 SDK 分支。可在管理工作台编辑模型资源。实例首次启动也可从环境设置初始连接：

```bash
export NOUS_MODEL__BASE_URL=https://api.openai.com/v1
export NOUS_MODEL__MODEL=your-generation-model
export NOUS_MODEL__CREDENTIAL_FILE=/protected/path/generation.key
export NOUS_EMBEDDING__BASE_URL=https://your-provider.example/v1
export NOUS_EMBEDDING__MODEL=your-embedding-model
export NOUS_EMBEDDING__CREDENTIAL_FILE=/protected/path/embedding.key
export NOUS_EMBEDDING__MAX_BATCH_ITEMS=10
```

也可通过 `NOUS_MODEL_API_KEY` 引用运行环境提供的凭据。代码与调用快照不保存 Key。文字向量使用标准 `embeddings`；图文联合向量使用模型资源中配置的内容片段扩展路径，默认 `embeddings/multimodal`。模型空间包含实际返回的模型版本、维数和归一化方式。

`max_batch_items` 按模型的实际请求限制配置；共享向量服务分批请求，并分别保存结果，重试复用已成功的批次。上例将每批限制为十项。

媒体解释保留自由描述、直接结构化、原媒体结合已保存描述后二次结构化三种路径。description 角色使用 output_mode=text，直接接收自然语言正文；形成、维护与重排使用 output_mode=json_schema 和各自强类型输出。预览同时返回实际输出模式和对应 Schema，字符串模式不要求模型返回 JSON。图片区域、视频/音频时间片段和 PDF 页区使用精确来源；日期和无时区表达保留其精度与时间域。

提供方支持 `video_url` 内容片段扩展时，在对应模型资源设置 `video_content_format: "video_url"`，或首次启动时设置 `NOUS_MODEL__VIDEO_CONTENT_FORMAT=video_url`。视频通过官方兼容 SDK 的结构化输出接口发送，响应原文、实际输入摘要与解析结果保存在同一调用账本。

工作台“媒体与派生”可打开正式地址，选择原件或派生内容的准确区域，查看谱系与修订，并裁剪、截取、提取音轨或采样。Memory 引用具体表示修订；来源清除会抹除相关派生正文和缓存，独立接触仍可保留其原件。

## 认知与来源

资料接纳保存原件、冻结 Catalog/Unit、实际 Import Observation、UnitOutput 和完整文件成功事实。形成、向量与持久任务分别记录真实状态。权威状态与 DBOS 调度位于同一个 SQLite 文件，领域提交和后续入队在同一事务完成。

Memory、Concept、Entity、Episode、Journal、Schema、Association、Self、Social、Language 与 Motivation 使用明确的领域记录和修订。检索支持中文词项、语义、Hybrid、VCP、NousQL、五轴时间与历史截点。来源权限同时约束候选、图计算和最终物化；撤回、恢复与清除由来源所有者处理。

Managed Context 保留 prepare、实际完整输入测量、admit、model contact、实际使用与结果反馈的区别。SDK 和 MaiBot 集成通过正式服务入口调用；A-Memorix 工具和管理协议消费同一认知权威。

认知成长可按需运行或按时区与 cron 定时生成提案。在“认知成长”中查看依据与动作，再选择采纳或拒绝；采纳对原对象修订进行冲突检查，并在一个事务内应用。定时任务持续生成可审阅的提案。任务与模型调用分别持久化，成功的付费结果可在重启后继续使用。

## SDK 与 MCP

Python SDK 的发行名为 `nous-wave`，异步客户端是 `nous_wave.NousClient`。公开命令与结果位于 `nous_wave.contracts`，导入合同不会启动服务或创建网络客户端。

```bash
nous mcp --namespace-id NAMESPACE --subject-id SUBJECT \
  --instance temp/instances/default --url http://127.0.0.1:7441
```

MCP 在启动时绑定命名空间和主体，可使用 `--credential-file` 指定受限 Key。工具包括记忆查询、接纳文本/摘要、人物资料、维护、来源、统计与完整 NousQL 查询。模型参数不能改变绑定的身份。

## 数据与验证

每个实例独立保存数据库、不可变内容和原生派生索引。在线备份通过 SQLite backup API 取得一致快照，并保存该快照引用的全部原件；恢复写入一个新实例。API Key 只保存哈希，支持来源范围收窄和立即撤销。

```bash
source scripts/dev-env.sh
uv run basedpyright
uv run ruff check src sdk/python/src native/python/python tests/python
uv run pytest tests/python native/python/tests
```

开发缓存、运行实例、上游源码和构建结果位于 `temp/`，不进入版本控制。原生源码变更后重新执行 `uv sync --locked` 并重启服务。Windows 与 macOS 由对应构建 runner 产生并验证 wheel；本地 Linux 构建按其实际平台标签使用。

## 查询结果消费

查询可请求来源展开与可选模型重排：`nous query <namespace-id> <subject-id> '项目操作者' --materialize --rerank optional`。复杂请求通过 `--request query.json` 传入同一公开 QueryRequest。工作台提供相同的来源展开选择与重排模式，并显示实际重排状态。

QueryResult.expression 返回带准确地址的正文表达；sources 是有界来源图，保留真实 evidence、观察与精确区域，不将区域读取扩大成整份原件。`budget.source_depth`、`source_nodes`、`source_text_bytes` 控制展开，complete 指明是否完整。

重排默认关闭。需要时在模型资源配置 `query_rerank` 角色，使用 purpose=rerank、text-generation 路线及 `query-rerank` Prompt。optional 在缺少模型能力时沿用基础顺序并返回 skipped 原因；真实提供方失败仍返回错误。required 要求模型能力并将失败返回。模型调用继续保存在现有账本，`rerank_invocation_id` 固定本次调用身份。详见 [SDK 查询消费](sdk/python/README.md#查询结果与来源展开)。

机器查询可通过同一 QueryRequest.expression 提交 atom/any/all 树；filters 选择正式命名来源、观察身份、来源类型、origin 字段与值、标签及认知语义。budget 的 candidates/branches/depth 限制实际候选与组合。capabilities.semantic/exploration 可设 required 或 optional；require_complete_vectors 默认 false，单独控制完整向量覆盖。execution 始终报告真实 effective_mode、missing_vectors、candidate_limited 与降级原因，关闭详细 diagnostics 后仍保留；Hybrid 覆盖不足时采用真实词项通道。authority 按实际认知、证据与解释分类；外部权威和资源描述要求对应资源 owner。current=prefer 保留本地结果并报告 external_freshness_unchecked，required 缺外部时效能力时返回错误；modality 按真实原件及同一来源根过滤。


## 出站网络配置

应用主动的模型、向量和媒体提供方请求由同一 NetworkService 管理。`network` 是正式配置 key，沿 instance → namespace → subject 覆盖；`nous config get network` 读取实际有效值与 origin，`nous config history network` 查看保存历史。配置保存后，后续调用使用新策略，活跃调用保留其原连接，结束后退役；应用关闭会等待租约释放并统一关闭连接池。

```json
{"trust_env":true,"proxy_url":null,"verify_tls":true,"ca_file":null,
 "connect_timeout_seconds":10,"read_timeout_seconds":120,"write_timeout_seconds":30,
 "pool_timeout_seconds":10,"max_connections":100,"max_keepalive_connections":20,
 "keepalive_expiry_seconds":5,"follow_redirects":false}
```

将 JSON 保存后，通过 `nous config set network network.json --scope instance` 设置。HTTP、SDK 和 MCP 的通用 configuration 操作支持相同 key；history 由同一配置 owner 读取。默认保留 HTTP_PROXY、HTTPS_PROXY、NO_PROXY 与环境 CA 支持；proxy_url 可显式指定 HTTP(S) 代理，代理凭据留在受保护的进程环境。Provider timeout_seconds 作为各阶段超时的额外上限；TLS 和连接池值使用 NetworkPolicy 的有效配置。SDK 外部客户端、DBOS 与可选进程级 OTEL exporter 各自由成熟 SDK 管理；遥测沿 OTEL 专用配置生命周期。

自由文本只在提供方明确返回 stop 时保存成功；length、拒绝和未确定完成原因会返回带真实调用身份的错误，付费原响应和实际用量仍在调用账本中保留。
