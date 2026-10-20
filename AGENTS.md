# Nous Wave

PRE_PRODUCTION rebuild. Read the construction package at `../Specs/README.md`, its constitution, architecture, contracts and construction order before changing behavior. Read the current feature Spec. Track Objective, Decisions and Current Work in `docs/construction.md`.

Use the installed antirlhf-exec and refactoring skills. Replace internal shapes and all current producers/consumers together. Delete superseded code, tests and documentation. Git preserves prior implementations. Use mature dependencies for generic execution, storage, parsing, transport and UI mechanics.

Nous is one TypeScript modular service. Modules under `packages/nous/src/` own their semantics and transactions. PostgreSQL is authoritative; LanceDB is rebuildable. Public Zod contracts live in `packages/contracts/`; generate OpenAPI and Client through `pnpm generate`. DBOS owns workflow progress and Kysely datasource owns atomic business/checkpoint transactions. Do not create a second queue, outbox, lease or workflow engine.

Hept is an independent product and database. Share only pure common values. Use the formal Client across products. Model identity and permissions come from the trusted host, never model input. Preserve native OpenClaw execution/history/tools and AstrBot channel mechanics.

Use Node 24 and TypeScript 7 ESM. Generated local artifacts belong in ignored `data/`, except standard `node_modules/` and third-party CodeGraph storage. Acquire runtimes explicitly. Keep secrets out of logs and portable data.

Human documentation is Chinese. AI instructions are concise technical English. Use a few real scenarios and necessary rule tests. During iteration run affected checks.

Default branches are integration-only: use a feature branch and PR/squash merge, never push directly to the default branch.
