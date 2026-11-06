# Nous Native

`vcp` 拥有 VCP d5c019e 的纯数值内核、来源独立性及具名 DTSC；`retrieval` 拥有 Tantivy/Jieba 与 USearch 索引；Windows 的 Tantivy Directory 复用标准库原子替换，允许旧 metadata 读者持有已打开的文件；`python` 提供 `nous_native` 的 PyO3/NumPy 边界。Python WaveOperations 负责内容、模型、来源引用与快照发布。

从仓库根目录设置 `scripts/dev-env` 后执行 `uv sync --locked`。maturin 随本地依赖构建 CPython 3.14 扩展；Rust 和 C/C++ 构建产物、下载位于 `temp/`。

接口见 [类型声明](../src/nous_native/_core.pyi)。四个只读输入 bundle 将数组拷入 Rust 自有存储；快照与查询共用 Engine 的有界线程池。build 写入调用者的 staging 目录，open 核对身份、组件与索引；close 等待实际计算结束。observe、readout 和 think 共享一次观测，普通消费者使用 search。

构建阶段导出的 [schema](../src/nous_native/resources/schemas.json) 和 [默认值](../src/nous_native/resources/defaults.json) 与执行类型同源；配置目录可读取这些静态资源而无需加载扩展。普通结果保持排名顺序，quality_score 与 ranking_score 分别表达质量与实际排名键。

关键数值合同位于 `vcp/tests`，可运行 `cargo test --manifest-path native/vcp/Cargo.toml`。来源与许可见 [VCP](vcp/UPSTREAM.md) 和 [索引](retrieval/UPSTREAM.md)。
