# 向量执行、精确输入与空间绑定

## 空间责任

Embedding owner 管理 SpaceDefinition、实际编码、输出校验、持久向量与输入行映射。沿用现有模型名/部署版本、维度、模态、query/document 输入规则、预处理和归一化语义。同维度不能代替同空间。

空间配置和正式绑定使用同一纯值合同。未用空间可编辑；首次正式向量采用时登记，之后语义变化使用新 ID。配置删除不删掉已保存空间说明。模型别名背后的不可见部署变化保持未核实，保存实际返回模型名。

## 共同批量能力

```text
encode_batches(ordered actual_texts, resource/space, purpose, context)
    -> async iterator[EncodedBatch(original_indices, matrix, usage)]
ensure(ordered ContentSelection[], space_id, purpose, context) -> EmbeddingBatchRef
run(inputs, space_id, purpose, context) -> EmbeddingOutcome
encode_query(texts_or_content, space_id, context) -> MatrixWithRows
read(batch_ref) -> MatrixWithRows
probe(model_selection, input, context) -> ProbeOutcome
```

run 和 probe 使用 `TextInput` 表达文件、文字或 Artifact 及其文本选区；ensure 接受已接纳的精确 ContentSelection。probe 临时读取选区，run 接纳正式原件后读取相同范围。返回 batch_ref、每行输入引用、row_index、space definition、purpose 和向量 Artifact。Wave 通过该能力取得矩阵，不直接查询 embedding 私有表、读取 Provider SDK 或按结果数量猜顺序。

exact 复用键包含实际编码文字/媒体摘要、空间完整签名、purpose 和已采用输出。相同模型重复调用可能产生不同数值；复用已采用向量使快照可重开。不使用模糊文本相似缓存，不凭内容近似代替精确向量身份。

## 执行与校验

query/document 各自使用空间的实际前缀与预处理。长数组按协议和配置限制批量提交，保留每项映射，不能用单条错误阻断其它已完成可读取的结果。上层需要一组完整矩阵时只在所有所需行完成后发布它。

后续批次失败或取消时，错误中的 `completed_result_refs` 指向已采用的向量结果；重新调用 `ensure` 会精确复用这些行。

保留 fdb39e4 的 SDK 响应校验：index 必须是整数且精确覆盖输入；向量维数正确、数值有限；f32 转换后仍有限。l2 使用稳定范数，零范数失败。usage 未提供保持未知。重试共享模型请求策略，不另写协议客户端。

## 存储与 Native

实际 f32 矩阵采用 `.npy`，禁止 pickle；空间、purpose 和行引用一起采用。空间绑定、ResultStore.adopt_in 和行关系在同一 WriteTx 完成。模型成功后保存失败不重新请求。

Native 的几何计算需要单位向量时，在其计算资产内规范化并记录原空间；不覆盖正式向量。多个空间不能直接混合，一个 WaveSnapshot 选择一个空间。空间扩展/替换重建相应快照，无旧内部迁移承诺。

## 所有入口共用请求规划

encode_batches 属于 EmbeddingExecutor，拥有 actual input encoding、每项/每请求限制、max_batch_size、实际调用与输出下标校验。run、ensure、probe、encode_query 直接消费这一能力，不各自提交所有输入或复制批次循环。5 条输入且 max_batch_size=2 时所有入口都按 2+2+1 发出。

EmbeddingOutcome 保存有序行引用、成功批次和 coverage。run 不再要求所有向量必须来自一个 ProcessingResult；按批采用后组装一个结果视图，CLI 同步迁移，旧返回形状不做兼容。EmbeddingBatchRef 的 rows 严格对应原输入顺序，包括重复项。

文档输入采用正式 Artifact；临时查询的文字可由 encode_query 直接编码并返回内存矩阵，默认不为每次查询接纳新 Artifact/ProcessingResult。已有 ContentSelection 查询引用继续保留。两者共用同一编码、批次和请求策略，不形成两套算法。需要保存实验查询时由调用者明确选择采用。

## 并发

删除实例级 `_ensure_lock`。先查精确缓存，命中立即返回；不同键的缺失工作交给现有 Execution 并发管理。

本轮采用最简单的并发规则：每个根操作拥有自己的请求与取消；同一次调用先按精确输入摘要去重，跨调用复用已经采用的缓存。两个并发请求同时未命中时允许各自执行，采用时由唯一键和事务选择规范行；不让后来的结果覆盖已经提交的规范向量。

不新增共享在途任务、按键长锁、跨键锁链或全局队列。这样避免某个等待者取消、较短 deadline 或根操作并发名额影响另一请求。以后真实调用量确有需要时，再在共同请求 owner 中增加有明确生命周期的同键合并；它不是本轮完成条件。

空间、用途、输入摘要的唯一性在向量 owner 数据库中定义。网络在事务外；采用时复查绑定和缓存，所有调用者拿到相同的已采用规范行。

## 按需读取

按向量 Artifact 分组取得 CAS 本地借用，使用 `numpy.load(path, mmap_mode="r", allow_pickle=False)` 或同等现有能力取需要的行。先验证 dtype/维度/行号，避免 bytes+解码+矩阵的多份整文件拷贝。借用期间映射有效；返回自有连续目标矩阵后关闭映射和借用，尤其保持 Windows 可关闭/发布语义。

缺失的每项输入上限从配置/模型合同取得，单项过大报告实际 index，不盲发整个大批再递归二分重试。成功批次用量保留，未知 token 不填零。
