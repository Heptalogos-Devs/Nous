# Agent guide

Start with the self-contained [NousQL manual](NOUSQL.md). It defines the objects,
query language, time views, optional exploration and a complete Agent workflow.

Strongly prefer [explicit known referents](NOUSQL.md#prefer-explicit-referents-for-retrieval)
and searchable keywords. Uncertain referents remain valid original queries.
Create and foreground a WorkContext once, then reuse its text and real anchors.

`nous help nousql` provides a daemon-free text quick reference; use `--json` when
a program needs the structured CLI projection.
Normal commands return stable lexical references and local `result:N` / `query:last`
handles. `show src:…` or `show art:…` lists up to 20 actual observations of that
Artifact; select an `obs:…` explicitly when forming cognition. Repeated observations
remain distinct. `read <Material reference> --max-bytes <bound> --output <new-file>`
exports complete exact bytes for inspecting original media. An existing file is
never overwritten; partial input requires a larger bound or a bounded region.
For implementation details, use the Chinese [developer reference](../reference/NOUSQL.md).

The default MCP connection supports cognition reads, observations and task context.
Authority mutations require an operator-started `--management` connection. A chat
request to remember or forget does not change the connection scope. `self soul`
reads a derived current view; adoption, revision and interpretation are explicit
management operations. See the [CLI reference](../../apps/nous-cli/README.md).

[Documentation index](../INDEX.md)
