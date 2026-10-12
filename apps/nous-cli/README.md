# Nous CLI

[仓库地图](../../INDEX.md) · [Applications](../README.md) · [Agent 使用](../../docs/agent/README.md)

命令实现按操作类别放在 [commands](src/commands/index.ts)；Terminal 与 MCP 调用同一个命令核心。`runtime.ts` 固定本次选择和输入，`state.ts` 管理 consumer 事务，`friendly.ts` 只处理呈现。测试和测试用 Client 在 `tests/`，不随 CLI/MCP 打包。

第一方 reference consumer，通过 `@nous-wave/client` 调用 Core；命令使用 citty 0.2.2，复杂输入使用 smol-toml 与 Zod。CLI 的选择、查询结果索引和冻结原请求 保存在 InstanceRoot，属于 consumer 本地状态。

默认 consumer 是初始化 operator policy 中的 `default`；需要 query/projection 的自定义 `--consumer` 应在 operator 配置中存在同名 policy。

共享状态按 Core discovery 中的稳定 instanceId 与 `--consumer` 共同隔离，目录为 `InstanceRoot/consumers/<身份摘要>/`。Core 重启保留实例身份；同一 state root 中不同实例或 consumer 使用不同选择和回执。每条命令固定开始时的 Subject、Session、WorkContext 与输入，后续 RPC 不重读另一条命令的选择。原子写与跨进程锁仅用于本地短事务，不覆盖 RPC 或模型等待。独立字段更新合并；同字段、Subject 或 query context 冲突保留业务结果并返回 `STATE_UNSAVED` notice，后续操作使用返回的 exact references。

`consumer_state` 配置提供回执容量、文件字节预算和锁时限，完整 active policy 在命令开始时固定。当前 selection/operation 各使用一个 format 标识，原请求 codec 保留 BigInt 与任意 JSON 正文键的区别；旧 consumer 格式不由当前生产入口读取。未知回执的原请求与身份须在清理旧运行材料前保全。

按[根 README](../../README.md)准备开发环境并运行 `corepack pnpm dev`，另一个终端可执行：

```sh
corepack pnpm nous subject create
corepack pnpm nous session open
corepack pnpm nous observe text --text "实际来源的有界原文" --source "https://example.com/source"
corepack pnpm nous form <返回的 obs:词汇引用> --tag <返回的 Tag 词汇引用>
corepack pnpm nous context create --purpose "继续部署评估" --text "当前任务、限制和未决问题"
corepack pnpm nous context foreground
corepack pnpm nous query 'Alice 的部署项目进展怎样？ $return(memory,schema) $limit(5)'
corepack pnpm nous query '后续还有什么限制？'
corepack pnpm nous show result:1
corepack pnpm nous trace result:1
corepack pnpm nous context pin --cognition result:1 --entity entity:alice --tag <Tag 词汇引用>
corepack pnpm nous use result:1 --kind referenced
```

默认输出语义文本，正常操作使用 `result:N`、稳定 LexicalRef 或能唯一解析的名称。Subject、Session、WorkContext、认知 revision 和 Material 来源都返回可再次输入的词汇引用，例如 `sub:titil-lamat-napor`、`ctx:guhur-muguz-pojij`。这些引用由 Authority 持久保存，跨进程与 consumer state root 有效；不是 UUID 的截断，也不随标题修改改变。相同名称有歧义时，使用明确的词汇引用。精确 revision 的词汇引用仍指向原 revision。

`--subject`、`--session`、`--work-context` 与各命令引用参数接受返回的词汇引用。`identity bind --kind <kind> --canonical <词汇引用> --name <名称> --alias <别名>` 可显式设置可读名称；普通显示不会覆盖既有名称或别名。原始来源、Memory 内容与自由文本保持原文，因此历史资料中已有的 UUID 不被改写。

地址由对象创建事务或显式 Identity 服务分配；正常呈现依据协议声明的引用字段去重并只读批量查询。用户 JSON 中的 `$typeName`、`$unknown`、`subjectId` 等键保持原值，不因字段名或字符串形状转换。Query 的 `ref` 是当前 canonical `{kind,value}`，`result:N` 保存 exact reference，正常文本的 `lexicalRef` 优先指向该 exact revision。`query prepare` 直接展示 owner 冻结的 boundQuery 与能力要求；详细 query trace 由 `--developer` 保留。

