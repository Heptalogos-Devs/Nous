# Nous Core

[仓库地图](../../INDEX.md) · [Applications](../README.md) · [开发脚本](../../scripts/dev/README.md)

Nous Core is the TypeScript host process. It loads local configuration, starts the Rust Kernel, exposes the Core Connect/HTTP surface, and composes WorkContext, Projection, Context, NousQL, and model operations.

- [Current implementation architecture](../../docs/architecture/current-implementation.md)
- [NousQL reference](../../docs/reference/NOUSQL.md)
- [Package manifest](package.json)

The Core does not own canonical PostgreSQL state or the internal semantics of Rust domain owners.

Implementation is grouped by owner. [configuration](src/configuration/catalog.ts) normalizes and hosts the current configuration graph; [model execution snapshots](src/model/execution/snapshot.ts) resolve bindings for both startup and frozen recovery, and [route execution](src/model/execution/routes.ts) owns fallback, cancellation, attempts and usage. Model invocations compose those owners with provider operations. Resource, Query, NousQL, Cognition and Maintenance retain their own directories. Tests live in `tests/` under the affected feature; shared maintenance fixtures stay there, outside production.

Use `corepack pnpm dev` from the repository root for the platform-neutral development path. Public smoke uses normal Core startup and the official Client.

For an external foreground launcher, `nous serve --locator <bootstrap.toml> --stop-on-stdin-close` requests graceful shutdown when the launcher's stdin pipe closes. Interactive deployments can continue using SIGINT/SIGTERM. `ConfigurationRoot/nous.toml` owns user settings and `RunRoot/core.json` supplies official Client discovery. Core model clients use explicit gateway/profile/role bindings and local Prompt assets. Core resolves upstream credentials through SecretRoot/environment and keeps them outside Kernel process settings.
