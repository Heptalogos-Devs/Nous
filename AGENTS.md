# Nous Development Guide

This file applies to the entire repository. When modifying content in one part of it, you should also follow the `AGENTS.md` of the corresponding directory, such as the [AGENTS.md](src/nous/AGENTS.md) of `src/nous/`. No Agent may modify, move, or delete `AGENTS.md` or other such files used to govern Agents unless the developer has explicitly permitted and requested it. The rule for changes to these files: create a Git patch file `AGENTS.proposal.patch` in the repository root, and promptly remind the developer to review it. There is no need to stop other unrelated work for the proposal. After the patch is applied, delete it yourself.

## AntiRLHF

Required reading before acting: the [AntiRLHF Charter](docs/agents/antirlhf/CHARTER.md) and the [CONSTITUTION](docs/agents/antirlhf/CONSTITUTION.zh-CN.md), especially for GPT models. AntiRLHF is project owner's model-governance scheme, directed at "Local Defensibility Optimization (LDO)", over-engineering, over-testing, meaningless compatibility, and the behavior of substituting verifying or proving process for *real engineering work*.

Apply these requirements into actual actions and decisions: find where responsibility for the problem lies, complete the usage path the user needs, modify structures that are not appropriate, and clear out the implementations that have been replaced. Do not use extra reports, checklists, or repeated declarations of compliance with the rules as a substitute for the work.

## The Project and Current Work

Nous is an independently installed long-term cognitive service, oriented toward memory, knowledge, sources, time, retrieval, and cognitive growth. The Python main application unifies domain operations and the formal entry points. Necessary Rust PyO3 Native takes on the performance hotspots.

The code currently running is in `src/nous/`. `legacy/` is retired research material and does not participate in running; when a capability in it is needed, migrate/adapt it in according to the current contract, and do not restore the entire old runtime stack.

Read the relevant entries according to the task. There is no need to read all of the designs and historical discussions all the way through:

| What to understand | Entry |
| --- | --- |
| Current capabilities, goals, and subsequent work | [docs/CURRENT.md](docs/CURRENT.md) |
| Ownership, dependency direction, and product boundaries | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Design topics, implementation plans, and document responsibilities | [docs/README.md](docs/README.md) |
| Existing implementations, symbols, and callers | [src/nous/INDEX.md](src/nous/INDEX.md) |
| Installation, running, and product entry points | [README.md](README.md) |

`docs/design/` defines the current contract, `docs/plans/` arranges the dependencies and delivery of specific work, and `docs/sources/` retains the original discussions. Proposals in the original discussions cannot be taken directly as approved decisions. When code and design are inconsistent, judge, in conjunction with the current requirements, which side should be modified, and do not take the old implementation or old tests as the final standard.

## Engineering Choices & Principles

- The project is still in the prototype stage. For the time being, keep `v0.1.0`. It has no external consumers, and it has no need for internal compatibility or for incrementing internal versions. When a structure, the form/shape of the data, or a framework is not appropriate, directly replace the responsible implementation, migrate the actual producers, consumers, configuration, tests, and documents, and delete the replaced code and the support dedicated to it. Internal implementations must not add incrementing identifiers such as `v1`/`v2`, version chains, compatibility branches, or dual-track paths, and must not establish a mechanism of old-format retention or backup migration for internal structures being eliminated. Business revisions of user manuscripts and precise historical citations continue to be preserved according to the product contract.
- It is prohibited to make local bypasses, wrappers, and duplicate implementations in order to preserve a wrong/inappropriate structure. First judge who should be responsible for a behavior, a piece of data, or a state; then choose where to implement it. Existing modules may be reused, and, because of a real need, may also have their responsibilities redivided and be refactored.
- Prefer to adopt the formal capabilities of mature dependencies already chosen. DO NOT rewrite, on your own, existing capabilities such as protocol clients, media parsing, and database mechanisms, and do not mechanically establish a service, repository, manager, or forwarding layer for every concept.
- Remain vendor-neutral. LLM services are accessed through a unified gateway. "New API" is one usable gateway. Production code does not add special business behavior for a particular external platform. Configure the work according to explicit protocol, capability, and task, and do not guess capabilities from model names.
- Follow the [LLM usage and prompt guide](docs/agents/llm-use.md) when designing or assessing model calls. Provide the context needed for the task and assess outputs using the information actually available to the model.
- When completing a capability, also handle the actual producers, consumers, configuration, and user entry points. Do not leave functionality that is valid only in an internal method or in demonstration code.

