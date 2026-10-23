# nous-vcp

Typed, native Rust computation copied and adapted from the pinned VCP sources.
The crate has no N-API, database, filesystem, model or network dependency. The
caller supplies a frozen numerical snapshot and the actual Tag search adapter.

| Module | Callable computation | Status |
| --- | --- | --- |
| `geometry` | `anchored_gs`, `build_geometry`: local/global AGS, intrinsic residuals, anchor gains, EPA mean/basis/coverage | Compiled; analytic residual and full-pipeline fixture exercised |
| `artifact` / `graph` | Directed fact/transport graph, hub and wormhole weights, CSR, source provenance and signatures | Compiled; invoked by the pipeline fixture |
| `sensing` / `projection` | Finite-hop predecessor states, FIR/return-current suppression, dynamic residual wormholes, basin projection and shared budgets | Compiled; upstream projection tests and a fixed 0.028 tail-edge counterexample exercised |
| `pipeline` | EPA, residual Pyramid, gating, Sense, fusion, local/transfer fields and projections | Compiled; connected fixture exercised |
| `river` | Full topology V3 readout, multi-source candidate pool, geometry, direct anchors, morphology, Omega and conditional innovation scoring | Compiled; scoped readout and generation mismatch exercised |
| `thinking` | Grounded stage planning using actual predecessor transitions, file scope and candidate closure | Compiled; connected fixture exercises a grounded route |
| `baseline` | Complete previous Nous numerical reference, including SVD EPA, DTSC and V3 | 23 preserved golden tests pass; kept separate from current AGS |

The semantic QueryPlan, identity mapping, historical/source permission filtering,
actual embeddings, ANN implementation and persistent asset publication are supplied
by Nous retrieval and its authority owners. `MemoAssets::try_new(MemoAssetInput)`
validates and freezes one dimension/generation/signature, CSR, declared Tag IDs,
file membership and candidate vectors. Missing vectors for declared Tags are
reported by `diagnostics()`. Tag search results must use these declared IDs and
finite scores. Tag vectors and names are stored once, file membership once, and
candidate vectors by chunk ID. `PipelineOutput`
holds the actual observation and both fields for subsequent readout/planning;
its generation and artifact signature must still match the supplied assets.

`PipelineInput`, `ReadoutRequest` and `ThinkingInput` use the upstream camelCase
parameter names. Method planning requires non-truncated transitions; set a positive
`spikeRouting.maxTransitionRecords` when requesting it. Empty Sense seeds remain a
valid observation and preserve the upstream core/ghost supplementation behavior.

The 35 tests cover the eight copied projection tests, 23 previous Nous golden
tests and four current-kernel integration/boundary cases. The isolated manifest
`temp/spikes/vcp/Cargo.toml` ran all targets and Clippy with `-D warnings` successfully
on 2026-10-23. The preserved tests and fixture JSON are in `tests/`.

From the root workspace, run `cargo run -p nous-vcp --example native_pipeline` for
the complete offline numerical replay. Its exact fixture scan supplies the Tag
search boundary. The executed replay produced three observation nodes, six
transitions, scoped result `1001` and source `memory:fixture/alpha@revision-1`.
Its complete output is temporary; source input and source mapping live in this
crate. Production retrieval next binds real embeddings, indexes, asset publication
and authority-owned source identities to these kernels.

Current VCP adaptations retain CC BY-NC-SA 4.0. The previous Nous baseline retains
Apache-2.0. See [UPSTREAM.md](UPSTREAM.md), [source hashes](upstream-sources.json)
and the accompanying license files.
