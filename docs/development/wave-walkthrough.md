# Wave 实际使用通路

## 准备

在本次明确的开发实例上使用已配置真实文本 Embedding 空间。依赖、环境和产物继续按项目 temp 规则放置。现有 New API 网关可直接使用；不要为了算法实验重建网关或模型 SDK。Native 构建按本项目稳定工具链执行，具体版本锁入正式配置。

本目录的例子是公开的人工小语料，帮助查看来源/概念关系，不能作为自然语言相关性效果结论。用户真实资料才是后续质量判断的主要输入。

## 建立集合与查询

```sh
nous --instance ./temp/nous-wave tools wave build --manifest docs/examples/wave/collection.json --space <已配置空间>
nous --instance ./temp/nous-wave tools wave inspect <build返回的wav地址>
nous --instance ./temp/nous-wave tools wave query <wav地址> "取消了任务，为什么文件仍在使用" --mode dense
nous --instance ./temp/nous-wave tools wave query <wav地址> "取消了任务，为什么文件仍在使用" --mode rivermemo --explain
nous --instance ./temp/nous-wave tools wave query <wav地址> "此前相关的资源所有权经验" --context docs/examples/wave/context.json --core resource-ownership --mode hybrid --explain
nous --instance ./temp/nous-wave tools wave query <wav地址> "先定位资源再解决问题" --mode rivermemo --thinking-stages docs/examples/wave/stages.json --explain
nous --instance ./temp/nous-wave tools wave read <wav地址> cancellation --sources
```

按实际返回值替换参数；命令中的 `<...>` 是说明占位，不是可直接粘贴的合法地址。库未配 embedding 时，先用现有 embedding probe/run 完成选择；不要在产品默认值写入某台开发机模型名。

## 需要观察的结果

同一候选集运行 lexical、dense、tagmemo、rivermemo、hybrid、dtsc，展开实际 used lanes 和分数类型。Thinking 未形成合法完整路径时应解释原因，不能伪造关系；在具备路径的资料上实际展示前驱与候选闭合。

分别提供/去掉 context、core、unknown cue，观察数值输入和候选变化。长描述 Concept 的向量应来自完整描述。重复导入派生总结不会增加原始事实支持；反证关系独立展示。

关闭并重开后 query/read 使用保存快照与相同正式来源；文稿向量和 Concept 向量不重新生成。纯 lexical 与读取不依赖新模型请求。

## 同时检查已有通路

正式 preview 不发转写，probe 不伪造配置；多阶段媒体保持同一模板快照。长 PDF/视频在有界批次完成后释放实际 bytes。处理中取消后，当前 worker/子进程退出再释放其输入文件；已采用结果仍能读回。

给定时间域查询不拿真实墙钟冒充虚拟现在。特定 evidence scope 的快照不能消费范围外来源，即使最终候选隐藏了它。坏空间/矩阵映射在边界返回明确错误。

## 实施者与用户验证

实现者选择能暴露当前问题的真实命令，修正已发生故障后返回功能。只为独立高价值的数值/事务/取消合同保留必要回归，不重复测试依赖内部、不新增评测 DSL 或 CI。用户继续进行独立全面验证和压力检查。

Native stage timings、输入规模、实际结果用于分析瓶颈；不要求每次提交跑全量实验，也不从一次更快就推导所有场景更优。临时样本生成与测量脚本使用 temp，正式 CLI/API 是长期使用入口。
