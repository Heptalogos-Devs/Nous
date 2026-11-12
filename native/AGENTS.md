# Native computation

This directory is a Rust cargo workspace. `vcp` holds the traceable pure numeric kernels, `retrieval` organizes Tantivy, USearch, query snapshots, and grouped recall, and `python` is the thin PyO3 binding for `nous_native._core`. Interfaces, lifetimes, and data shapes follow the [Rust Native interface and lifetime](../docs/design/wave/native.md). The retrieval core is in [Retrieval core](../docs/design/wave/engine.md).

## Boundaries

- Native manages only the index and staging directories passed in by Python. It does not know the business database address, the CAS root, or the content directories, and it does not access the network.
- Inputs are frozen arrays and rebuildable assets. Do not borrow a NumPy buffer the caller can still write. Hot loops do not call Python.
- Rust types export algorithm parameters, defaults, and the schema. Python connects them through `WaveConfig` and does not keep a second handwritten set of defaults.
- Computation uses a bounded Rayon pool per instance. The cancellation token is checked during iteration, batches, and indexing. After cancellation, wait for the actual work to exit before releasing the snapshot.
- Express errors as `Failure` / `NativeError` with a code. Do not classify them by matching error text, and do not return an empty implementation with `success=true`.

## Building and changing

- From the repository root, rebuild the extension with `uv sync --locked` through maturin. First run `source ./dev.sh env` (on Windows, `. ./dev.ps1 env`) so the cargo directories land in `temp/`.
- After changing `vcp` or `retrieval`, run the corresponding tests from the repository root with the development check: `tests --rust nous-vcp` or `tests --rust nous-retrieval`. The Windows entry is `pwsh -File ./scripts/check/run.ps1`; other systems use `sh scripts/check/run.sh`. These two crates enable the workspace lints and forbid `unsafe`. Verify a change to the `python` crate through a real `nous tools wave` call.
- When an interface or behavior changes, update the design topics above. When a source location or responsibility changes, update [INDEX.md](../src/nous/INDEX.md).