## Verification

TDD and red-green cycles are prohibited. Must not add shallow tests, field-existence tests, or repeated verification of behavior that a dependency library already owns. Except for key contracts, security details, and independent failures that actually have protective value, prefer to verify the complete path through the real user entry point (end to end). Verifying wrong input and failure behavior is not manufacturing failures for a red-green cycle.

Choose a verification method that can expose problems in the current change. Check functional results and failure behavior. DO NOT substitute commit count, test count, or an all-green test run for product judgment. State the verification actually executed and the results; distinguish simulation from real external services, and the scope that was not verified. DO NOT treat a development report or remote CI as evidence.

When an old test only protects an old implementation, delete it together with the old implementation. Migrate usage situations that still have value to the new entry point. Do not, for tests, add a dedicated export point or a parallel implementation to production code, or set up an extra test framework.

Put development scaffolding, temporary probes, experimental tests, and scripts in `temp/`. They must not enter Git. Tests are located in `tests`.

## Document Maintenance

When design, a public contract, or responsibilities change, update the responsible topic and the source index in place, and remove statements that no longer hold. Define the same rule in only one responsible place; other places cite it and DO NOT copy multiple versions.

For important classes and methods, use comments to state responsibilities, state boundaries, or non-obvious invariants, and do not restate the implementation line by line. For the details, see [docstring standard](docs/agents/docstring-std.md).

`CURRENT.md` records current capabilities, decisions in effect, and remaining work. DO NOT write it as an operations log or changelog. DO NOT write each correction up as a permanent prohibition, and DO NOT leave retired schemes, the implementation process, or self-review explanations in product documents, code comments, or the UI.

## Development Environment and Commands

Use Python 3.14 and uv. First set up the development environment in the repository root, so that the virtual environment, downloads, and caches are located in `temp/`.

Linux / macOS:

```bash
source scripts/dev-env.sh
```

Windows PowerShell:

```powershell
. ./scripts/dev-env.ps1
```

Install the development dependencies and view the entry points:

```sh
uv sync --locked
uv run --locked nous --help
uv run --locked nous tools --help
```

The check commands are as follows. Choose which to use according to the change. It is not required to mechanically run all of them on every small modification:

```sh
uv run --locked pytest tests/python
uv run --locked ruff check src tests/python
uv run --locked ruff format --check src tests/python
uv run --locked basedpyright
```

Audio and video processing requires FFmpeg and ffprobe. Configure them through PATH, or through `media.ffmpeg_path` and `media.ffprobe_path`. For the concrete product commands, see the [Tools CLI](docs/design/interfaces/tools-cli.md). When model resources are involved, read the [model configuration example](docs/examples/models.toml) and the related design. Do not write the gateway address, model name, or credentials of a particular development machine as product defaults.

This project is cross-platform. Except where the requirement itself involves platform adaptation, do not write the development machine's absolute paths or its particular settings into project documents or Git, and do not create long-term autostart or background services.

## Temporary Data

Put generated products, reference repositories, worktrees, downloads, installations, and experimental scripts all in `temp/`. Do not scatter them into the user directory or other locations. External development tools that already exist may continue to be used. Newly added temporary project content still follows this convention.

Use the [temp management tool](scripts/temp/README.md) to create directories, view status, and clean up.

## Code and Reference-Project

### Nous Source Code

When locating code, understanding symbols, and tracing call relationships, first use the CodeGraph MCP's `codegraph_explore`, and specify the file, symbol, or concrete problem in the query. You may also run the following in the project directory:

```sh
codegraph explore "<symbols or questions>"
```

When CodeGraph or the index is unavailable, continue locating by file and symbol search. You may also choose to install or repair that tool, but you need to explain this to the developer and request permission. Along the hit locations, read the responsible module and the real callers. Do not decide the modification only on the basis of a search summary.

### Reference Implementations

When retrieving reference implementations, read [reference-project retrieval](docs/agents/reference-code.md).
