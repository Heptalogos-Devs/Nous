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

媒体解释保留自由描述、直接结构化、原媒体结合已保存描述后二次结构化三种路径。图片区域、视频/音频时间片段和 PDF 页区使用精确来源；日期和无时区表达保留其精度与时间域。

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