`--json` 返回 `schemaVersion="nous.cli.v1"` 的机器 envelope，保留 canonical IDs，int64 使用十进制字符串。native message 采用 Buf 的 canonical Protobuf JSON：oneof 使用实际字段名、Timestamp 为 ISO 字符串、bytes 为 base64；任意用户 JSON 不按字段形状重解释。`--developer` 保留诊断身份和 query trace，`--raw --developer` 显式选择原始 Client DTO。正常文本省略这些内部追踪身份。成功仅写 stdout，错误仅写 stderr 并返回非零码；错误保留 code、message、details、candidates 和未知结果的 receipt。

Launcher 提供 RunRoot/InstanceRoot。`--subject`、`--session`、`--work-context` 覆盖本地选择。查询和幂等修改需要 InstanceRoot 保存续接状态；只读查询准备、配置与 status 可仅指定 RunRoot。Query 的 `result:N` 按实际命中顺序保存真实引用，并限定到原 Subject。Memory、Schema、Episode、Journal 保留 exact immutable revision，`show/trace/use` 不替换成最新 head。Evidence/Resource 与混合 hits 正常展示，每条只提供 owner 支持的后续动作；外部 Resource records 单独显示稳定来源身份。

Client 直接返回 canonical generated Protobuf，CLI 只在呈现边界读取 generated descriptor 并输出文本/JSON；不建立平行 DTO 或 schema 恢复层。正文与 freeform Struct/Value 在所属字段保持不透明。查询已返回的 lexicalRef 直接沿用，避免重复目录查询。

