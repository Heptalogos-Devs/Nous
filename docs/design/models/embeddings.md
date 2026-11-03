# 向量执行、精确输入与空间绑定

## 空间责任

Embedding owner 管理 SpaceDefinition、实际编码、输出校验、持久向量与输入行映射。沿用现有模型名/部署版本、维度、模态、query/document 输入规则、预处理和归一化语义。同维度不能代替同空间。

空间配置和正式绑定使用同一纯值合同。未用空间可编辑；首次正式向量采用时登记，之后语义变化使用新 ID。配置删除不删掉已保存空间说明。模型别名背后的不可见部署变化保持未核实，保存实际返回模型名。

## 共同批量能力

```text
ensure(ordered EmbeddingInput[], space_id, purpose,
       reuse=exact, context) -> EmbeddingBatchRef
read(batch_ref) -> MatrixWithRows
probe(model_selection, input) -> ProbeOutcome
```

EmbeddingInput 是精确 ContentSelection 或已接纳的文本 Artifact。返回 batch_ref、每行输入引用、row_index、space definition、purpose 和向量 Artifact。Wave 通过该能力取得矩阵，不直接查询 embedding 私有表、读取 Provider SDK 或按结果数量猜顺序。

exact 复用键包含实际编码文字/媒体摘要、空间完整签名、purpose 和已采用输出。相同模型重复调用可能产生不同数值；复用已采用向量使快照可重开。不使用模糊文本相似缓存，不凭内容近似代替精确向量身份。

## 执行与校验

query/document 各自使用空间的实际前缀与预处理。长数组按协议和配置限制批量提交，保留每项映射，不能用单条错误阻断其它已完成可读取的结果。上层需要一组完整矩阵时只在所有所需行完成后发布它。

后续批次失败或取消时，错误中的 `completed_result_refs` 指向已采用的向量结果；重新调用 `ensure` 会精确复用这些行。

保留 fdb39e4 的 SDK 响应校验：index 必须是整数且精确覆盖输入；向量维数正确、数值有限；f32 转换后仍有限。l2 使用稳定范数，零范数失败。usage 未提供保持未知。重试共享模型请求策略，不另写协议客户端。

## 存储与 Native

实际 f32 矩阵采用 `.npy`，禁止 pickle；空间、purpose 和行引用一起采用。空间绑定、ResultStore.adopt_in 和行关系在同一 WriteTx 完成。模型成功后保存失败不重新请求。

Native 的几何计算需要单位向量时，在其计算资产内规范化并记录原空间；不覆盖正式向量。多个空间不能直接混合，一个 WaveSnapshot 选择一个空间。空间扩展/替换重建相应快照，无旧内部迁移承诺。
