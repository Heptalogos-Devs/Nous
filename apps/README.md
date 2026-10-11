# 应用

[返回目录](../INDEX.md)

Nous Wave 由公共 TypeScript Core 与私有 Rust Kernel 组成，CLI 是通过官方 Client 连接 Core 的第一方使用端。开发启动命令为根目录的 `corepack pnpm dev`。

- [Core](nous-core/README.md)：公共 API、模型/资源调用与进程编排。
- [Kernel](nous-kernel/README.md)：Rust owners 的组合与私有进程。
- [CLI](nous-cli/README.md)：上传、查询与实例操作。

进程和 owner 关系见 [当前实现架构](../docs/architecture/current-implementation.md)。