按[官方 Agent 手册](../../docs/agent/NOUSQL.md#prefer-explicit-referents-for-retrieval)强烈建议显式化能够可靠辨认的指称，使用名称和关键词增强 lexical、embedding 与 Entity/Tag activation；不确定时仍直接查询原文。一次创建并 foreground 的 WorkContext 可连续供多条问题使用，任务变化时才更新文本或 anchors。

`context set --text <text>` 或 `--file <path|->` 更新自由文本；`--purpose` 可同时修改目的。`pin/unpin` 使用 `--cognition`、`--entity`、`--tag` 的逗号分隔引用；cognition 只接受 exact revision 或 Occurrence。`clear --scope text|anchors|all` 清理相应字段；`pause/resume/end` 管理生命周期，`select <id>` 保存选择，`foreground [id]` 在选中 Session 激活，`foreground --clear` 解除激活。所有更新保留未修改字段并提交读取到的 expected revision。context_text 上限 64 KiB，与 query representation 预算分离。

修改前保存 operation identity、规范输入、时间戳和 expected revision。响应失败返回 `retry <id>`；重试读取冻结原请求，不重新读取新的 revision、重新解析引用或生成 operation ID。Observe、formation、config override、Memory/Schema/Episode/Journal、Tag/Association、WorkContext 和 UseEvent 使用各 owner 的幂等身份。成功响应后删除本地原请求；结果与拒绝语义只由 owner 决定，本地不缓存终态。`retry forget <id>` 删除不再需要的重试输入，不撤销已提交操作。Session lifecycle、Identity bind 和 derivation 的领域合同单独决定重试行为。

复杂 Tag/Association 文件为语义 TOML，上限 64 KiB；没有 Client DTO JSON 输入模式。例如：

```toml
# revise-tag.toml
 target = "<返回的 Tag 词汇引用>"
 label = "新的概念名称"
 description = "概念语义"
```

```toml
# merge-tags.toml
survivor = "<保留的 Tag 词汇引用>"
retired = ["<合并的 Tag 词汇引用>"]
[[basis]]
ref = "<返回的 memrev:词汇引用>"
role = "direct"
epistemic_relation = "corroborates"
```

```toml
# association.toml
from = "<返回的 memrev:词汇引用>"
to = "<返回的 Tag 词汇引用>"
relation = "tag_attachment"
polarity = "positive"
basis_class = "host_explicit"
[[basis]]
ref = "<返回的 obs:词汇引用>"
role = "contextual"
```

`tag revise|merge|split --request-file <file>` 解析目标 Tag 当前修订后冻结请求；split 使用 `parent`、`[[children]]` 的 label/description/kind_hint 与 `[[basis]]`。`association create --association-file <file>` 读取 from/to/relation/polarity/basis_class/basis。`tag attach <exact cognition> --tag <tag> --association-file <file>` 的文件只有 basis，CLI 固定 exact cognition→Tag attachment。Basis role 为 direct/interpretation/contextual，epistemic_relation 独立表达 supports/contradicts/corroborates/weakens/corrects/counterexample/inferred_from；Association 也可用 `[[basis]]` 内的 `use_event={consumer="...",event="UUID"}`。Authority 验证 Subject、exact references、catalog 与 lifecycle。

`help` 与 `help nousql` 不连接 daemon。[NousQL Agent 手册](../../docs/agent/NOUSQL.md) 提供 Unicode 意图、可选语法岛、五轴时间、history/asof、direct/explore 与 projection 的可执行示例。`query prepare|inspect` 不调用 provider，输出冻结 representation、context snapshot 和能力降级；普通 query 使用同一 preparation 协议执行。

`show` 显示 cognition revision 内容或 Material 对象元数据。核对原始来源时，复制返回的 `obs:`、`src:`、`art:`、`repr:` 或 `region:` 词汇引用执行 `read <引用>`；也可续接 `read result:N` 的 Material hit。`read <art:引用> --output <新文件>` 流式导出完整 Artifact，大小不受 preview 上限限制，失败清除部分输出。读取通过 Material Authority，默认最多 64 KiB，`--max-bytes` 可设到 1 MiB，返回实际范围、总量与 partial。文本材料返回原文，二进制材料返回元数据和 derive 提示。

`maintenance grant --max-operations 2 --max-model-calls 1 --max-elapsed-ms 30000` 明确授权有界维护；每次实际模型调用都计入预算，包括 execution fallback。Use 接受 exact cognition revision，refutation 是负反馈，不能自动授权模型活动或概念修改。Tag list/search 与 association neighborhood 维持有界分页和遍历。

Query 返回本 consumer 的 `query:last`。`use result:N` 自动关联该查询；通过词汇引用反馈时可写 `use <memrev:引用> --kind referenced --query-id query:last`。`query:last` 与 `result:N` 都在下一次 query 时更新，失败的新 query 清除它们，不能跨 Subject 使用。

## 多 Agent 与 stdio MCP

同一个真实 Core 可以服务多个 Agent。每个 Agent 使用独立 InstanceRoot 保存选择、result:N 和 receipt，RunRoot 指向同一个 Core discovery；长期 cognition 仍按 Subject 共享。源码入口示例：

```sh
node --import tsx apps/nous-cli/src/main.ts --run-root <Core RunRoot> --instance-root <Agent 私有目录> --consumer consumer:codex:research help
```

复用长期 Subject 时运行 `subject use <返回的 sub:词汇引用>`，每个并行 Agent 单独 `session open`。不要复制 Core Authority 数据库来隔离本地选择。

`nous --locator <bootstrap.toml> mcp --state-root <Agent 私有目录> --consumer consumer:codex:research` 启动官方 MCP SDK v2 stdio consumer。MCP 必须显式指定私有 state root 和稳定 consumer；它通过参数数组在进程内调用 Terminal 共用的命令核心，并使用同一跨进程状态事务。并发命令各自冻结输入，长操作不阻塞其他调用。stdout/stdin 仅用于 MCP 协议；MCP 输入使用文件或 `--text`，不接受文件 `-`。三个工具为 `nous_help`、`nous_command`（`args` 是 argv 字符串数组，不是 shell 命令）和 `nous_query`。错误保留 CLI 文本与未知结果 receipt；用 `nous_command` 调用 `retry <receipt>` 恢复。

Codex 项目 `.codex/config.toml` 的源码配置示例，替换全部绝对路径：

```toml
[mcp_servers.nous]
command = "C:/path/to/node.exe"
args = ["--import", "tsx", "C:/path/to/Nous-Wave/apps/nous-cli/src/mcp-main.ts", "--run-root", "C:/path/to/Core/run", "--state-root", "C:/path/to/Agent/state", "--consumer", "consumer:codex:research"]
cwd = "C:/path/to/Nous-Wave"
tool_timeout_sec = 960
```

Portable 使用包内 Node 与 `program/cli/mcp-main.js`，去掉 `--import tsx`。其他支持 stdio 的 Host（包括 OpenCode）可复用同一 command/args。Agent 从 `nous_help` 和 `nous_help` 的 `topic="nousql"` 开始，然后通过 `nous_command` 选择 Subject、打开 Session、恢复 WorkContext。每个并行使用者在配置中绑定自己的 state root/consumer；同一 Agent 重启保留该目录与 Subject ID。

视频输入配置与派生输出是两个选择。`derive <src:引用> --strategy description_only|direct_structured|describe_then_structure` 选择输出流程；`frames` 是 `video` 配置对象内的 `input_mode` 值。用 `nous config describe video` / `nous config get video` 查看 JSON Schema、完整值及应用方式；修改整块对象时保留其现有限制，并按回执重启 Core。帧采样作为有界模型预处理，解释会保存原视频来源及采样时刻/coverage；当前不提供独立 frame Artifact 的导出命令。

`session show <reference-or-name>` reads Session metadata, including a closed
Session, without opening or selecting it. With no argument it reads the selected
Session. Closing a Session clears consumer selection; its returned stable address
remains readable through the explicit form. `session open` and `session close`
accept no reference argument.

## 完整 owner 管理命令

`memory form|get|list|revision|history|revise|accessibility|link|suppress|restore|withdraw|reaccept|purge`；`schema create|get|revision|evidence|revise|split|merge|suppress|restore|withdraw|reaccept|purge`；`episode create|get|list|revision|history|revise|link` 加同样 lifecycle；`journal get|list|revision|history` 加 lifecycle。这些都是实际 Authority 操作，MCP 的 `nous_command` 可直接使用。

Memory correction 示例：

```toml
text = "核对原始来源后的完整修正内容"
semantic_role = "reported_fact"
epistemic_class = "reported"
formation_mode = "grounded"
grounding_occurrence_id = "<obs:引用>"
[[basis]]
ref = "<obs:引用>"
role = "direct"
```

`nous memory revise <Memory对象引用> --kind correct --request-file correction.toml` 读取当前 epoch 后冻结语义输入；`memory form --kind declarative --request-file content.toml` 显式形成源支持 cognition，无需 provider。创建时的 `--kind` 指定对象级 cognitive role，修订不会修改该角色；两者使用相同内容文件。`memory history <Memory对象引用> --page-size 50 --page-token <token>` 续接分页。`revision` 参数是精确修订，`get/history/lifecycle` 参数是稳定对象。生命周期恢复与修正可以读取 suppressed/withdrawn 对象；purge 由 owner 的不可逆语义处理。

Schema content 使用 title、structural_claim、applicability_scope、boundary_definition、formation_kind、evidence_links。`schema evidence` 文件是一项 `SchemaEvidenceLink`；split 文件用 `[[children]]`；merge 文件含 `sources=["<Schema引用>",...]` 与 `[merged]` 完整内容。`memory link`/`episode link` 文件含 from、to、relation，from/to 是 exact revision。复杂内容字段由 generated owner schema 校验，不提供 raw RPC 输入入口。

`resource put --request-file resource.toml` 文件使用 ResourceDescriptor 字段；get/list/remove 使用 Registry。`resource materialize --request-file reference.toml` 使用真实 stable external reference。`projection build|managed --consumer <operator已配置consumer> [--request-file context.toml]` 需要选中 Session，使用该 Session 的实际 foreground WorkContext，local context selection 不覆盖 foreground；文件可包含 query、situation_refs、known_cursor，后者用于 managed delta/snapshot，`--max-items`/`--max-bytes` 明确预算。

`subject seed` 与 `subject adopt-seed --kind import --request-file seed.toml` 管理真实种子版本；`identity rebind --request-file mention.toml` 管理 mention_id/entity_ref/binding_state/reason。`subject create --request-file subject.toml` 可显式设置 capabilities 与 cognitive_seed。`session list`、`context list` 使用 `--page-size`/`--page-token`，公开 Page 上限为 200。`context create/set --request-file` 可设置 purpose/context_text/unresolved_questions/constraints/resume_conditions/budget_summary。创建的 questions/resume 是数组；set 的两个列表使用 `{ values = [...] }` 表达字段存在，空数组、空文本和空对象明确清空，未提供字段保持原值。`set/pin/unpin/clear --expected-revision` 可指定所见修订；省略时只读取当前 revision，内容修改由 Runtime 在锁内完成。

`material limits/artifacts/occurrences/representations/producer` 提供 admission 上限、Artifact 分页、实际 Observation 反查、派生表示列表与 producer 读取。occurrences 接受 Artifact，representations 可用 Artifact/SourceRegion 和 `--kind` 过滤。

列表使用 `--page-size`/`--page-token`；`memory|episode|journal list --status accepted|withdrawn`、`session list --status open|closed` 与 `resource list --status ready|degraded|unavailable` 直接使用 owner 的筛选合同，上述列表省略 `--status` 返回所有状态；`tag list --status active|withdrawn` 默认 active。`subject create --operation-id <id>` 可以保留用户指定的操作身份。

`episode create --kind <segmentation track> --request-file episode.toml` 与 `episode revise --kind resegment|reinterpret --request-file episode.toml` 共用纯 EpisodeContent 文件，track 只在创建指定；修订不再次提供操作 envelope。

`tag create --request-file tag.toml` 使用与修订/拆分相同的 TagContent 字段；创建默认 host-explicit origin。`material clean` 清理整个 BlobRoot 的无引用对象与残留 staging，不要求选中 Subject；登记中的内容和所有 Subject 引用的材料由 Material owner 保护。
