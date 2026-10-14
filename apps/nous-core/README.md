# Nous Core

[仓库地图](../../INDEX.md) · [Applications](../README.md) · [开发脚本](../../scripts/dev/README.md)

Nous Core is the TypeScript host process. It loads local configuration, starts the Rust Kernel, exposes the Core Connect/HTTP surface, and composes WorkContext, Projection, Context, NousQL, and model operations.

- [Current implementation architecture](../../docs/architecture/current-implementation.md)
- [NousQL reference](../../docs/reference/NOUSQL.md)
- [Package manifest](package.json)

The Core does not own canonical PostgreSQL state or the internal semantics of Rust domain owners.

实现按能力归属组织。[configuration](src/configuration/catalog.ts) 规范化并托管配置；[模型绑定](src/model/execution/binding.ts) 在启动时解析可执行路由，[routes](src/model/execution/routes.ts) 直接执行这些绑定并记录 fallback、取消和 usage，不重建配置。[结果操作](src/operation-results.ts) 读取已有结果、保存实际提案并记录 owner 提交结果；没有成果时不登记恢复 snapshot。Resource、Query、NousQL、Cognition 与 Maintenance 保持自己的能力目录；测试和维护 fixtures 位于 `tests/`。

Use `corepack pnpm dev` from the repository root for the platform-neutral development path. Public smoke uses normal Core startup and the official Client.

For an external foreground launcher, `nous serve --locator <bootstrap.toml> --stop-on-stdin-close` requests graceful shutdown when the launcher's stdin pipe closes. Interactive deployments can continue using SIGINT/SIGTERM. `ConfigurationRoot/nous.toml` owns user settings and `RunRoot/core.json` supplies official Client discovery. Core model clients use explicit gateway/profile/role bindings and local Prompt assets. Core resolves upstream credentials through SecretRoot/environment and keeps them outside Kernel process settings.
