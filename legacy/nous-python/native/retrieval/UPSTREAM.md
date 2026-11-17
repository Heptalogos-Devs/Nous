# Native index source adoption

The lexical and vector index owners are adapted from Nous-Wave commit
`412b062082afb75ccfae4c3f69fc606d3e3cc862`, under Apache-2.0. The retained license
is bundled in ../../src/nous_native/licenses/Nous-LICENSE-APACHE.

Tantivy 0.27 and tantivy-jieba 0.21 provide literal term queries and Chinese/Latin
analysis. Immutable Wave snapshots use a single committed frozen generation.
The caller supplies candidate identity and allowed rows before Top-K.

USearch 2.26.2 supplies the two native HNSW indexes for candidates and Concepts,
using cosine/F32 unique IDs. Index publication and reopening verify component
hashes, format, row IDs and actual stored vectors. Original provider vectors
remain owned input; indexing derives unit F32 values with f64 norm accumulation.
vector-sources.json records the original source and inspected USearch revision.

The retired serving layer is replaced by the frozen Wave orchestration in
../python/src/engine.rs and query.rs. Business content, model calls, source
resolution and snapshot publication remain with Python WaveOperations.
