# Nous Wave

PRE_PRODUCTION; no released external consumers. Internal replacements update current producers/consumers together and delete superseded shapes. Architecture-Vault owns long-term cognition semantics and accepted decisions. Executable contracts are routed through [docs/INDEX.md](docs/INDEX.md). For behavior changes, read the relevant Vault target design and accepted decisions, owning current Spec, direct dependency contracts, and current code.

Use project-specific engineering guidance in [.agents/skills/AGENTS.md](.agents/skills/AGENTS.md); the installed agency-execution skill owns general execution behavior.

TypeScript Core owns public hosting and model/resource execution. Rust semantic owners hold Authority, mutation and lifecycle meaning. `persistence` supplies database mechanics; `retrieval` supplies rebuildable Serving. `runtime` is independent of concrete retrieval/provider implementations.

Canonical wire contracts live in `proto/`; regenerate bindings with `corepack pnpm generate`. Fresh database schema is `crates/persistence/migrations/0001_foundation.sql` through `0005_workflow_envelope.sql`; the final migration also preserves saved operations during the envelope transition.

Development configuration is ignored `data/config/apps/nous.toml`; secrets use SecretRoot/environment. Portable payloads contain no operator configuration. Repository-generated local artifacts live under `data/`, except standard `node_modules/`, Cargo `target/`, and third-party CodeGraph storage. Runtime acquisition is explicit; ordinary serve uses installed packs.

Default branches are integration-only: PR and squash merge, never direct push.
