# src/nous development guide

Locate the owner for the task with [INDEX.md](INDEX.md), CodeGraph, and the actual call chain, then read the related design and implementation. Fix a missing or wrong capability at its owner, and strengthen the shared infrastructure. Do not bypass it locally at the use site, add a parallel implementation, or use a wrapper to keep a structure that is already unsuitable.

The rules below distill the integration approach and the key constraints of the current design. The corresponding design topic is the authority for the detailed contract. When a responsibility or public contract changes, update the owning module, every actual caller, and the corresponding design together.

## Instance and assembly

- Manage instances through `initialize_instance` and `open_instance`. Paths and identity use `InstanceLocator`, `ResolvedPaths`, and the instance bindings. An ordinary open must not create a replacement database or bypass the resource lock.
- The composition root constructs dependencies explicitly. A service can be used once constructed; do not backfill dependencies after it is published. The owner of a new table supplies it, and explicit initialization collects those tables together.
- Resources belong to the runtime lifetime. On shutdown, stop accepting work and finish operations, blocking work, and media subprocesses before closing model connections, storage, observability, and the instance lock. Leave no background work the runtime cannot wait for.

## Configuration

- Define configuration fields in the module that owns them and connect them to `NousConfig`. `Configuration` handles source merging, the effective snapshot, and saving the file. Online and offline changes both use `ConfigurationOperations`.
- Business code does not read environment variables on its own, keep a copy of the defaults, or write its own binding rules for the instance, time, or a vector space. An application operation calls the actual owner to check a binding and adopt it.
- One operation uses a fixed configuration snapshot. Whether a configuration change applies immediately or waits for restart follows the existing contract. An entry point does not decide that on its own.

## Execution and transactions

- Before binding input, application dispatch fixes the request context with `ExecutionManager.request`. A business stage uses `operation` / `stage` and inherits the configuration snapshot and the deadline. Blocking work uses `ExecutionManager.blocking`. Cancellable Native work passes `on_cancel` and waits until that work has actually exited. The current Runtime holds background maintenance with `spawn`. The actual owner manages resource capacity.
- SQLite uses `Database.read` / `write` and the caller's transaction. Keep slow parsing, network requests, and byte publication outside the write transaction. Code that already holds a transaction does not call an operation that opens another one.
- One formal adoption commits the related results and references in the same short transaction. Cross-owner work uses the `*_in` methods that receive the existing `WriteTx`. Do not build atomicity by committing each piece and then compensating.

## Content and sources

- Store bytes in `ContentStore`, artifact identity in `ArtifactStore`, and processing results in `ResultStore`. A CAS digest represents bytes. It does not replace source identity. Two acceptances of the same bytes may be different Artifacts.
- When owners adopt together, publish the bytes first, then commit each owner's records in the same `WriteTx`. The first embedding-space binding, the result, and the vector relation commit together.
- Content acceptance, media preparation, model requests, and result adoption use the shared reference and value types. Do not invent a parallel file, source, or result structure. Selections, actual coverage, and derivation relations stay consistent with the topic that owns them.
- Artifact operations live in `content.artifacts`, and result adoption lives in `content.results`. Read the related ancestors and the terminal root through `ResultStore.provenance`. Express the set of provenance roots and the independent-support groups of relations separately. Do not infer independent support from material that was only used together.
- Markdown structure uses `ArtifactStore.text_view` / `read_blocks` and the shared `TextView`. A block key fixes the text and its parse profile. Model-selected segments and concept annotations use these exact references.
- For natural-language handling, do not base that handling on a particular language's keywords, word lists, language categories, or content enumerations, and do not use regular expressions or a similar device for it. Lexical full-text search, and domain work the user has explicitly requested such as translation or summary, continues under its own contract.
- The cognition manuscript owner adopts Note and Journal body versions and their citations. Model writing first saves the `ProcessingResult` and the draft, then adopts the formal manuscript in a separate transaction. Cognition also adopts a Concept's sense version and the body annotations, and fixes the version of each.

## Prompts, models, and media

