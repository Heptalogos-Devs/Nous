# Motivation

[返回 Rust owner 目录](../INDEX.md)

本 owner 保存 Desire、Concern、DesiredCondition 及精确条件评价，不拥有 Pursuit 或外部执行。稳定 object/revision、来源、scope、生命周期、共享反思和 evaluator 合同见 [Motivation Spec](../../docs/specs/active/motivation/motivation.md)；canonical 接口见 [motivation.proto](../../proto/nous/wave/v1alpha1/motivation.proto)。

`MotivationService` 复用 Authority 事务与 Subject Clock。`CriterionReader` 的正式实现只读取实际 Material/owner 状态；semantic 成果通过 Kernel 内部 workflow 读取已保存的 ModelInvocations 输出，公共 evaluate 不接收调用方真值或完整覆盖声明。metadata 来源边与批量正文读取支持共享 SourceRead 前置许可；selector 不混入认知支持依赖。
