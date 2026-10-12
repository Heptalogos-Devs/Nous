# Runtime Bundle 与运行目录

[返回当前产品合同](../../INDEX.md)

## Owner

TypeScript Core owns path resolution, instance discovery, process lifecycle and explicit runtime installation. Rust Kernel owns Authority/Serving and the private PostgreSQL cluster lifecycle.

## RuntimeLocations

Each installation resolves independent Program, Runtime, Instance, Configuration, Data, Blob, Cache, Secret, Log, Run, Temp and Backup roots. A root can reside on a different volume; paths are not inferred from the current working directory or another root.

`nous serve --home <path>` maps instance roots to `instance/`, `config/`, `data/`, `blobs/`, `cache/`, `secrets/`, `logs/`, `run/`, `temp/` and `backups/`. Program and Runtime default to the installed payload. `nous serve --locator <bootstrap.toml>` reads explicit `[paths]`; relative paths resolve against that locator. A process selects either home or locator.

Bootstrap rejects unknown or duplicate path keys and invalid path definitions. ProgramRoot contains the application payload. RuntimeRoot contains installed versioned packs. ConfigurationRoot contains the sole editable `nous.toml`; SecretRoot contains referenced credentials. DataRoot owns the private database, BlobRoot owns Artifact content, CacheRoot owns rebuildable Serving, and RunRoot owns protected process discovery.

## Configuration 与 lifecycle

Core 将数据库、对象与 Serving 路径及 typed ConfigurationBootstrapBundle 一次写入受保护的 `RunRoot/kernel-bootstrap.json`，Kernel 从同一文件打开实例。该私有启动文件不是用户配置入口；关闭实例时删除。Configuration Service 仍是运行配置快照的权威。

The portable application package contains Core, CLI/official Client, Kernel, generated protocol, Prompts, migrations and runtime manifests. User configuration, credentials, instance data and locators are created outside ProgramRoot.

`nous init` and first `nous serve` create a minimal configuration with exclusive create; existing content is not overwritten. `nous config check` uses the same parser and validates local references without starting Runtime or calling a provider. Full fields are documented in [Configuration](../../../reference/CONFIGURATION.md).

Managed-private database mode uses the installed PostgreSQL pack, a protected instance credential and the instance's fixed loopback port. External mode connects only to the configured endpoint. Cluster/version mismatch and port conflicts fail explicitly; startup does not upgrade, recreate or discard database state.

The PRE_PRODUCTION database foundation is the current `0001_foundation.sql` through `0004_indexes.sql`. They declare the current owner shapes directly, including WorkContext text/anchors, Formation basis and model execution metadata. Superseded incremental schemas and payload decoders are removed. Existing cognition is preserved before a deliberate fresh-database restore; ordinary startup does not rewrite an older database's migration ledger.

Core 生命周期由 launcher 直接启动；源码开发与 portable 使用同一启动过程。数据库、Kernel、Core 和 discovery 可用后发布 READY。公开启动输出不包含 bearer token；token 仅保存在受保护的 RunRoot。实例支持 SIGINT、SIGTERM 和显式 `--stop-on-stdin-close`；Kernel 意外退出使 Core 停止并报告失败。关闭过程逐项释放资源，单项失败不跳过其他资源。

`nous status` 从 Core 的一次快照取得 Core、Kernel 与模型角色状态。Kernel transport 不可用时仍报告可用的 host 状态与实际错误；模型 READY 表示本地配置可执行，不代表 provider 调用成功或 Subject 的 Serving 已准备。Subject projection 状态单独读取。

普通 serve 直接使用已安装 runtime，不下载，也不重复扫描整个 inventory。安装及显式 verify 校验完整 inventory；list 只显示安装元数据和 INSTALLED、MISSING 或 INVALID 状态，不声称完整性校验已通过。Runtime 命令为：

```text
nous runtime list
nous runtime verify <component>
nous runtime install <component> [--pack <archive>]
```

Installation accepts a local archive or a manifest URL, checks platform, archive hash, inventory and executable closure, extracts into a private staging directory, then publishes the validated pack atomically. Existing installations are not silently upgraded.

FFmpeg resolves from an explicitly configured executable or the installed FFmpeg pack. Frames processing uses argument-array execution, bounded temporary files, source duration, frame/audio byte limits, process timeout and cancellation. FFmpeg subprocesses do not inherit configured credential variables.

## Windows x64 portable assembly

The current release pipeline builds the shipping Kernel for `x86_64-pc-windows-gnullvm` with LLVM-MinGW UCRT and includes its private `libc++.dll` and `libunwind.dll` dependencies. The source-less portable ZIP contains compiled Core/CLI/Client, Kernel, Node, PostgreSQL, FFmpeg, Prompts, migrations, manifests, SPDX SBOM and license/source notices.

Assembly writes `data/releases/windows-x64/current/` and `current.zip` through sibling staging paths. A normal assembly replaces current; `release:archive` explicitly saves an immutable release artifact. Build and operation commands are documented in [scripts/README.md](../../../../scripts/README.md).