- Select and compile prompts with `PromptCatalog` / `PromptCompiler`. A static `TaskDefinition` owns the task output and the meaning of its sources. Generation, transcription, preview, and probe go through `GenerationOperations`. Protocol execution goes through `ModelExecutor`. Media preparation and interpretation use `MediaPreparer` / `MediaAnalysis`.
- `ModelConnections` and `Credentials` manage the gateway connection and credentials. Use the existing dependencies for protocol encoding, media parsing, and conversion. Do not reimplement a client, a retry policy, or a parser.
- Select behavior from the explicit protocol, capability, and task configuration. Pass the model name through unchanged. Do not add a business branch for a model name or a vendor. Deploying and administering New API belongs to development integration, and does not enter product logic.
- Media preparation owns deterministic decoding, selection, conversion, and a plan that refers to bytes without carrying them. Materialize one bounded batch before consumption. Media interpretation owns the policy and the stage handoff for transcription, description, and structuring, and reuses the shared generation operations. The CLI, a model client, and a later cognition area do not each write their own preparation.
- `models.input` plans the set of semantic material. An ordinary task uses joint. An explicit request or the task definition selects an independent semantic unit. A batch of physical representations does not change the semantic boundary. The plan stores the MIME of the actual representation and the MIME of the source.
- Model execution does not access the business database. Application operations adopt the result. When the model has already succeeded and saving fails, handle the adoption failure. Do not request the model again.
- Fetch and reuse text vectors in batches through `EmbeddingOperations.ensure` / `read`. Read a formal vector row by space, purpose, and exact input. Do not go around the owner with a private model client or a private vector cache.
- `run`, `ensure`, `probe`, and a temporary query share `EmbeddingExecutor.encode_batches`. A temporary query uses `encode_query`. A stored vector uses `VectorRef` / `read_vector`. The table's unique key selects the canonical adopted row. A network request does not hold the instance-level reuse lock.
- Divide an LLM task, choose its actual input, handle its result, and write its prompt according to [LLM use and prompt design](../../docs/agents/llm-use.md). Judge the output by the information the model actually received. The system owns deterministic business rules.

## Wave and Native

- In one consistent read transaction, the cognition domain supplies the formal manuscripts, concepts, annotations, and source views. `WaveOperations` receives a typed `CorpusView` and shares asset construction and snapshot publication with manifest import. PDF retrieval reuses `MediaPreparer`. Native receives only frozen arrays and rebuildable assets.
- A build publishes an immutable snapshot. Query, Thinking, and reading the original text consume that snapshot. An original source sequence, a candidate-curve manuscript, and an independent evidence root are different identities. Derived material does not contribute the same independent fact again.
- Rust exports the Wave parameters, and `WaveConfig` connects them to the shared configuration. Python does not implement a fallback retrieval algorithm. See [native/AGENTS.md](../../native/AGENTS.md) for the Native pool, the cancellation token, and the data boundary.

## Time, errors, and observation

- Represent time with the domains, clocks, and value types in `temporal`. `TimeService.validate` checks that a coordinate domain is registered. Preserve unknown values, precision, and the coordinate domain. A media page number, a frame number, or a relative interval must not be treated as real-world time.
- Use `NousError`. The entry point presents it. Do not hide an actual failure behind an empty result, default data, or a success status.
- The shared operation context carries the root-operation correlation, the attempts, and the completed references. A request uses `models.policy.attempt_scope`. Cancellation keeps its original exception type until the entry point. Read diagnostics from the existing JSONL with `runtime logs --operation`.
- Basic instrumentation uses the standard logging library and `observability.instrumentation`. SDK assembly stays in `observability.assembly`. A basic module does not take a reverse dependency on the full SDK, an exporter, or business assembly for the sake of instrumentation.
- Logs, errors, and formal results contain no credentials.

## Entries and responsibilities

- The CLI converts arguments, calls a complete operation, and presents the result. Business rules stay in the shared operations. Language selection and copy use `presentation`.
- A user invocation should complete a meaningful operation. It does not require the user to run the internal prepare, publish, attach, or compile steps one by one.
- The CLI, an SDK, and the host reuse the same application operations. Register the typed request, result, and events with `define_operation`. Bind external input through `CallContext.inputs`. After the formal commit, publish the product reference with `call.saved`. A later entry keeps that contract. Do not expose every internal method as its own tool.
- For a new capability, check the existing owner and the dependencies already chosen, and organize the code by the real high-cohesion responsibility. Do not create a layer of class, file, or package for every term.

## Maintaining source knowledge

When a responsibility, a public contract, configuration ownership, or an actual calling pattern changes, update [INDEX.md](INDEX.md), the corresponding connection constraint in this file, and the affected design topic, and remove guidance that no longer holds. INDEX.md locates the responsible place. This file says how to connect to it. Neither one copies the full design or accumulates implementation history.
