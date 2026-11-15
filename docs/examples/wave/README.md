# Wave 小型示例集合

这里是用于查看接口与来源关系的人工语料，不是真实用户经历，也不是算法效果基准。输入包含三份原始说明、一份明确由前两份派生的总结和一份用来比较反证关系的人工主张。source_sequences 的顺序由说明中的概念顺序明确提供；总结可以召回，但不生成新的独立共现事实。

`collection.json` 不含模型名、维度、API Key 或假向量。build 接纳清单中的文件；指定 --space 时使用实例的共同 Embedding 能力产生向量。路径相对清单目录。

context 和 stages 是产品公开输入，不是测试 DSL。Thinking 的阶段候选只是可选池，算法仍需依据实际 transitions/closure 决定是否产生路线。

每个路径步骤以 `selections` 数组声明一个或多个原文选区；数组内的字节坐标对应随附 Markdown 文件。未配置向量空间时省略 `--space`，构建后可明确选择 `text_only` 程序：

```sh
nous --instance ./temp/nous-wave init --layout portable
nous --instance ./temp/nous-wave tools wave build --manifest ./docs/examples/wave/collection.json
nous --instance ./temp/nous-wave tools wave query "<构建返回的快照引用>" "取消后如何释放资源" --program text_only
```

配置向量空间后，build 可通过 `--space` 指定它；查询可选择实例安装的其他程序。[text-program.toml](text-program.toml) 展示词项召回、排序、局部定位和证据交付的完整程序，可将其中的配置并入实例 `nous.toml`，然后用 `--program example_text` 调用。程序定义和算子见 [Wave 程序](../../design/wave/program.md)。

自定义程序可在定位与选择之间接入 `passages.evaluate`。数值评价以 `numeric_order` 指定高分或低分优先；分类量表的 `labels` 只声明允许值，需要排序时另用 `category_order` 按优先顺序列出所有类别。未声明分类顺序时保留判断，不改变排序。评价任务和模型配置见 [模型示例](../models.toml)。
