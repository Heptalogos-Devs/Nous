# 配置示例

[返回文档目录](../../INDEX.md)

- [nous.toml](nous.toml)：可编辑项和模型角色的完整说明。
- [bootstrap.toml](bootstrap.toml)：独立 logical roots 的 locator 示例。
- [gateway.env.example](gateway.env.example)：仅含空值的 credential variable 示例；使用时保存到实例 SecretRoot。

这些文件供阅读和手工配置，不进入 portable payload。`nous init --home <instance>` 或首次 `nous serve --home <instance>` 在该实例 ConfigurationRoot 创建最小 `nous.toml`。已有文件保持原样；模型与外部 Resource 需要用户自行配置。

最小文件由包内 Core 代码生成，包含当前 `config_revision` 和 default consumer。生成器与启动/检查命令使用同一配置解析合同；完整示例由同一解析器验证。示例不是启动模板，更新示例不会覆盖你的配置。

修改后运行 `nous config check --home <instance>` 或 `--locator <bootstrap.toml>`；源码开发使用 `pnpm nous config check`。检查只读取本地配置和明确引用的文件，不连接数据库或模型。
