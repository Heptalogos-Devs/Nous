# nous-vcp

Typed Rust computation copied and adapted from pinned VCP sources. The crate
accepts immutable numerical assets and an actual Tag search adapter. Host storage,
model calls, source authority and native ANN are supplied by their owners.

| Module | Computation |
| --- | --- |
| `geometry/` | Anchored greedy Gram-Schmidt, local/global intrinsic residuals, anchor gains and EPA assets |
| `artifact/`, `graph` | Fact/transport matrices, CSR, source provenance, wormholes and content signatures |
| `sensing/`, `projection` | Finite-hop predecessor states, FIR/return-current suppression, dynamic wormholes and bounded basin growth |
| `pipeline/` | EPA, residual Pyramid, gates, sensing, fusion and local/transfer fields |
| `river/` | Curve selection, topology, direct anchors, morphology, Omega and readout scores |
| `thinking` | Grounded stage plans using actual predecessor transitions and candidate closure |
| `baseline/` | Preserved previous Nous reference, including SVD EPA, DTSC and topology scoring |

`MemoAssets::try_new` validates the frozen dimension, generation, signature, CSR,
Tags, files and candidate vectors. Missing vectors for declared Tags are reported
by diagnostics. `PipelineOutput` carries the actual observation and both fields
for subsequent readout and planning; its asset identity must still match.

`PipelineInput`, `ReadoutRequest` and `ThinkingInput` retain upstream parameter
names. Thinking requires non-truncated transitions; request a positive
`spikeRouting.maxTransitionRecords`. Empty sensing seeds preserve upstream
core/ghost supplementation and direct candidate behavior.

Run `cargo test -p nous-vcp --all-targets` for the preserved numerical corpus,
source projection cases and connected pipeline checks. Run
`cargo run -p nous-vcp --example native_pipeline` for the complete offline replay.
Fixtures and original source hashes are committed within the crate. Real USearch
wiring and grouped persistence are exercised in `nous-retrieval`.

Current VCP adaptations retain CC BY-NC-SA 4.0. The previous Nous baseline retains
Apache-2.0. [UPSTREAM.md](UPSTREAM.md) and [upstream-sources.json](upstream-sources.json)
record exact sources, original hashes and adaptation boundaries.
