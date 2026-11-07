# Native 计算

本目录是 Rust cargo workspace，`vcp` 保存来源可追溯的纯数值内核，`retrieval` 组织 Tantivy、USearch、查询快照与分组召回，`python` 是 `nous_native._core` 的薄 PyO3 绑定。接口、生命周期和数据形状以 [Rust Native 计算接口与生命周期](../docs/design/wave/native.md) 为准，检索核心见 [检索核心](../docs/design/wave/engine.md)。

## 边界

- Native 只管理 Python 传入的 index/staging 目录，不知道业务数据库地址、CAS 根或内容目录，不访问网络。
- 输入是冻结数组和可重建资产；不借用调用方仍可写的 NumPy buffer，热循环不调用 Python。
- 算法参数、默认值和 schema 由 Rust 类型导出，Python 经 `WaveConfig` 接入；不维护另一套手写默认值。
- 计算使用实例级有界 Rayon 池。取消令牌在迭代、批次和索引阶段检查，取消后等待实际工作退出再释放快照。
- 错误用带 code 的 `Failure`/`NativeError` 表达，不靠匹配错误文本分类，也不返回 `success=true` 的空实现。

## 构建与修改

- 从仓库根执行 `uv sync --locked` 经 maturin 重建扩展；先 `source scripts/workspace.sh`（Windows 为 `. ./scripts/workspace.ps1`），让 cargo 目录落在 `temp/`。
- 修改 `vcp` 或 `retrieval` 后在 `native/` 用 `cargo test -p nous-vcp` 或 `cargo test -p nous-retrieval` 运行对应测试。这两个 crate 启用 workspace lints，禁止 `unsafe`。`python` crate 的改动通过实际的 `nous tools wave` 调用验证。
- 接口或行为改变时更新上述设计专题；源码位置或职责变化时更新 [INDEX.md](../src/nous/INDEX.md)。
