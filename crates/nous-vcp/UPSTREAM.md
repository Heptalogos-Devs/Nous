# Native VCP sources

Current kernels are copied and adapted from `lioensky/VCPToolBox` at
`d1b40c863273cbbba082fb7657671424768c4a53`, under its retained
CC BY-NC-SA 4.0 license in `LICENSE-VCP`. Exact source hashes are recorded in
`upstream-sources.json`. Database, N-API and host access are replaced with typed
immutable input. Numerical kernels retain their source computation and ordering.

| Original source in `rust-vexus-lite/src` | Adapted module and boundary |
| --- | --- |
| `memo_geometry.rs` | `geometry`: AGS core copied; local/global asset orchestration takes explicit vectors, memberships, model signature and timestamp |
| `memo_artifact_builder.rs` | `artifact`: fact, transport, provenance and signature computation copied; input rows replace SQL and `buildThreads` replaces environment lookup |
| `memo_sensing.rs` | `sensing`: state propagation and dynamic residual wormholes copied; in-memory vector pool replaces database reads |
| `memo_projection.rs` | `projection`: basin and bounded growth computation plus all eight source tests copied |
| `memo_pipeline.rs` | `pipeline`: EPA, Pyramid, gating, fusion and dual-field computation copied; typed assets/search replace handles, global caches and host reads |
| `rivermemo_topology_v3.rs` | `graph` and `river`: CSR, full readout, direct anchors, morphology, Omega and scoring copied; candidate/file data replace SQL loads |
| `memo_thinking.rs` | `thinking`: predecessor-grounded stage planning copied; frozen file metadata replaces host source lookup |

Adaptation adds validation at public input boundaries, immutable asset generations,
structured diagnostics and Rust I/O types. Small Clippy style changes preserve
formulas and accumulation order. Targeted allowances retain upstream numerical
function signatures, tuple decomposition and indexed loops.

The `baseline` module and legacy fixture directory are copied from Nous Wave
master `412b062082afb75ccfae4c3f69fc606d3e3cc862`, under the retained Apache-2.0
license in `licenses/Nous-LICENSE-APACHE`. Its SVD-based EPA is a separately named
historical baseline. This component record does not select the whole product's
distribution license.

The original reference tree `crates/retrieval/src/reference/**` maps to
`src/baseline/**`; `crates/retrieval/tests/vcp_reference.rs` and `tests/vcp/*.rs`
map to `tests/baseline_golden.rs` and `tests/legacy/*.rs`. Their JSON corpus maps
from `docs/research/corpus/vcp` to `tests/fixtures/legacy`. Only imports and fixture
paths change; the original JSON bytes and expected numbers are preserved. The
source manifest records every original reference/test/corpus file hash.

The original AGS comparison script referenced an uncommitted fixture
`scripts/fixtures/memo_geometry_v2.rs`. For the 2026-10-23 local comparison, that
fixture was recovered from the actual geometry source at upstream commit
`39ee4c2fa6babb506dc1f6893f661d83ded22016`; the locked current source stayed at
`d1b40c863273cbbba082fb7657671424768c4a53`. The unchanged harness then ran 2,000
random/degenerate cases, six boundary cases and its alternating-order benchmark.
The report retains the observed rank differences, source hashes and compiler.
Current AGS recovered the analytic 3e-7 residual and reduced near-collinear
orthogonality error to roughly 1e-14 in that run.
