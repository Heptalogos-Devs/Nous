# 工具配置

[返回目录](../INDEX.md)

格式、TypeScript lint、依赖审查、重复检测和许可证审查配置集中在这里，由 `package.json` / `justfile` 显式选择。Buf module 配置随 canonical Proto 放在 `proto/buf.yaml`；Clippy 配置由 `.cargo/config.toml` 指向 `.cargo/clippy.toml`。

根目录保留 Cargo/pnpm workspace、lockfiles、TypeScript 项目入口、toolchain 和 Git/editor 自动发现文件。`dupes.toml` 保留在根目录，因为该工具没有配置路径参数。

`typos --config .config/typos.toml` 可运行按需拼写审查。产品配置示例在 [docs/reference/examples](../docs/reference/examples/README.md)，开发实例配置在 ignored `data/config/apps/`。

`scripts/` 保存仓库脚本的配置。`scripts/doc-navigation.toml` 配置文档导航检查的忽略目录及返回/覆盖豁免页面，调用方法见 [脚本 README](../scripts/README.md)。

`scripts/code-length.toml` 是手写 TypeScript/Rust 物理行数门限及生成路径排除的唯一配置。`pnpm check:length` 检查 Git 管理与未忽略的新文件，包括生产源码、测试文件、测试 helper、内联 Rust tests 和仓库脚本；测试目录没有豁免。TypeScript 达到 600 行、Rust 达到 500 行警告，分别达到 900/800 行拒绝。Linux/Windows 的 `check:fast` 共用此门禁；入口和边值说明见 [开发检查](../scripts/dev/README.md)。
