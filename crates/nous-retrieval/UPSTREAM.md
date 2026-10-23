# Reused source

The index schema, document replacement, literal natural-language term queries,
scope filters and TopDocs materialization in `src/lib.rs` are adapted from
[`crates/retrieval/src/mechanisms/lexical.rs`](https://github.com/Heptalogos-Devs/Nous-Wave/blob/412b062082afb75ccfae4c3f69fc606d3e3cc862/crates/retrieval/src/mechanisms/lexical.rs)
at commit `412b062082afb75ccfae4c3f69fc606d3e3cc862`. Its Apache-2.0 license is
retained in [licenses/Nous-LICENSE](licenses/Nous-LICENSE).

Tantivy 0.27.0's `PreparedCommit::set_payload` commits index coverage with the
projection in the same atomic metadata update. Its implementation was inspected
from the local upstream checkout at commit
`67889269cae67063c372bc236b102d632a021568`.

Chinese and Latin tokenization uses the `tantivy-jieba` 0.21 dependency directly,
with Tantivy's lowercase and long-token filters. The bridge was inspected from
the local upstream checkout at commit
`882dcc4304e415879b9e0169ff439a4806a2bf8e`. Its tokenizer API 0.7 matches Tantivy
0.27; no tokenizer implementation is copied into Nous.

## Frozen vector generations

`src/vector.rs` adapts the candidate/Tag pair, unique F32 cosine configuration,
native filtered search and vector readback from the previous Nous
`crates/retrieval/src/vcp/index.rs` and `mechanisms/dense.rs` at the same
`412b062082afb75ccfae4c3f69fc606d3e3cc862` commit. The retained Apache-2.0
license above covers these sources. [vector-sources.json](vector-sources.json)
records their original hashes and the inspected USearch sources.

USearch 2.26.2 is a published native HNSW dependency, pinned to source commit
`f91fe5bc000222aa1af6e91daf78c2bb20b0c90e`. Its actual Rust bindings, C++ bridge,
header metadata, save/load/get and ANN/filter APIs were inspected from the local
clone and matched against the published crate. Default NumKong SIMD is disabled;
the same native HNSW index runs with its built-in cosine implementation.

The caller supplies one authorized `VectorSnapshot`, including space, producer,
generation, signature, watermark, dimension and stable numeric-ID/source-key
mapping for each lane.
Numeric IDs and source keys are unique within each lane. Candidate and Tag lanes
have independent mappings, including when their numeric IDs overlap.
`VectorGeneration::build` creates the pair in memory;
`create` stages both files and a checksum manifest, then publishes one complete
directory. `open` requires the exact expected snapshot and validates identity,
both file hashes, native headers, counts, IDs and every stored vector once.
Read-only search uses native ANN; additional candidate restrictions are applied
inside native filtered search before Top-K. An empty restriction returns no hits.
`TagSearch` returns cosine similarity as `1 - distance`.

Vectors and queries are normalized in f64 before F32 indexing to keep cosine
arithmetic finite; zero vectors, non-finite values and wrong dimensions are
rejected. The manifest binds both original and normalized vector hashes. Source
keys remain opaque caller-owned references. No business storage or model calls
are introduced here.

The isolated adapter tests passed on Windows: real dual-lane ANN, restricted and
empty searches, save/reopen, invalid inputs, incomplete files, checksum changes,
snapshot mismatches and native readback rejecting substituted IDs. All-target
Clippy with `-D warnings` also passed. Root workspace and serving integration are
managed by the application owner.

## Serving snapshots and graph assets

`src/serving.rs` continues the previous Nous
`vcp/{generation,graph,observation,lane,readout}.rs` implementation at
`412b062082afb75ccfae4c3f69fc606d3e3cc862`. It preserves frozen generation
ownership, ordered membership, independent evidence roots, one query observation
and the original/enhanced/local/transfer candidate union. The formulas execute
through the pinned [nous-vcp kernels](../nous-vcp/UPSTREAM.md); this module adapts
identity, source assembly, persistence and result materialization.

The app supplies a `ServingSnapshot` after scope, source, time and activation
filtering. Memory candidate IDs, Tag IDs and source-root file IDs have separate
sorted mappings. A Memory explicitly names its material root; each Tag occurrence
must name that same root. Multiple memories from one root share one ordered Tag
membership and one contribution to geometry/graph evidence. A conflicting
membership for one root is an explicit input error. Imported literal material
labels remain source-derived Tags in this projection.

`ServingGeneration::create_or_open` hashes the complete raw snapshot, normalization policy,
caller identity and asset configuration to select an immutable generation. It
publishes geometry, CSR/provenance, identity mappings and both native ANN indexes
as one directory. Reopen checks component hashes and native vector contents,
reuses persisted geometry and graph, and makes no model call. Incomplete or
changed components return a specific error; a changed snapshot selects a new
generation. The app owns any cache and publication gate.

Dense uses native candidate ANN. VCP prepares one complete pipeline observation,
offers candidates from its four actual query vectors, and passes the closed
union to RiverMemo V3. Optional fields and grounded Thinking use that observation.
An empty Tag lane still executes the VCP pipeline and retains direct candidates.
Returned numeric IDs resolve through the frozen Memory mapping, and source
mappings cover returned and planned memories. Normal query diagnostics contain counts;
the full generation mapping is available through `asset_diagnostics()`.

Four standalone serving tests passed: full native save/reopen and query replay;
graph/provenance restricted to caller-filtered material and unchanged by repeated
derivation from one root; and no-Tag VCP with explicit corrupt/new-generation
behavior; and non-unit provider vectors with one derived normalization policy. All-target Clippy with `-D warnings` passed. The app composes these assets with the shared model ledger and durable embedding
worker; controlled protocol tests and the real gateway exercise cover the complete
query, source and restart path.

The serving format `nous-vcp-serving-ags-unit-f32-v2` binds its derived vector
policy. Authoritative model values and ANN raw snapshot hashes remain intact;
one f64 L2-to-F32 derivation supplies geometry, graph, MemoAssets and readout.
This makes VCP fusion and ANN use compatible cosine inputs for provider vectors
whose norms differ slightly from one. Current VCP source keeps raw vectors in
`storage.rs` and uses them directly in `memo_pipeline.rs` fusion; the adaptation
records this intentional serving policy separately from the preserved kernels.
