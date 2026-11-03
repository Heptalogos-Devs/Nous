# 文本 Embedding 与空间绑定

## 本轮能力

通过已配置统一网关执行真实文本 Embedding，保存向量、实际输入、用途、空间和 producer，支持读取与重启后继续使用。使用 Pydantic AI Embedder/OpenAIEmbeddingModel 或官方 SDK 的同一连接能力；不实现 Wave 索引、ANN、跨空间搜索或多模态联合向量。

CLI `embedding probe` 用实际样例确认维度与返回格式，但不登记正式空间；`embedding run` 使用已明确的空间并保存结果。只读配置不隐式调用网络。

## 空间语义

SpaceDefinition 包含 space_id、解析后的模型/部署版本、维度、模态、encoding、query/document 前缀/处理规则和 normalization。配置与表使用同一个纯值合同。

模型资源和空间都给出 deployment_revision 时必须一致；空间给出的明确声明可用于模型资源尚未指定版本的情况。实际网关报告的 model 字段另存，不由本地猜权重版本。

provider/resource ID 是配置定位，不能单独判断空间是否变化。绑定比较实际模型名、声明部署版本和编码合同；连接 endpoint/密钥的轮换不自动形成新语义空间，操作者负责其仍指向声明部署。模型别名背后的不可见变化标记未核实，不建立虚假数学证明。

## 执行与提交

调用前读取空间记录并核对。先在事务外执行模型，确认批量输出与输入逐项对应、维度正确、有限数值，必要时按空间规则做 l2 归一化。原始向量与后处理是否保存由结果规格明确，不能在已绑定空间中改变含义。

SDK 响应仍需经过模型 owner 的结构校验：索引为整数，向量为有限数值数组；畸形响应以 `embedding.invalid_response` 返回，不进入结果采用。没有返回用量时保留未知，不以零补足。

将 f32 向量数组用 NumPy `.npy`（allow_pickle=False）保存为输出 Artifact。记录真实维度、输入项顺序及 `purpose=query|document`；查询与文档使用空间配置中各自的前缀/规则。

采用前先发布向量等输出字节。短事务内再次核对当时正式空间：首次使用 `bind_in(tx)` 登记完整合同，调用 `ResultStore.adopt_in(tx, ...)` 并保存向量关联；已有同 ID 同合同复用。任何一步冲突全部回滚，不能用返回长度相同作为兼容依据，也不能另开结果事务逃出回滚。数据库失败不再调用模型。

l2 归一化遇到零范数应明确报出无效向量；不填零向量、NaN 或错误维度后宣称成功。

## 配置接入

替换首轮 Configuration 内的全量拒绝分支，按 [配置应用](../foundation/configuration-integration.md) 由 Embedding owner 核对所有入口。新、未绑定配置允许创建和编辑；已有资产绑定的空间保持语义不变，改模型或维度创建新 ID。删除还被资产引用的空间配置时保留正式描述供读取，并明确当前任务引用需要调整。

本文本轮没有旧向量迁移或零停机双写。改变任一新空间不需要先重建尚不存在的 Wave。

## 返回

默认显示空间名、用途、实际模型、输入数、维度、用量和结果地址，不打印数千坐标。`result read`/内容导出可取得具体向量文件。字段未报告就保持未知。

依据：[Pydantic AI Embeddings](https://pydantic.dev/docs/ai/guides/embeddings/)。
