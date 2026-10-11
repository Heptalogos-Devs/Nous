# Runtime Bundle 与运行目录

[返回当前产品合同](../../INDEX.md)

## Owner

TypeScript Core owns path resolution, instance discovery, process lifecycle and explicit runtime installation. Rust Kernel owns Authority/Serving and the private PostgreSQL cluster lifecycle.

## RuntimeLocations

Each installation resolves independent Program, Runtime, Instance, Configuration, Data, Blob, Cache, Secret, Log, Run, Temp and Backup roots. A root can reside on a different volume; paths are not inferred from the current working directory or another root.

`nous serve --home <path>` maps instance roots to `instance/`, `config/`, `data/`, `blobs/`, `cache/`, `secrets/`, `logs/`, `run/`, `temp/` and `backups/`. Program and Runtime default to the installed payload. `nous serve --locator <bootstrap.toml>` reads explicit `[paths]`; relative paths resolve against that locator. A process selects either home or locator.

Bootstrap rejects unknown or duplicate path keys and invalid path definitions. ProgramRoot contains the application payload. RuntimeRoot contains installed versioned packs. ConfigurationRoot contains the sole editable `nous.toml`; SecretRoot contains referenced credentials. DataRoot owns the private database, BlobRoot owns Artifact content, CacheRoot owns rebuildable Serving, and RunRoot owns protected process discovery.

## Configuration 与 lifecycle

The portable application package contains Core, CLI/official Client, Kernel, generated protocol, Prompts, migrations and runtime manifests. User configuration, credentials, instance data and locators are created outside ProgramRoot.

Configuration must declare the current `config_revision`. `nous init` and first `nous serve` create a minimal configuration with exclusive create; existing content is not overwritten. `nous config check` uses the same parser and validates local references without starting Runtime or calling a provider. Full fields are documented in [Configuration](../../../reference/CONFIGURATION.md).

Managed-private database mode uses the installed PostgreSQL pack, a protected instance credential and the instance's fixed loopback port. External mode connects only to the configured endpoint. Cluster/version mismatch and port conflicts fail explicitly; startup does not upgrade, recreate or discard database state.

The PRE_PRODUCTION database foundation is the current `0001_foundation.sql` through `0004_indexes.sql`. They declare the current owner shapes directly, including WorkContext text/anchors, Formation basis and model execution metadata. Superseded incremental schemas and payload decoders are removed. Existing cognition is preserved before a deliberate fresh-database restore; ordinary startup does not rewrite an older database's migration ledger.

READY is published after database, Kernel, Core and discovery are available. Public startup output is redacted; bearer tokens are stored only in protected RunRoot. Graceful shutdown is available through stdin close, SIGINT and SIGTERM.

Ordinary serve uses installed runtime packs and does not acquire them from the network. Runtime commands are:

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
