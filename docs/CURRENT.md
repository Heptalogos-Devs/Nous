# 当前工作

项目决定从头重建 Nous。旧 Python/PyO3 版本保存在 legacy/nous-python/；旧开发分支 PR #60 作为独立历史参考保留，未合入新的活动代码。

**当前唯一产品施工范围：通用基础平台。** 已确定平台必须可独立安装、调用、验证和发布，完成后暂停，不自动进入任何 Nous 认知模块。

已确定工具基线：Bun 1.4.4、TypeScript 7.0.2、Zod 4.6.7、Rust 1.100.0、napi-rs 3.x；Bun/Cargo workspace，隔离 Native 构建，统一本地检查。旧 Python/uv/PyO3 命令不再是活动入口。

当前仓库只保留领域设计及研究参考。基础平台实际实现、构建与非 Nous 消费者验证尚未完成；后续开发依据 [基础平台设计](design/FOUNDATION.md)展开。

在首次基础平台独立使用结论形成之前，不开始移植 Note/Journal、Wave 数值接线、Steward 或其他 Nous 领域。
