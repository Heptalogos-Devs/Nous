# Wave 小型示例集合

这里是用于查看接口与来源关系的人工语料，不是真实用户经历，也不是算法效果基准。输入包含三份原始说明、一份明确由前两份派生的总结和一份用来比较反证关系的人工主张。source_sequences 的顺序由说明中的概念顺序明确提供；总结可以召回，但不生成新的独立共现事实。

`collection.json` 不含模型名、维度、API Key 或假向量。build 的 --space 指向实例实际空间；命令接纳所有文件并通过共同 Embedding 能力产生向量。路径相对清单目录。

context 和 stages 是产品公开输入，不是测试 DSL。Thinking 的阶段候选只是可选池，算法仍需依据实际 transitions/closure 决定是否产生路线。
