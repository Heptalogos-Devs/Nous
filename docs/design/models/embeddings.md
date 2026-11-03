# Embedding 空间与危险配置

## 目标

Embedding 是持久检索资产的基础配置。首次选型需要明确输入、模型、向量空间及之后更换的成本。配置修改必须保护已有资产的解释，不靠隐藏高级选项解决。

首轮只提供纯配置定义、校验和风险说明；真实 embedding、空间登记与资产写入在模型计划中共同接通。Wave 的索引构建和已有数据迁移不在首轮基础施工。

## 空间描述

`EmbeddingSpaceDefinition` 包含稳定 space_id、实际模型/部署版本、维度、允许模态、输入组合方式、文本和媒体预处理、query/document 输入规则以及向量归一化规则。

同一模型支持文档与查询不同前缀时，将两种配对规则共同放进空间定义。多模态联合向量保持输入 parts 顺序；若只提供文本 embedding，不能把它声称为图文联合向量。

`IndexProfile` 独立描述距离函数、索引算法和搜索参数。搜索参数如候选量可以按查询变化；距离函数或索引结构变化一般要求重建索引。改变模型、维度或编码规则则需要重新生成相应向量。

## 配置形状

`embedding.spaces.<id>` 至少具有 model（已配置模型资源）、deployment_revision、dimensions（正整数）、modalities、normalization（none 或 l2）以及 encoding 对象。encoding 明确 document_instruction、query_instruction、preprocessing_revision 和 input_combination（text 或 ordered_multimodal）。空 instruction 可以明确保存为空字符串；不能因缺字段而采用来源不明的前缀。

这些字段由同一个 EmbeddingSpace 配置模型定义。首轮完成纯形状校验；模型计划接入空间登记、实际输出检查和已使用绑定，Wave 随后消费相同定义。

## 首次配置

操作者为新空间提供明确模型、维度及上述编码约定。`embedding inspect-config` 展示会影响已有资产的字段。`embedding probe` 只有在明确发起时联网，确认实际返回维度、数值形状和用量。

probe 只能检查它实际观察到的内容。Provider 模型别名可能被更换；要保持可复现空间，使用固定模型版本或操作者明确维护的 deployment_revision。相同长度的向量不足以识别语义空间。

没有维度时，不在后台猜值并发布正式空间。可以先显式 probe，再将结果作为已确认配置保存。配置本身不触发付费探测。

## 绑定与使用

空间描述在数据库由 Embedding owner 保存。首次正式资产采用前，将规范化空间描述和配置中的拟使用描述逐字段比较；采用成功后，该 space_id 的语义字段不可原地改变。

向量资产保存 space_id、精确输入/目标引用、实际生成出处和向量值。读取、搜索和 query vector 均检查使用的空间。内容或模型显示名不是空间身份。

`set`、环境覆盖、手改 TOML、重新启动都要经过同一绑定检查。发现相同 space_id 的语义变化，返回 `embedding.space_conflict` 并列出影响字段；不提供 --force 继续使用旧资产。

endpoint/credential 轮换只有在仍指向明确同一模型部署与编码规则时保留空间；改变实际部署就使用新空间。

## 已使用空间的更换

新建 space_id，重新生成所需向量，建立新索引，明确切换使用者。开发阶段默认允许暂停相关语义检索完成重建，不要求零停机双写。原文和认知正文保持自己的身份；已有词项/精确读取继续由相应能力提供。

迁移实现由 Wave/索引专题在真实资产出现时补充。当前普通配置修改不能伪装为迁移已经完成；不为尚无资产的原型先建蓝绿发布平台。

## 变化分类

| 变化 | 处理 |
|---|---|
| 相同资源的凭据、网络超时 | 普通资源配置变更 |
| 模型权重/部署版本、维度、输入预处理、query/document 编码规则 | 新空间，重做向量 |
| 已存向量的归一化含义 | 新空间或明确的可重复变换，由资产 owner 决定 |
| 距离函数、量化、索引结构 | 索引重建；是否重算向量取决于原始向量是否保留 |
| 查询候选数、搜索 effort | 普通查询配置，不重做数据 |

## 失败和验证

维度不匹配、非有限数值和错误批量对应关系在采用前失败。Embedding fallback 只允许同一明确空间合同。取消后不提交半份宣称完整的批次；部分结果按显式逐项合同保存。

验证首个真实 embedding、保存后重启、同 space_id 改模型/维度被拒绝、新 space_id 可创建，以及 query/document 规则使用一致。无需此时比较所有向量数据库。

依据：[Pydantic AI embeddings](https://pydantic.dev/docs/ai/guides/embeddings/)、[Qdrant 模型更换说明](https://qdrant.tech/documentation/tutorials-operations/embedding-model-migration/)。后者用于确认重生成资产的责任，不据此引入 Qdrant 或零停机双写。
