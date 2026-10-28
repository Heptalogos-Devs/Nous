# Reused source

The index schema, document replacement, literal natural-language term queries,
scope filters and TopDocs materialization in `src/lexical.rs` are adapted from
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

`src/vector/{snapshot,engine,persistence}.rs` adapts the candidate/Tag pair,
unique F32 cosine configuration, filtered search and vector readback from the
previous Nous `crates/retrieval/src/vcp/index.rs` and `mechanisms/dense.rs` at
`412b062082afb75ccfae4c3f69fc606d3e3cc862`. The retained Apache-2.0 license
covers these adaptations. [vector-sources.json](vector-sources.json) records
original hashes and the inspected USearch sources.

USearch 2.26.2 is a native HNSW dependency pinned to source commit
`f91fe5bc000222aa1af6e91daf78c2bb20b0c90e`. The actual Rust bindings, C++ bridge,
metadata and persistence APIs were inspected from the local clone and matched
against the published crate. Default NumKong SIMD is disabled; native HNSW uses
its built-in cosine implementation.

The caller supplies one authorized `VectorSnapshot`: space, producer, generation,
signature, watermark, dimension and stable ID/source mappings. IDs and source keys
are unique within each lane; candidate and Tag lanes have independent mappings.
`build` creates the pair in memory. `create` stages both indexes and checksums,
then publishes one complete directory. `open` verifies the expected snapshot,
file hashes, native headers, counts, IDs and stored vectors once. Search uses
native ANN; restrictions apply before Top-K, including the empty allowed set.
`TagSearch` returns cosine similarity as `1 - distance`.

The cosine boundary derives unit F32 vectors using f64 accumulation. The manifest
binds both original and derived vector hashes. Zero, non-finite and wrongly sized
vectors are rejected. Source keys remain opaque caller-owned identities.

## Serving snapshots and graph assets

`src/serving/{snapshot,assets,persistence,query}.rs` continues previous Nous
`vcp/{generation,graph,observation,lane,readout}.rs` at the same commit. It preserves
frozen generations, ordered membership, independent evidence roots, one query
observation and the original/enhanced/local/transfer candidate union. Numerical
computation uses the pinned [nous-vcp kernels](../vcp/UPSTREAM.md).

The caller freezes scope, source, time and lifecycle before supplying
`ServingSnapshot.candidates`. Each `ServingCandidate` has its exact ID/revision,
raw vector and nonempty complete `source_refs`. Each Tag occurrence names one of
those roots. Candidate IDs, Tag IDs and source-root IDs have separate sorted
mappings. Candidates sharing one original root keep individual ordered Tag curves; root membership is the ordered union of their declared Tags. Each original root contributes independent evidence once.

A multi-source candidate has its own curve over its declared Tags. RiverMemo and
Thinking exclude every own root when measuring independent edge support. Source
mappings and independent-root sets survive grouped persistence and restoration.

`ServingGeneration::create_or_open` derives the generation from actual candidates,
space, producer, watermark, dimensions, asset configuration and normalization
policy. Caller generation labels do not enter that content identity. Geometry,
CSR/provenance, mappings and both ANN indexes publish together. Reopen verifies
component hashes and native vector contents and reuses accepted raw vectors.

Dense uses native candidate ANN. VCP executes the complete pipeline and RiverMemo
readout; optional fields and grounded Thinking share its observation. An empty
Tag lane preserves direct candidates through that same pipeline. Returned IDs
resolve through the frozen candidate mapping. Query diagnostics contain counts;
`asset_diagnostics()` supplies the complete generation mapping.

ANN and VCP assets use one shared `raw-f32/l2-f64-to-unit-f32` conversion. Source
vectors retain their original bytes and hashes. This serving policy adapts the
upstream raw-vector fusion path so provider vectors with slightly non-unit norms
use consistent cosine inputs throughout retrieval.

Public behavior checks live in `tests/lexical.rs`, `tests/vector.rs`,
`tests/serving.rs` and the real ANN-to-VCP wiring case `tests/vector_pipeline.rs`.
